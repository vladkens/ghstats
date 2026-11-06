use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::get;
use chrono::{SecondsFormat, Utc};
use db_client::RepoFilter;
use reqwest::StatusCode;
use state::{AppState, DB_HEALTH_INTERVAL_SECS, DB_NOT_WRITABLE_MESSAGE, is_sync_healthy};
use tower_http::trace::{self, TraceLayer};
use tracing::Level;
use types::Res;

mod db_client;
mod filter;
mod gh_client;
mod helpers;
mod routes;
mod state;
mod types;
mod utils;

const DEFAULT_METRICS_CRON: &str = "0 59 * * * *";

async fn check_new_release(state: Arc<AppState>) -> Res {
  let tag = state.gh.get_latest_release_ver("vladkens/ghstats").await?;
  let mut last_tag = state.last_release.lock().unwrap();
  if *last_tag != tag {
    // a dev build is ahead of its own release tag, so only a newer release is worth a log
    if utils::is_newer_release(&tag, env!("CARGO_PKG_VERSION")) {
      tracing::info!("new release available: {} -> {}", *last_tag, tag);
    }

    *last_tag = tag.clone();
  }

  Ok(())
}

fn metrics_cron_schedule() -> String {
  std::env::var("GHS_CRON_SCHEDULE").unwrap_or_else(|_| DEFAULT_METRICS_CRON.to_string())
}

fn new_metrics_job(state: Arc<AppState>, cron_schedule: &str) -> Res<tokio_cron_scheduler::Job> {
  let job = tokio_cron_scheduler::Job::new_async(cron_schedule, move |_, _| {
    let state = state.clone();
    Box::pin(async move {
      if state.version_check {
        let _ = check_new_release(state.clone()).await;
      }

      if let Err(e) = helpers::update_metrics(state.clone()).await {
        tracing::error!("failed to update metrics: {:?}", e);
      }
    })
  })?;

  Ok(job)
}

async fn start_cron(state: Arc<AppState>, cron_schedule: &str) -> Res {
  use tokio_cron_scheduler::JobScheduler;

  let job = match new_metrics_job(state.clone(), cron_schedule) {
    Ok(job) => {
      tracing::info!("metrics cron schedule: {}", cron_schedule);
      job
    }
    Err(e) => {
      tracing::warn!(
        "invalid metrics cron schedule '{}': {:?}; using default '{}'",
        cron_schedule,
        e,
        DEFAULT_METRICS_CRON
      );
      let job = new_metrics_job(state.clone(), DEFAULT_METRICS_CRON)?;
      tracing::info!("metrics cron schedule: {}", DEFAULT_METRICS_CRON);
      job
    }
  };

  // Try once on startup, but keep the scheduler alive if GitHub is unavailable.
  if let Err(e) = helpers::update_metrics(state.clone()).await {
    tracing::error!("failed to update metrics: {:?}", e);
  }

  let repos = state.db.get_repos(&RepoFilter::default()).await?;
  if repos.is_empty() {
    tracing::info!("no repos found after startup sync; waiting for next scheduled metrics update");
  } else if let Err(e) = state.db.update_deltas().await {
    tracing::error!("failed to update deltas: {:?}", e);
  }

  // https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api?apiVersion=2022-11-28
  // >> All of these requests count towards your personal rate limit of 5,000 requests per hour.

  // https://docs.github.com/en/repositories/viewing-activity-and-data-for-your-repository/viewing-traffic-to-a-repository
  // >> Full clones and visitor information update hourly, while referring sites and popular content
  // sections update daily.

  let runner = JobScheduler::new().await?;
  runner.add(job).await?;
  runner.start().await?;

  Ok(())
}

async fn start_db_healthcheck(state: Arc<AppState>) {
  let interval = std::time::Duration::from_secs(DB_HEALTH_INTERVAL_SECS);
  loop {
    tokio::time::sleep(interval).await;
    state.update_db_health().await;
  }
}

fn sync_health(state: &AppState) -> serde_json::Value {
  let status = state.sync_status();
  let last_success = status.last_success.map(|x| x.to_rfc3339_opts(SecondsFormat::Secs, true));
  serde_json::json!({
    "healthy": is_sync_healthy(status.last_success, state.started_at, Utc::now()),
    "last_success": last_success,
    "last_error": status.last_error,
    "running": status.running,
  })
}

