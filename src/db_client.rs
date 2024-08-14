use std::future::Future;
use std::pin::Pin;

use anyhow::Ok;
use serde::{Deserialize, Serialize};
use serde_variant::to_variant_name;
use sqlx::{sqlite::SqliteConnectOptions, FromRow, SqlitePool};

use crate::gh_client::{Repo, RepoClones, RepoPopularPath, RepoReferrer, RepoViews};
use crate::types::Res;

// MARK: Migrations

async fn migrate_v1(db: &SqlitePool) -> Res {
  let mut queries = vec![];

  let qs = "CREATE TABLE IF NOT EXISTS repos (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT DEFAULT NULL,
    archived BOOLEAN DEFAULT FALSE
  );";
  queries.push(qs);

  let qs = "CREATE TABLE IF NOT EXISTS repo_stats (
    repo_id INTEGER NOT NULL,
    date TEXT NOT NULL,
    stars INTEGER NOT NULL DEFAULT 0,
    forks INTEGER NOT NULL DEFAULT 0,
    watchers INTEGER NOT NULL DEFAULT 0,
    issues INTEGER NOT NULL DEFAULT 0,
    clones_count INTEGER NOT NULL DEFAULT 0,
    clones_uniques INTEGER NOT NULL DEFAULT 0,
    views_count INTEGER NOT NULL DEFAULT 0,
    views_uniques INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (repo_id, date)
    -- FOREIGN KEY (repo_id) REFERENCES repos(id)
  );";
  queries.push(qs);

  let qs = "CREATE TABLE IF NOT EXISTS repo_referrers (
    repo_id INTEGER NOT NULL,
    date TEXT NOT NULL,
    referrer TEXT NOT NULL,
    count INTEGER NOT NULL DEFAULT 0,
    uniques INTEGER NOT NULL DEFAULT 0,
    count_delta INTEGER NOT NULL DEFAULT 0,
    uniques_delta INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (repo_id, date, referrer)
  );";
  queries.push(qs);

  let qs = "
  CREATE TABLE IF NOT EXISTS repo_popular_paths (
    repo_id INTEGER NOT NULL,
    date TEXT NOT NULL,
    path TEXT NOT NULL,
    title TEXT NOT NULL,
    count INTEGER NOT NULL DEFAULT 0,
    uniques INTEGER NOT NULL DEFAULT 0,
    count_delta INTEGER NOT NULL DEFAULT 0,
    uniques_delta INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (repo_id, date, path)
  );";
  queries.push(qs);

  for qs in queries {
    sqlx::query(qs).execute(db).await?;
  }

  Ok(())
}

async fn migrate_v2(db: &SqlitePool) -> Res {
  let qs = "ALTER TABLE repos ADD COLUMN stars_synced BOOLEAN DEFAULT FALSE;";
  sqlx::query(qs).execute(db).await?;
  Ok(())
}

async fn migrate<'a>(db: &'a SqlitePool) -> Res {
  type BoxFn = Box<dyn for<'a> Fn(&'a SqlitePool) -> Pin<Box<dyn Future<Output = Res> + 'a>>>;
  let migrations: Vec<BoxFn> = vec![
    Box::new(|db| Box::pin(migrate_v1(db))),
    // Box::new(|db| Box::pin(migrate_v2(db))),
  ];

  let version: (i32,) = sqlx::query_as("PRAGMA user_version").fetch_one(db).await?;
  let version = version.0;

  for (idx, func) in migrations.iter().enumerate() {
    let mig_ver = idx as i32 + 1;
    if version < mig_ver {
      tracing::info!("running migration to v{}", mig_ver);
      let _ = func(db).await?;
      let qs = format!("PRAGMA user_version = {}", mig_ver);
      sqlx::raw_sql(&qs).execute(db).await?;
    }
  }

  Ok(())
}

pub async fn get_db(db_path: &str) -> Res<SqlitePool> {
  let opts = SqliteConnectOptions::new().filename(db_path).create_if_missing(true);
  let pool = SqlitePool::connect_with(opts).await?;
  migrate(&pool).await?;
  Ok(pool)
}

// MARK: Models

