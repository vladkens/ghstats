use std::sync::Arc;

use axum::{response::IntoResponse, routing::get, Router};
use db_client::RepoFilter;
use reqwest::StatusCode;
use state::AppState;
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

async fn check_new_release(state: Arc<AppState>) -> Res {
  let tag = state.gh.get_latest_release_ver("vladkens/ghstats").await?;
  let mut last_tag = state.last_release.lock().unwrap();
  if *last_tag != tag {
    tracing::info!("new release available: {} -> {}", *last_tag, tag);
    *last_tag = tag.clone();
  }

  Ok(())
}

async fn start_cron(state: Arc<AppState>) -> Res {
  use tokio_cron_scheduler::{Job, JobScheduler};

  // note: for development, uncomment to update metrics on start
  helpers::update_metrics(state.clone()).await?;

  // if new db, update metrics immediately
  let repos = state.db.get_repos(&RepoFilter::default()).await?;
  if repos.len() == 0 {
    tracing::info!("no repos found, load initial metrics");
    match helpers::update_metrics(state.clone()).await {
      Err(e) => tracing::error!("failed to update metrics: {:?}", e),
      Ok(_) => {}
    }
  } else {
    state.db.update_deltas().await?;
  }

  // https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api?apiVersion=2022-11-28
  // >> All of these requests count towards your personal rate limit of 5,000 requests per hour.

  // https://docs.github.com/en/repositories/viewing-activity-and-data-for-your-repository/viewing-traffic-to-a-repository
  // >> Full clones and visitor information update hourly, while referring sites and popular content sections update daily.

  // last minute of every hour
  let job = Job::new_async("0 59 * * * *", move |_, _| {
    let state = state.clone();
    Box::pin(async move {
      let _ = check_new_release(state.clone()).await;

      match helpers::update_metrics(state.clone()).await {
        Err(e) => tracing::error!("failed to update metrics: {:?}", e),
        Ok(_) => {}
      }
    })
  })?;

  let runner = JobScheduler::new().await?;
  runner.start().await?;
  runner.add(job).await?;

  Ok(())
}

async fn health() -> impl IntoResponse {
  let msg = serde_json::json!({ "status": "ok" });
  (StatusCode::OK, axum::response::Json(msg))
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

  let cron_state = state.clone();
  tokio::spawn(async move {
    loop {
      match start_cron(cron_state.clone()).await {
        Err(e) => {
          tracing::error!("failed to start cron: {:?}", e);
          tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
        Ok(_) => break,
      }
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
