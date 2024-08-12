use std::sync::{Arc, Mutex};

use axum::routing::get;
use axum::{http::StatusCode, response::IntoResponse, Router};

mod db_client;
mod gh_client;
mod helpers;
mod pages;
mod types;

use db_client::{DbClient, RepoFilter};
use gh_client::GhClient;
use types::Res;

struct AppState {
  db: DbClient,
  gh: GhClient,
  last_release: Mutex<String>,
}

impl AppState {
  async fn new() -> Res<Self> {
    let gh_token = std::env::var("GITHUB_TOKEN").unwrap_or_default();
    if gh_token.is_empty() {
      tracing::error!("missing GITHUB_TOKEN");
      std::process::exit(1);
    }

    let db_path = std::env::var("DB_PATH").unwrap_or("./data/ghstats.db".to_string());
    tracing::info!("db_path: {}", db_path);

    let db = DbClient::new(&db_path).await?;
    let gh = GhClient::new(gh_token)?;

    let last_release = Mutex::new(env!("CARGO_PKG_VERSION").to_string());
    Ok(Self { db, gh, last_release })
  }
}

async fn update_metrics(db: &DbClient, gh: &GhClient) -> Res {
  let stime = std::time::Instant::now();

  let date = chrono::Utc::now().to_utc().to_rfc3339();
  let date = date.split("T").next().unwrap().to_owned() + "T00:00:00Z";

  let repos = gh.get_repos().await?;
  for repo in &repos {
    match helpers::update_repo_metrics(db, gh, &repo, &date).await {
      Err(e) => {
        tracing::warn!("failed to update metrics for {}: {:?}", repo.full_name, e);
        continue;
      }
      // Ok(_) => tracing::info!("updated metrics for {}", repo.full_name),
      Ok(_) => {}
    }
  }

  tracing::info!("update_metrics took {:?} for {} repos", stime.elapsed(), repos.len());
  db.update_deltas().await?;

  Ok(())
}

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

  // if new db, update metrics immediately
  let repos = state.db.get_repos(&RepoFilter::default()).await?;
  if repos.len() == 0 {
    tracing::info!("no repos found, load initial metrics");
    match update_metrics(&state.db, &state.gh).await {
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

      match update_metrics(&state.db, &state.gh).await {
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
  use crate::pages;
  use tower_http::trace::{self, TraceLayer};
  use tracing::Level;

  dotenvy::dotenv().ok();
  tracing_subscriber::fmt() //
    .with_target(false)
    .compact()
    // .with_max_level(Level::TRACE)
    .init();

  let brand = format!("{} v{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
  tracing::info!("{}", brand);

  let router = Router::new()
    .route("/health", get(health))
    .route("/", get(pages::index))
    .route("/:owner/:repo", get(pages::repo_page));

  let router = router.layer(
    TraceLayer::new_for_http()
      .make_span_with(trace::DefaultMakeSpan::new().level(Level::INFO))
      .on_response(trace::DefaultOnResponse::new().level(Level::INFO)),
  );

  let state = Arc::new(AppState::new().await?);
  let service = router.with_state(state.clone()).into_make_service();
  start_cron(state.clone()).await?;

  let host = std::env::var("HOST").unwrap_or("127.0.0.1".to_string());
  let port = std::env::var("PORT").unwrap_or("8080".to_string());
  let addr = format!("{}:{}", host, port);

  let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
  tracing::info!("listening on {}", addr);
  axum::serve(listener, service).await.unwrap();

  Ok(())
}