#[derive(Clone, Debug, Serialize, Deserialize, FromRow)]
pub struct RepoTotals {
  pub id: i64,
  pub name: String,
  pub description: Option<String>,
  pub date: String,
  pub stars: i32,
  pub forks: i32,
  pub watchers: i32,
  pub issues: i32,
  pub clones_count: i32,
  pub clones_uniques: i32,
  pub views_count: i32,
  pub views_uniques: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize, FromRow)]
pub struct RepoMetrics {
  pub date: String,
  pub clones_count: i32,
  pub clones_uniques: i32,
  pub views_count: i32,
  pub views_uniques: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize, FromRow)]
pub struct RepoStars {
  pub date: String,
  pub stars: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize, FromRow)]
pub struct RepoPopularItem {
  pub name: String,
  pub count: i64,
  pub uniques: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, FromRow)]
pub struct RepoItem {
  pub id: i64,
  pub name: String,
  pub archived: bool,
  pub stars_synced: bool,
}

// MARK: Filters

pub enum PopularKind {
  Refs,
  Path,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
  Asc,
  Desc,
}

impl Default for Direction {
  fn default() -> Self {
    Direction::Desc
  }
}

impl std::fmt::Display for Direction {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "{}", to_variant_name(self).unwrap())
  }
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RepoSort {
  Name,
  Stars,
  Forks,
  Watchers,
  Issues,
  #[serde(rename = "clones_count")]
  Clones,
  #[serde(rename = "views_count")]
  Views,
}

impl Default for RepoSort {
  fn default() -> Self {
    RepoSort::Views
  }
}

impl std::fmt::Display for RepoSort {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "{}", to_variant_name(self).unwrap())
  }
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PopularSort {
  Name,
  Count,
  Uniques,
}

impl Default for PopularSort {
  fn default() -> Self {
    PopularSort::Uniques
  }
}

impl std::fmt::Display for PopularSort {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "{}", to_variant_name(self).unwrap())
  }
}

#[derive(Debug, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct RepoFilter {
  pub sort: RepoSort,
  pub direction: Direction,
}

#[derive(Debug, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct PopularFilter {
  pub sort: PopularSort,
  pub direction: Direction,
  pub period: i32,
}

// MARK: DbClient

const TOTAL_QUERY: &'static str = "
SELECT * FROM repos r
INNER JOIN (
	SELECT
		rs.repo_id,
		SUM(clones_count) AS clones_count, SUM(clones_uniques) AS clones_uniques,
    SUM(views_count) AS views_count, SUM(views_uniques) AS views_uniques,
    latest.*
	FROM repo_stats rs
	INNER JOIN (
		SELECT repo_id, MAX(date) AS date, stars, forks, watchers, issues
		FROM repo_stats GROUP BY repo_id
	) latest ON latest.repo_id = rs.repo_id
	GROUP BY rs.repo_id
) rs ON rs.repo_id = r.id
";

pub struct DbClient {
  db: SqlitePool,
}

impl DbClient {
  pub async fn new(db_path: &str) -> Res<Self> {
    let db = get_db(db_path).await?;
    Ok(Self { db })
  }

  // MARK: Getters

  pub async fn get_repo_totals(&self, repo: &str) -> Res<Option<RepoTotals>> {
    let qs = format!("{} WHERE r.name = $1;", TOTAL_QUERY);
    let item = sqlx::query_as(qs.as_str()).bind(repo).fetch_optional(&self.db).await?;
    Ok(item)
  }

  pub async fn get_metrics(&self, name: &str) -> Res<Vec<RepoMetrics>> {
    let qs = "
    SELECT * FROM repo_stats rs
    INNER JOIN repos r ON r.id = rs.repo_id
    WHERE r.name = $1 AND (rs.clones_count > 0 OR rs.views_count > 0)
    ORDER BY rs.date ASC;
    ";

    let items = sqlx::query_as(qs).bind(name).fetch_all(&self.db).await?;
    Ok(items)
  }

  pub async fn get_repos(&self, filter: &RepoFilter) -> Res<Vec<RepoTotals>> {
    let qs = format!("{} ORDER BY {} {}", TOTAL_QUERY, filter.sort, filter.direction);
    let items = sqlx::query_as(qs.as_str()).fetch_all(&self.db).await?;
    Ok(items)
  }

