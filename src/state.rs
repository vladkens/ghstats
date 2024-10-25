use std::sync::Mutex;

use crate::{
  db_client::{DbClient, RepoFilter, RepoTotals},
  gh_client::GhClient,
  helpers::GhsFilter,
  types::Res,
};

fn env_bool(key: &str) -> bool {
  let val = std::env::var(key).unwrap_or_else(|_| "false".to_string()).to_lowercase();
  return val == "true" || val == "1";
}

pub struct AppState {
  pub db: DbClient,
  pub gh: GhClient,
  pub filter: GhsFilter,
  pub include_private: bool,
  pub last_release: Mutex<String>,
}

impl AppState {
  pub async fn new() -> Res<Self> {
    let gh_token = std::env::var("GITHUB_TOKEN").unwrap_or_default();
    if gh_token.is_empty() {
      tracing::error!("missing GITHUB_TOKEN");
      std::process::exit(1);
    }

    let db_path = std::env::var("DB_PATH").unwrap_or("./data/ghstats.db".to_string());
    tracing::info!("db_path: {}", db_path);

    let db = DbClient::new(&db_path).await?;
    let gh = GhClient::new(gh_token)?;

    let filter = std::env::var("GHS_FILTER").unwrap_or_default();
    let filter = GhsFilter::new(&filter);
    tracing::info!("{:?}", filter);

    let include_private = env_bool("GHS_INCLUDE_PRIVATE");

    let last_release = Mutex::new(env!("CARGO_PKG_VERSION").to_string());
    Ok(Self { db, gh, filter, include_private, last_release })
  }

  pub async fn get_repos_filtered(&self, qs: &RepoFilter) -> Res<Vec<RepoTotals>> {
    let repos = self.db.get_repos(&qs).await?;
    let repos = repos.into_iter().filter(|x| self.filter.is_included(&x.name, x.fork, x.archived));
    let repos = repos.collect::<Vec<_>>();
    Ok(repos)
  }
}