// the status and HTTP code are only about the database: Docker's HEALTHCHECK reads them, and a
// stale sync shouldn't mark the container unhealthy
async fn health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
  let health = state.db_health();
  let sync = sync_health(&state);
  match health.result {
    Ok(_) => {
      let msg = serde_json::json!({ "status": "ok", "sync": sync });
      (StatusCode::OK, axum::response::Json(msg))
    }
    Err(details) => {
      let msg = serde_json::json!({
        "status": "error",
        "error": "database_not_writable",
        "message": DB_NOT_WRITABLE_MESSAGE,
        "details": details,
        "checked_seconds_ago": health.checked_at.elapsed().as_secs(),
        "check_interval_seconds": DB_HEALTH_INTERVAL_SECS,
        "sync": sync,
      });
      (StatusCode::SERVICE_UNAVAILABLE, axum::response::Json(msg))
    }
  }
}

#[tokio::main]
async fn main() -> Res {
  dotenvy::dotenv().ok();
  utils::init_logger();

  let brand = format!("{} v{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
  tracing::info!("{}", brand);

  let router = Router::new()
    .nest("/api", routes::api_routes())
    .merge(routes::badge_routes())
    .merge(routes::card_routes())
    .merge(routes::asset_routes())
    .merge(routes::html_routes())
    .layer(
      TraceLayer::new_for_http()
        .make_span_with(trace::DefaultMakeSpan::new().level(Level::INFO))
        .on_response(trace::DefaultOnResponse::new().level(Level::INFO)),
    )
    .route("/health", get(health)); // do not show logs for this route

  let state = Arc::new(AppState::new().await?);
  let service = router.with_state(state.clone()).into_make_service();

  let health_state = state.clone();
  tokio::spawn(async move {
    start_db_healthcheck(health_state).await;
  });

  let cron_state = state.clone();
  let cron_schedule = metrics_cron_schedule();
  tokio::spawn(async move {
    while let Err(e) = start_cron(cron_state.clone(), &cron_schedule).await {
      tracing::error!("failed to start cron: {:?}", e);
      tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    }
  });

  let host = std::env::var("HOST").unwrap_or("127.0.0.1".to_string());
  let port = std::env::var("PORT").unwrap_or("8080".to_string());
  let addr = format!("{}:{}", host, port);

  let listener = tokio::net::TcpListener::bind(&addr).await?;
  tracing::info!("listening on http://{}", addr);
  axum::serve(listener, service).with_graceful_shutdown(utils::shutdown_signal()).await?;

  Ok(())
}

#[cfg(test)]
mod tests {
  use std::time::{SystemTime, UNIX_EPOCH};

  use axum::body::to_bytes;
  use chrono::Duration;
  use serde_json::{Value, json};

  use super::*;
  use crate::db_client::DbClient;

  async fn test_state(test_name: &str) -> AppState {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let path = std::env::temp_dir()
      .join(format!("ghstats-main-{test_name}-{}-{nanos}.db", std::process::id()));
    let db = DbClient::new(&path.to_string_lossy()).await.unwrap();
    AppState::for_test(db, "")
  }

  async fn get_health(state: &Arc<AppState>) -> (StatusCode, Value) {
    let rep = health(State(state.clone())).await.into_response();
    let code = rep.status();
    let body = to_bytes(rep.into_body(), usize::MAX).await.unwrap();
    (code, serde_json::from_slice(&body).unwrap())
  }

  #[tokio::test]
  async fn health_has_sync() {
    let state = Arc::new(test_state("sync").await);

    let (code, body) = get_health(&state).await;
    let sync =
      json!({ "healthy": true, "last_success": null, "last_error": null, "running": false });
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body, json!({ "status": "ok", "sync": sync }));

    state.record_sync_success();
    state.record_sync_start();
    state.record_sync_error(&anyhow::anyhow!("sync failed"));

    let (_, body) = get_health(&state).await;
    let last_success = body["sync"]["last_success"].as_str().unwrap();
    assert_eq!(body["sync"]["last_error"], "sync failed");
    assert_eq!(body["sync"]["healthy"], true);
    assert!(last_success.ends_with('Z') && last_success.len() == 20, "{last_success}");
  }

  #[tokio::test]
  async fn stale_sync_keeps_health_ok() {
    let mut state = test_state("stale").await;
    state.started_at = Utc::now() - Duration::hours(9);

    let (code, body) = get_health(&Arc::new(state)).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["sync"]["healthy"], false);
  }

  #[tokio::test]
  async fn health_has_sync_on_db_error() {
    let state = Arc::new(test_state("db-error").await);
    state.set_db_health(Err("readonly".into()));

    let (code, body) = get_health(&state).await;
    assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["status"], "error");
    assert_eq!(body["error"], "database_not_writable");
    assert_eq!(body["details"], "readonly");
    assert_eq!(body["sync"]["healthy"], true);
  }
}