  pub async fn get_stars(&self, repo: &str) -> Res<Vec<RepoStars>> {
    let qs = "
    SELECT date, stars FROM repo_stats rs
    INNER JOIN repos r ON r.id = rs.repo_id
    WHERE r.name = $1
    ORDER BY rs.date ASC;
    ";

    let mut items: Vec<RepoStars> = sqlx::query_as(qs).bind(repo).fetch_all(&self.db).await?;

    // restore gaps in data
    let mut prev_stars = 0;
    for (idx, item) in items.iter_mut().enumerate() {
      if idx == 0 {
        continue;
      }

      if item.stars == 0 {
        item.stars = prev_stars;
      }

      prev_stars = item.stars;
    }

    // in case when data start to be collected for exist repo with some stats
    // view and clone stats can be collected without stars, so remove them
    let items = items.into_iter().filter(|x| x.stars > 0).collect();
    Ok(items)
  }

  pub async fn get_popular_items(
    &self,
    repo: &str,
    kind: &PopularKind,
    filter: &PopularFilter,
  ) -> Res<Vec<RepoPopularItem>> {
    let (table, col) = match kind {
      PopularKind::Refs => ("repo_referrers", "referrer"),
      PopularKind::Path => ("repo_popular_paths", "path"),
    };

    let time_where = match filter.period {
      x if x > 0 => format!("date >= date('now', '-{} day')", x),
      _ => "1=1".to_string(),
    };

    let order_by = format!("{} {}", filter.sort, filter.direction);

    #[rustfmt::skip]
    let qs = format!("
    SELECT {col} as name, SUM(count_delta) AS count, SUM(uniques_delta) AS uniques
    FROM {table} rr
    INNER JOIN repos r ON r.id = rr.repo_id
    WHERE r.name = $1 AND {time_where}
    GROUP BY rr.{col}
    ORDER BY {order_by};
    ");

    let items = sqlx::query_as(&qs).bind(repo).fetch_all(&self.db).await?;
    Ok(items)
  }

  pub async fn repos_to_sync(&self) -> Res<Vec<RepoItem>> {
    let qs = "SELECT * FROM repos WHERE stars_synced = FALSE;";
    let items = sqlx::query_as(qs).fetch_all(&self.db).await?;
    Ok(items)
  }

  // MARK: Inserters

  pub async fn insert_repo(&self, repo: &Repo) -> Res {
    let qs = "
    INSERT INTO repos (id, name, description, archived)
    VALUES ($1, $2, $3, $4)
    ON CONFLICT(id) DO UPDATE SET
      name = excluded.name,
      description = excluded.description,
      archived = excluded.archived;
    ";

    let _ = sqlx::query(qs)
      .bind(repo.id as i64)
      .bind(&repo.full_name)
      .bind(&repo.description)
      .bind(repo.archived)
      .execute(&self.db)
      .await?;

    Ok(())
  }

  pub async fn insert_stats(&self, repo: &Repo, date: &str) -> Res {
    let _ = self.insert_repo(repo).await?;

    let qs = "
    INSERT INTO repo_stats AS t (repo_id, date, stars, forks, watchers, issues)
    VALUES ($1, $2, $3, $4, $5, $6)
    ON CONFLICT(repo_id, date) DO UPDATE SET
      stars = MAX(t.stars, excluded.stars),
      forks = MAX(t.forks, excluded.forks),
      watchers = MAX(t.watchers, excluded.watchers),
      issues = MAX(t.issues, excluded.issues);
    ";

    let _ = sqlx::query(qs)
      .bind(repo.id as i64)
      .bind(&date)
      .bind(repo.stargazers_count as i32)
      .bind(repo.forks_count as i32)
      .bind(repo.watchers_count as i32)
      .bind(repo.open_issues_count as i32)
      .execute(&self.db)
      .await?;

    Ok(())
  }

  pub async fn insert_stars(&self, repo: &str, stars: &Vec<(String, u32)>) -> Res {
    let qs = "
    INSERT INTO repo_stats AS t (repo_id, date, stars)
    VALUES ((SELECT id FROM repos WHERE name = $1), $2, $3)
    ON CONFLICT(repo_id, date) DO UPDATE SET
      stars = MAX(t.stars, excluded.stars);
    ";

    for (date, count) in stars {
      let _ =
        sqlx::query(qs).bind(repo).bind(&date).bind(count.clone() as i32).execute(&self.db).await?;
    }

    Ok(())
  }

  pub async fn insert_clones(&self, repo: &Repo, clones: &RepoClones) -> Res {
    let qs = "
    INSERT INTO repo_stats AS t (repo_id, date, clones_count, clones_uniques)
    VALUES ($1, $2, $3, $4)
    ON CONFLICT(repo_id, date) DO UPDATE SET
      clones_count = MAX(t.clones_count, excluded.clones_count),
      clones_uniques = MAX(t.clones_uniques, excluded.clones_uniques);
    ";

    for doc in &clones.clones {
      let _ = sqlx::query(qs)
        .bind(repo.id as i64)
        .bind(&doc.timestamp)
        .bind(doc.count as i32)
        .bind(doc.uniques as i32)
        .execute(&self.db)
        .await?;
    }

    Ok(())
  }

  pub async fn insert_views(&self, repo: &Repo, views: &RepoViews) -> Res {
    let qs = "
    INSERT INTO repo_stats AS t (repo_id, date, views_count, views_uniques)
    VALUES ($1, $2, $3, $4)
    ON CONFLICT(repo_id, date) DO UPDATE SET
      views_count = MAX(t.views_count, excluded.views_count),
      views_uniques = MAX(t.views_uniques, excluded.views_uniques);
    ";

    for doc in &views.views {
      let _ = sqlx::query(qs)
        .bind(repo.id as i64)
        .bind(&doc.timestamp)
        .bind(doc.count as i32)
        .bind(doc.uniques as i32)
        .execute(&self.db)
        .await?;
    }

    Ok(())
  }

  pub async fn insert_referrers(&self, repo: &Repo, date: &str, docs: &Vec<RepoReferrer>) -> Res {
    let qs = "
    INSERT INTO repo_referrers AS t (repo_id, date, referrer, count, uniques)
    VALUES ($1, $2, $3, $4, $5)
    ON CONFLICT(repo_id, date, referrer) DO UPDATE SET
      count = MAX(t.count, excluded.count),
      uniques = MAX(t.uniques, excluded.uniques);
    ";

    for rec in docs {
      let _ = sqlx::query(qs)
        .bind(repo.id as i64)
        .bind(&date)
        .bind(&rec.referrer)
        .bind(rec.count as i32)
        .bind(rec.uniques as i32)
        .execute(&self.db)
        .await?;
    }

    Ok(())
  }

  pub async fn insert_paths(&self, repo: &Repo, date: &str, docs: &Vec<RepoPopularPath>) -> Res {
    let qs = "
    INSERT INTO repo_popular_paths AS t (repo_id, date, path, title, count, uniques)
    VALUES ($1, $2, $3, $4, $5, $6)
    ON CONFLICT(repo_id, date, path) DO UPDATE SET
      count = MAX(t.count, excluded.count),
      uniques = MAX(t.uniques, excluded.uniques);
    ";

    for rec in docs {
      let _ = sqlx::query(qs)
        .bind(repo.id as i64)
        .bind(&date)
        .bind(&rec.path)
        .bind(&rec.title)
        .bind(rec.count as i32)
        .bind(rec.uniques as i32)
        .execute(&self.db)
        .await?;
    }

    Ok(())
  }

  // MARK: Updater

  pub async fn update_deltas(&self) -> Res {
    let stime = std::time::Instant::now();
    let items = [("repo_referrers", "referrer"), ("repo_popular_paths", "path")];

    for (table, col) in items {
      #[rustfmt::skip]
      let qs = format!("
      WITH cte AS (
      SELECT
        rr.repo_id, rr.date, rr.{col}, rr.uniques, rr.count,
        LAG(rr.uniques) OVER (PARTITION BY rr.repo_id, rr.{col} ORDER BY rr.date) AS prev_uniques,
        LAG(rr.count) OVER (PARTITION BY rr.repo_id, rr.{col} ORDER BY rr.date) AS prev_count
      FROM {table} rr
      )
      UPDATE {table} AS rr	SET
        uniques_delta = MAX(0, cte.uniques - COALESCE(cte.prev_uniques, 0)),
        count_delta = MAX(0, cte.count - COALESCE(cte.prev_count, 0))
      FROM cte
      WHERE rr.repo_id = cte.repo_id AND rr.date = cte.date AND rr.{col} = cte.{col};
      ");

      let _ = sqlx::query(qs.as_str()).execute(&self.db).await?;
    }

    tracing::info!("update_deltas took {:?}", stime.elapsed());
    Ok(())
  }
}
