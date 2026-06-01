use std::sync::Arc;

use axum::{Router, extract::State, response::IntoResponse, routing::get};
use db_client::RepoFilter;
use reqwest::StatusCode;
use state::{AppState, DB_HEALTH_INTERVAL_SECS, DB_NOT_WRITABLE_MESSAGE};
use tower_http::trace::{self, TraceLayer};
use tracing::Level;
use types::Res;

mod db_client;
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
    tracing::info!("new release available: {} -> {}", *last_tag, tag);
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
      let _ = check_new_release(state.clone()).await;

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
  // >> Full clones and visitor information update hourly, while referring sites and popular content sections update daily.

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

async fn health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
  let health = state.db_health();
  match health.result {
    Ok(_) => {
      let msg = serde_json::json!({ "status": "ok" });
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
