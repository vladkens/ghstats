use std::sync::Arc;

use axum::routing::get;
use axum::{http::StatusCode, response::IntoResponse, Router};

mod db_client;
mod gh_client;
mod pages;
mod utils;

use db_client::DbClient;
use gh_client::GhClient;
use utils::Res;

struct AppState {
  db: DbClient,
  gh: GhClient,
}

impl AppState {
  async fn new() -> Res<Self> {
    let gh_token = match std::env::var("GITHUB_TOKEN") {
      Ok(token) => token,
      Err(err) => {
        tracing::error!("missing GITHUB_TOKEN: {:?}", err);
        std::process::exit(1);
      }
    };

    let db_path = std::env::var("DB_PATH").unwrap_or("./data/ghstats.db".to_string());
    tracing::info!("db_path: {}", db_path);

    let db = DbClient::new(&db_path).await?;
    let gh = GhClient::new(gh_token)?;
    Ok(Self { db, gh })
  }
}

async fn health() -> impl IntoResponse {
  let msg = serde_json::json!({ "status": "ok" });
  (StatusCode::OK, axum::response::Json(msg))
}

async fn update_metrics(db: &DbClient, gh: &GhClient) -> Res {
  let stime = std::time::Instant::now();

  let date = chrono::Utc::now().to_utc().to_rfc3339();
  let date = date.split("T").next().unwrap().to_owned() + "T00:00:00Z";

  let repos = gh.get_repos().await?;
  for repo in repos {
    let views = gh.traffic_views(&repo.full_name).await?;
    let clones = gh.traffic_clones(&repo.full_name).await?;
    let referrers = gh.traffic_refs(&repo.full_name).await?;
    let popular_paths = gh.traffic_paths(&repo.full_name).await?;

    db.insert_stats(&repo, &date).await?;
    db.insert_views(&repo, &views).await?;
    db.insert_clones(&repo, &clones).await?;
    db.insert_referrers(&repo, &date, &referrers).await?;
    db.insert_paths(&repo, &date, &popular_paths).await?;
  }

  tracing::info!("update_metrics took {:?}", stime.elapsed());
  db.update_deltas().await?;

  Ok(())
}

async fn start_cron(state: Arc<AppState>) -> Res {
  use tokio_cron_scheduler::{Job, JobScheduler};

  state.db.update_deltas().await?;

  // if new db, update metrics immediately
  let repos = state.db.get_repos().await?;
  if repos.len() == 0 {
    match update_metrics(&state.db, &state.gh).await {
      Err(e) => tracing::error!("error updating metrics: {:?}", e),
      Ok(_) => {}
    }
  }

  // https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api?apiVersion=2022-11-28
  // >> All of these requests count towards your personal rate limit of 5,000 requests per hour.

  // https://docs.github.com/en/repositories/viewing-activity-and-data-for-your-repository/viewing-traffic-to-a-repository
  // >> Full clones and visitor information update hourly, while referring sites and popular content sections update daily.

  // last minute of every hour
  let job = Job::new_async("0 59 * * * *", move |_, _| {
    let state = state.clone();
    Box::pin(async move {
      match update_metrics(&state.db, &state.gh).await {
        Err(e) => tracing::error!("error updating metrics: {:?}", e),
        Ok(_) => {}
      }
    })
  })?;

  let runner = JobScheduler::new().await?;
  runner.start().await?;
  runner.add(job).await?;

  Ok(())
}

#[tokio::main]
async fn main() -> Res {
  use crate::pages;
  use tower_http::trace::{self, TraceLayer};
  use tracing::Level;

  dotenvy::dotenv().ok();
  tracing_subscriber::fmt().with_target(false).compact().init();

  let router = Router::new()
    .route("/health", get(health))
    .route("/", get(pages::index))
    .route("/:user/:name", get(pages::repo_page));

  let router = router.layer(
    TraceLayer::new_for_http()
      .make_span_with(trace::DefaultMakeSpan::new().level(Level::INFO))
      .on_response(trace::DefaultOnResponse::new().level(Level::INFO)),
  );

  let state = Arc::new(AppState::new().await?);
  let service = router.with_state(state.clone()).into_make_service();
  start_cron(state.clone()).await?;

  let addr = "0.0.0.0:8080";
  let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
  tracing::info!("listening on {}", addr);
  axum::serve(listener, service).await.unwrap();

  Ok(())
}
