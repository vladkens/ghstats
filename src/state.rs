use std::sync::Mutex;
use std::time::Instant;

use chrono::{DateTime, Utc};

use crate::db_client::{DbClient, RepoFilter, RepoTotals};
use crate::gh_client::GhClient;
use crate::helpers::GhsFilter;
use crate::types::Res;

pub const DB_HEALTH_INTERVAL_SECS: u64 = 60 * 60;
pub const DB_NOT_WRITABLE_MESSAGE: &str = "SQLite database is not writable. ghstats Docker images run as a non-root user, so existing bind-mounted data directories may need an ownership fix. For docker run: docker run --rm -u root -v ./data:/app/data ghcr.io/vladkens/ghstats:latest chown -R appuser:appgroup /app/data. For Docker Compose: docker compose run --rm --user root ghstats chown -R appuser:appgroup /app/data";

fn env_bool(key: &str) -> bool {
  let val = std::env::var(key).unwrap_or_else(|_| "false".to_string()).to_lowercase();
  val == "true" || val == "1"
}

#[derive(Clone)]
pub struct DbHealth {
  pub checked_at: Instant,
  pub result: Result<(), String>,
}

#[derive(Clone, Default)]
pub struct SyncStatus {
  pub last_success: Option<DateTime<Utc>>,
  pub last_error: Option<String>,
}

impl DbHealth {
  fn new(result: Result<(), String>) -> Self {
    Self { checked_at: Instant::now(), result }
  }
}

pub struct AppState {
  pub db: DbClient,
  pub gh: GhClient,
  pub filter: GhsFilter,
  pub include_private: bool,
  pub last_release: Mutex<String>,
  db_health: Mutex<DbHealth>,
  sync_status: Mutex<SyncStatus>,
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
    if let Err(e) = db.check_writable().await {
      let details = e.to_string();
      tracing::error!("{}: {}", DB_NOT_WRITABLE_MESSAGE, details);
      anyhow::bail!("{}: {}", DB_NOT_WRITABLE_MESSAGE, details);
    }

    let gh = GhClient::new(gh_token)?;

    let filter = std::env::var("GHS_FILTER").unwrap_or_default();
    let filter = GhsFilter::new(&filter);
    tracing::info!("{:?}", filter);

    let include_private = env_bool("GHS_INCLUDE_PRIVATE");

    let last_release = Mutex::new(env!("CARGO_PKG_VERSION").to_string());
    let db_health = Mutex::new(DbHealth::new(Ok(())));
    let sync_status = Mutex::new(SyncStatus::default());
    Ok(Self { db, gh, filter, include_private, last_release, db_health, sync_status })
  }

  pub fn db_health(&self) -> DbHealth {
    self.db_health.lock().unwrap().clone()
  }

  pub async fn update_db_health(&self) {
    let result = self.db.check_writable().await.map_err(|e| e.to_string());
    if let Err(e) = &result {
      tracing::error!("{}: {}", DB_NOT_WRITABLE_MESSAGE, e);
    }

    *self.db_health.lock().unwrap() = DbHealth::new(result);
  }

  pub fn sync_status(&self) -> SyncStatus {
    self.sync_status.lock().unwrap().clone()
  }

  pub fn record_sync_success(&self) {
    *self.sync_status.lock().unwrap() =
      SyncStatus { last_success: Some(Utc::now()), last_error: None };
  }

  pub fn record_sync_error(&self, error: &anyhow::Error) {
    self.sync_status.lock().unwrap().last_error = Some(error.to_string());
  }

  pub async fn get_repos_filtered(&self, qs: &RepoFilter) -> Res<Vec<RepoTotals>> {
    let repos = self.db.get_repos(qs).await?;
    let repos = repos.into_iter().filter(|x| self.filter.is_included(&x.name, x.fork, x.archived));

    let repos: Vec<RepoTotals> = match &qs.q {
      Some(q) if !q.is_empty() => {
        let q = q.to_lowercase();
        repos.filter(|x| x.name.to_lowercase().contains(&q)).collect()
      }
      _ => repos.collect(),
    };

    let repos: Vec<RepoTotals> = match &qs.owner {
      Some(owner) if !owner.is_empty() => {
        repos.into_iter().filter(|x| x.name.split('/').next().is_some_and(|o| o == owner)).collect()
      }
      _ => repos,
    };

    Ok(repos)
  }

  pub async fn get_owners(&self) -> Res<Vec<String>> {
    let filter = RepoFilter::default();
    let repos = self.db.get_repos(&filter).await?;
    let mut owners: Vec<String> = repos
      .iter()
      .filter(|x| self.filter.is_included(&x.name, x.fork, x.archived))
      .filter_map(|x| x.name.split('/').next().map(|s| s.to_string()))
      .collect();
    owners.sort();
    owners.dedup();
    Ok(owners)
  }

  #[cfg(test)]
  pub(crate) fn for_test(db: DbClient, filter: GhsFilter) -> Self {
    Self {
      db,
      gh: GhClient::new("test-token".to_string()).unwrap(),
      filter,
      include_private: false,
      last_release: Mutex::new(env!("CARGO_PKG_VERSION").to_string()),
      db_health: Mutex::new(DbHealth::new(Ok(()))),
      sync_status: Mutex::new(SyncStatus::default()),
    }
  }
}
