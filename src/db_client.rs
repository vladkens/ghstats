use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use serde_variant::to_variant_name;

use crate::gh_client::{PullRequest, Repo, RepoClones, RepoPopularPath, RepoReferrer, RepoViews};
use crate::types::Res;

// MARK: Migrations

fn migrate_v1(db: &Connection) -> Res {
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
    db.execute(qs, [])?;
  }

  Ok(())
}

fn migrate_v2(db: &Connection) -> Res {
  let queries = vec![
    "ALTER TABLE repos ADD COLUMN stars_synced BOOLEAN DEFAULT FALSE;",
    "ALTER TABLE repos ADD COLUMN fork BOOLEAN DEFAULT FALSE;",
    // can be deleted or marked as private or user removed from org
    // keep in db – but hide from UI and updates
    "ALTER TABLE repos ADD COLUMN hidden BOOLEAN DEFAULT FALSE;",
  ];

  for qs in queries {
    db.execute(qs, [])?;
  }

  Ok(())
}

fn migrate_v3(db: &Connection) -> Res {
  let queries = vec!["ALTER TABLE repo_stats ADD COLUMN prs INTEGER NOT NULL DEFAULT 0;"];

  for qs in queries {
    db.execute(qs, [])?;
  }

  Ok(())
}

fn migrate(db: &Connection) -> Res {
  let migrations: Vec<fn(&Connection) -> Res> = vec![migrate_v1, migrate_v2, migrate_v3];
  let version: i32 = db.query_row("PRAGMA user_version", [], |row| row.get(0))?;

  for (idx, func) in migrations.iter().enumerate() {
    let mig_ver = idx as i32 + 1;
    if version < mig_ver {
      tracing::info!("running migration to v{}", mig_ver);
      func(db)?;
      let qs = format!("PRAGMA user_version = {}", mig_ver);
      db.execute_batch(&qs)?;
    }
  }

  Ok(())
}

pub async fn get_db(db_path: &str) -> Res<Connection> {
  let db = Connection::open(db_path)?;
  migrate(&db)?;
  Ok(db)
}

// MARK: Models

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RepoTotals {
  pub id: i64,
  pub name: String,
  pub description: Option<String>,
  pub fork: bool,
  pub archived: bool,
  pub date: String,
  pub stars: i32,
  pub forks: i32,
  pub watchers: i32,
  pub issues: i32,
  pub prs: i32,
  pub clones_count: i32,
  pub clones_uniques: i32,
  pub views_count: i32,
  pub views_uniques: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RepoMetrics {
  pub date: String,
  pub clones_count: i32,
  pub clones_uniques: i32,
  pub views_count: i32,
  pub views_uniques: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RepoStars {
  pub date: String,
  pub stars: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RepoPopularItem {
  pub name: String,
  pub count: i64,
  pub uniques: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
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

#[derive(Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
  Asc,
  #[default]
  Desc,
}

impl std::fmt::Display for Direction {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "{}", to_variant_name(self).unwrap())
  }
}

#[derive(Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RepoSort {
  Name,
  Stars,
  Forks,
  Watchers,
  Issues,
  Prs,
  #[serde(rename = "clones_count")]
  Clones,
  #[serde(rename = "views_count")]
  #[default]
  Views,
}

impl std::fmt::Display for RepoSort {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "{}", to_variant_name(self).unwrap())
  }
}

#[derive(Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PopularSort {
  Name,
  Count,
  #[default]
  Uniques,
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
  pub q: Option<String>,
  pub owner: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct PopularFilter {
  pub sort: PopularSort,
  pub direction: Direction,
  pub period: i32,
}

// MARK: DbClient

const TOTAL_QUERY: &str = "
SELECT * FROM repos r
INNER JOIN (
	SELECT
		rs.repo_id,
		SUM(clones_count) AS clones_count, SUM(clones_uniques) AS clones_uniques,
    SUM(views_count) AS views_count, SUM(views_uniques) AS views_uniques,
    latest.*
	FROM repo_stats rs
	INNER JOIN (
		SELECT repo_id, MAX(date) AS date, stars, forks, watchers, issues, prs
		FROM repo_stats GROUP BY repo_id
	) latest ON latest.repo_id = rs.repo_id
	GROUP BY rs.repo_id
) rs ON rs.repo_id = r.id
";

fn repo_totals_from_row(row: &Row<'_>) -> rusqlite::Result<RepoTotals> {
  Ok(RepoTotals {
    id: row.get("id")?,
    name: row.get("name")?,
    description: row.get("description")?,
    fork: row.get("fork")?,
    archived: row.get("archived")?,
    date: row.get("date")?,
    stars: row.get("stars")?,
    forks: row.get("forks")?,
    watchers: row.get("watchers")?,
    issues: row.get("issues")?,
    prs: row.get("prs")?,
    clones_count: row.get("clones_count")?,
    clones_uniques: row.get("clones_uniques")?,
    views_count: row.get("views_count")?,
    views_uniques: row.get("views_uniques")?,
  })
}

fn repo_metrics_from_row(row: &Row<'_>) -> rusqlite::Result<RepoMetrics> {
  Ok(RepoMetrics {
    date: row.get("date")?,
    clones_count: row.get("clones_count")?,
    clones_uniques: row.get("clones_uniques")?,
    views_count: row.get("views_count")?,
    views_uniques: row.get("views_uniques")?,
  })
}

fn repo_stars_from_row(row: &Row<'_>) -> rusqlite::Result<RepoStars> {
  Ok(RepoStars { date: row.get("date")?, stars: row.get("stars")? })
}

fn repo_popular_item_from_row(row: &Row<'_>) -> rusqlite::Result<RepoPopularItem> {
  Ok(RepoPopularItem {
    name: row.get("name")?,
    count: row.get("count")?,
    uniques: row.get("uniques")?,
  })
}

fn repo_item_from_row(row: &Row<'_>) -> rusqlite::Result<RepoItem> {
  Ok(RepoItem {
    id: row.get("id")?,
    name: row.get("name")?,
    archived: row.get("archived")?,
    stars_synced: row.get("stars_synced")?,
  })
}

pub struct DbClient {
  db: Mutex<Connection>,
}

impl DbClient {
  pub async fn new(db_path: &str) -> Res<Self> {
    let db = get_db(db_path).await?;
    Ok(Self { db: Mutex::new(db) })
  }

  pub async fn check_writable(&self) -> Res {
    let mut db = self.db.lock().unwrap();
    let tx = db.transaction()?;

    tx.execute(
      "
      CREATE TABLE IF NOT EXISTS _ghstats_healthcheck (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        checked_at TEXT NOT NULL
      );
      ",
      [],
    )?;

    tx.execute(
      "
      INSERT INTO _ghstats_healthcheck (id, checked_at)
      VALUES (1, datetime('now'))
      ON CONFLICT(id) DO UPDATE SET checked_at = excluded.checked_at;
      ",
      [],
    )?;

    tx.rollback()?;
    Ok(())
  }

  // MARK: Getters

  pub async fn get_repos_ids(&self) -> Res<Vec<i64>> {
    let qs = "SELECT id FROM repos WHERE hidden = FALSE;";
    let db = self.db.lock().unwrap();
    let mut stmt = db.prepare(qs)?;
    let items = stmt.query_map([], |row| row.get(0))?.collect::<rusqlite::Result<Vec<i64>>>()?;
    Ok(items)
  }

  pub async fn get_repo_totals(&self, repo: &str) -> Res<Option<RepoTotals>> {
    let qs = format!("{} WHERE r.hidden = FALSE AND r.name = $1;", TOTAL_QUERY);
    let db = self.db.lock().unwrap();
    let item = db.query_row(&qs, params![repo], repo_totals_from_row).optional()?;
    Ok(item)
  }

  pub async fn get_metrics(&self, repo: &str) -> Res<Vec<RepoMetrics>> {
    let qs = "
    SELECT rs.date, rs.clones_count, rs.clones_uniques, rs.views_count, rs.views_uniques
    FROM repo_stats rs
    INNER JOIN repos r ON r.id = rs.repo_id
    WHERE r.hidden = FALSE AND r.name = $1 AND (rs.clones_count > 0 OR rs.views_count > 0)
    ORDER BY rs.date ASC;
    ";

    let db = self.db.lock().unwrap();
    let mut stmt = db.prepare(qs)?;
    let items = stmt
      .query_map(params![repo], repo_metrics_from_row)?
      .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(items)
  }

  pub async fn get_repos(&self, filter: &RepoFilter) -> Res<Vec<RepoTotals>> {
    let qs = format!(
      "{} WHERE r.hidden = FALSE ORDER BY {} {}",
      TOTAL_QUERY, filter.sort, filter.direction
    );
    let db = self.db.lock().unwrap();
    let mut stmt = db.prepare(&qs)?;
    let items = stmt.query_map([], repo_totals_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(items)
  }

  pub async fn get_stars(&self, repo: &str) -> Res<Vec<RepoStars>> {
    let qs = "
    SELECT date, stars FROM repo_stats rs
    INNER JOIN repos r ON r.id = rs.repo_id
    WHERE r.hidden = FALSE AND r.name = $1
    ORDER BY rs.date ASC;
    ";

    let db = self.db.lock().unwrap();
    let mut stmt = db.prepare(qs)?;
    let mut items: Vec<RepoStars> =
      stmt.query_map(params![repo], repo_stars_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;

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
    WHERE r.hidden = FALSE AND r.name = $1 AND {time_where}
    GROUP BY rr.{col}
    ORDER BY {order_by};
    ");

    let db = self.db.lock().unwrap();
    let mut stmt = db.prepare(&qs)?;
    let items = stmt
      .query_map(params![repo], repo_popular_item_from_row)?
      .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(items)
  }

  pub async fn repos_to_sync(&self) -> Res<Vec<RepoItem>> {
    let qs = "SELECT * FROM repos WHERE stars_synced = FALSE AND hidden = FALSE";
    let db = self.db.lock().unwrap();
    let mut stmt = db.prepare(qs)?;
    let items = stmt.query_map([], repo_item_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(items)
  }

  // MARK: Inserters

  pub async fn insert_repo(&self, repo: &Repo) -> Res {
    let qs = "
    INSERT INTO repos (id, name, description, archived, fork)
    VALUES ($1, $2, $3, $4, $5)
    ON CONFLICT(id) DO UPDATE SET
      name = excluded.name,
      description = excluded.description,
      archived = excluded.archived,
      fork = excluded.fork,
      hidden = FALSE; -- reset hidden flag if repo was hidden and appeared again
    ";

    let db = self.db.lock().unwrap();
    db.execute(
      qs,
      params![repo.id as i64, &repo.full_name, &repo.description, repo.archived, repo.fork],
    )?;

    Ok(())
  }

  pub async fn insert_stats(&self, repo: &Repo, date: &str, prs: &[PullRequest]) -> Res {
    let qs = "
    INSERT INTO repo_stats AS t (repo_id, date, stars, forks, watchers, issues, prs)
    VALUES ($1, $2, $3, $4, $5, $6, $7)
    ON CONFLICT(repo_id, date) DO UPDATE SET
      stars = MAX(t.stars, excluded.stars),
      forks = MAX(t.forks, excluded.forks),
      watchers = MAX(t.watchers, excluded.watchers),
      issues = MAX(t.issues, excluded.issues),
      prs = MAX(t.prs, excluded.prs);
    ";

    let db = self.db.lock().unwrap();
    db.execute(
      qs,
      params![
        repo.id as i64,
        date,
        repo.stargazers_count as i32,
        repo.forks_count as i32,
        repo.watchers_count as i32,
        repo.open_issues_count as i32 - prs.len() as i32,
        prs.len() as i32,
      ],
    )?;

    Ok(())
  }

  pub async fn insert_stars(&self, repo_id: i64, stars: &[(String, u32, u32)]) -> Res {
    let qs = "
    INSERT INTO repo_stats AS t (repo_id, date, stars)
    VALUES ((SELECT id FROM repos WHERE id = $1), $2, $3)
    ON CONFLICT(repo_id, date) DO UPDATE SET
      stars = MAX(t.stars, excluded.stars);
    ";

    let db = self.db.lock().unwrap();
    for (date, acc_count, _) in stars {
      db.execute(qs, params![repo_id, date, *acc_count as i32])?;
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

    let db = self.db.lock().unwrap();
    for doc in &clones.clones {
      db.execute(
        qs,
        params![repo.id as i64, &doc.timestamp, doc.count as i32, doc.uniques as i32],
      )?;
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

    let db = self.db.lock().unwrap();
    for doc in &views.views {
      db.execute(
        qs,
        params![repo.id as i64, &doc.timestamp, doc.count as i32, doc.uniques as i32],
      )?;
    }

    Ok(())
  }

  pub async fn insert_referrers(&self, repo: &Repo, date: &str, docs: &[RepoReferrer]) -> Res {
    let qs = "
    INSERT INTO repo_referrers AS t (repo_id, date, referrer, count, uniques)
    VALUES ($1, $2, $3, $4, $5)
    ON CONFLICT(repo_id, date, referrer) DO UPDATE SET
      count = MAX(t.count, excluded.count),
      uniques = MAX(t.uniques, excluded.uniques);
    ";

    let db = self.db.lock().unwrap();
    for rec in docs {
      db.execute(
        qs,
        params![repo.id as i64, date, &rec.referrer, rec.count as i32, rec.uniques as i32],
      )?;
    }

    Ok(())
  }

  pub async fn insert_paths(&self, repo: &Repo, date: &str, docs: &[RepoPopularPath]) -> Res {
    let qs = "
    INSERT INTO repo_popular_paths AS t (repo_id, date, path, title, count, uniques)
    VALUES ($1, $2, $3, $4, $5, $6)
    ON CONFLICT(repo_id, date, path) DO UPDATE SET
      count = MAX(t.count, excluded.count),
      uniques = MAX(t.uniques, excluded.uniques);
    ";

    let db = self.db.lock().unwrap();
    for rec in docs {
      db.execute(
        qs,
        params![repo.id as i64, date, &rec.path, &rec.title, rec.count as i32, rec.uniques as i32],
      )?;
    }

    Ok(())
  }

  // MARK: Updater

  pub async fn update_deltas(&self) -> Res {
    let items = [("repo_referrers", "referrer"), ("repo_popular_paths", "path")];
    let db = self.db.lock().unwrap();

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

      db.execute(&qs, [])?;
    }

    Ok(())
  }

  pub async fn mark_repo_hidden(&self, repos_ids: &[i64]) -> Res {
    if repos_ids.is_empty() {
      return Ok(());
    }

    let ids = repos_ids.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",");
    let qs = format!("UPDATE repos SET hidden = TRUE WHERE id IN ({});", ids);
    let db = self.db.lock().unwrap();
    db.execute(&qs, [])?;
    Ok(())
  }

  pub async fn mark_repo_stars_synced(&self, repo_id: i64) -> Res {
    let qs = "UPDATE repos SET stars_synced = TRUE WHERE id = $1;";
    let db = self.db.lock().unwrap();
    db.execute(qs, params![repo_id])?;
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use std::time::{SystemTime, UNIX_EPOCH};

  use super::*;
  use crate::gh_client::{
    PullRequest, Repo, RepoClones, RepoPopularPath, RepoReferrer, RepoViews, TrafficDaily,
  };

  fn db_path(test_name: &str) -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    std::env::temp_dir()
      .join(format!("ghstats-{test_name}-{}-{nanos}.db", std::process::id()))
      .to_string_lossy()
      .into_owned()
  }

  async fn test_db(test_name: &str) -> Res<DbClient> {
    DbClient::new(&db_path(test_name)).await
  }

  fn sample_repo(id: u64, full_name: &str) -> Repo {
    Repo {
      id,
      full_name: full_name.to_string(),
      description: Some(format!("{full_name} description")),
      stargazers_count: 10,
      forks_count: 2,
      watchers_count: 3,
      open_issues_count: 7,
      fork: false,
      archived: false,
    }
  }

  fn pr(id: u64) -> PullRequest {
    PullRequest { id, title: format!("PR {id}") }
  }

  fn daily(timestamp: &str, count: u32, uniques: u32) -> TrafficDaily {
    TrafficDaily { timestamp: timestamp.to_string(), count, uniques }
  }

  #[tokio::test]
  async fn migrates_schema_and_healthcheck_rolls_back() -> Res {
    let db = test_db("migrations").await?;
    db.check_writable().await?;

    let conn = db.db.lock().unwrap();
    let version: i32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    assert_eq!(version, 3);

    let mut stmt = conn.prepare(
      "
      SELECT name FROM sqlite_master
      WHERE type = 'table'
      ORDER BY name;
      ",
    )?;
    let tables =
      stmt.query_map([], |row| row.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;

    assert!(tables.contains(&"repos".to_string()));
    assert!(tables.contains(&"repo_stats".to_string()));
    assert!(tables.contains(&"repo_referrers".to_string()));
    assert!(tables.contains(&"repo_popular_paths".to_string()));
    assert!(!tables.contains(&"_ghstats_healthcheck".to_string()));

    Ok(())
  }

  #[tokio::test]
  async fn repos_can_be_inserted_hidden_restored_and_marked_synced() -> Res {
    let db = test_db("repos").await?;
    let mut repo = sample_repo(1, "owner/repo");

    db.insert_repo(&repo).await?;
    assert_eq!(db.get_repos_ids().await?, vec![1]);
    assert_eq!(db.repos_to_sync().await?.len(), 1);

    db.mark_repo_stars_synced(1).await?;
    assert!(db.repos_to_sync().await?.is_empty());

    db.mark_repo_hidden(&[]).await?;
    assert_eq!(db.get_repos_ids().await?, vec![1]);

    db.mark_repo_hidden(&[1]).await?;
    assert!(db.get_repos_ids().await?.is_empty());

    repo.description = Some("updated".to_string());
    repo.archived = true;
    repo.fork = true;
    db.insert_repo(&repo).await?;
    assert_eq!(db.get_repos_ids().await?, vec![1]);

    let conn = db.db.lock().unwrap();
    let row: (String, bool, bool, bool, bool) = conn.query_row(
      "SELECT description, archived, fork, hidden, stars_synced FROM repos WHERE id = 1",
      [],
      |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
    )?;
    assert_eq!(row, ("updated".to_string(), true, true, false, true));

    Ok(())
  }

  #[tokio::test]
  async fn stats_upserts_feed_totals_metrics_and_sorting() -> Res {
    let db = test_db("stats").await?;
    let repo = sample_repo(1, "owner/repo");
    let other = Repo {
      id: 2,
      full_name: "owner/other".to_string(),
      stargazers_count: 50,
      ..sample_repo(2, "owner/other")
    };

    db.insert_repo(&repo).await?;
    db.insert_repo(&other).await?;

    db.insert_stats(&repo, "2024-01-01T00:00:00Z", &[pr(1), pr(2)]).await?;
    db.insert_stats(
      &Repo {
        stargazers_count: 8,
        forks_count: 5,
        watchers_count: 1,
        open_issues_count: 9,
        ..sample_repo(1, "owner/repo")
      },
      "2024-01-01T00:00:00Z",
      &[pr(1)],
    )
    .await?;
    db.insert_stats(
      &Repo {
        stargazers_count: 12,
        forks_count: 3,
        watchers_count: 4,
        ..sample_repo(1, "owner/repo")
      },
      "2024-01-02T00:00:00Z",
      &[],
    )
    .await?;
    db.insert_stats(&other, "2024-01-02T00:00:00Z", &[]).await?;

    db.insert_clones(
      &repo,
      &RepoClones {
        count: 0,
        uniques: 0,
        clones: vec![daily("2024-01-01T00:00:00Z", 3, 2), daily("2024-01-02T00:00:00Z", 5, 4)],
      },
    )
    .await?;
    db.insert_views(
      &repo,
      &RepoViews {
        count: 0,
        uniques: 0,
        views: vec![daily("2024-01-01T00:00:00Z", 30, 20), daily("2024-01-02T00:00:00Z", 50, 40)],
      },
    )
    .await?;

    let totals = db.get_repo_totals("owner/repo").await?.unwrap();
    assert_eq!(totals.date, "2024-01-02T00:00:00Z");
    assert_eq!(totals.stars, 12);
    assert_eq!(totals.forks, 3);
    assert_eq!(totals.watchers, 4);
    assert_eq!(totals.issues, 7);
    assert_eq!(totals.prs, 0);
    assert_eq!(totals.clones_count, 8);
    assert_eq!(totals.clones_uniques, 6);
    assert_eq!(totals.views_count, 80);
    assert_eq!(totals.views_uniques, 60);

    let metrics = db.get_metrics("owner/repo").await?;
    assert_eq!(metrics.len(), 2);
    assert_eq!(metrics[0].date, "2024-01-01T00:00:00Z");
    assert_eq!(metrics[1].date, "2024-01-02T00:00:00Z");

    let repos = db
      .get_repos(&RepoFilter {
        sort: RepoSort::Stars,
        direction: Direction::Desc,
        ..RepoFilter::default()
      })
      .await?;
    assert_eq!(
      repos.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(),
      vec!["owner/other", "owner/repo"]
    );

    db.mark_repo_hidden(&[2]).await?;
    assert!(db.get_repo_totals("owner/other").await?.is_none());
    assert_eq!(db.get_repos(&RepoFilter::default()).await?.len(), 1);

    Ok(())
  }

  #[tokio::test]
  async fn stars_history_restores_gaps_and_ignores_leading_empty_rows() -> Res {
    let db = test_db("stars").await?;
    let repo = sample_repo(1, "owner/repo");
    db.insert_repo(&repo).await?;

    db.insert_clones(
      &repo,
      &RepoClones {
        count: 0,
        uniques: 0,
        clones: vec![daily("2024-01-01T00:00:00Z", 1, 1), daily("2024-01-03T00:00:00Z", 1, 1)],
      },
    )
    .await?;
    db.insert_stars(
      1,
      &[("2024-01-02T00:00:00Z".to_string(), 2, 2), ("2024-01-04T00:00:00Z".to_string(), 5, 3)],
    )
    .await?;

    let stars = db.get_stars("owner/repo").await?;
    assert_eq!(
      stars.iter().map(|x| (&x.date, x.stars)).collect::<Vec<_>>(),
      vec![
        (&"2024-01-02T00:00:00Z".to_string(), 2),
        (&"2024-01-03T00:00:00Z".to_string(), 2),
        (&"2024-01-04T00:00:00Z".to_string(), 5),
      ]
    );

    Ok(())
  }

  #[tokio::test]
  async fn popular_item_deltas_are_non_negative_and_sorted() -> Res {
    let db = test_db("popular").await?;
    let repo = sample_repo(1, "owner/repo");
    db.insert_repo(&repo).await?;

    db.insert_referrers(
      &repo,
      "2024-01-01T00:00:00Z",
      &[
        RepoReferrer { referrer: "google.com".to_string(), count: 10, uniques: 5 },
        RepoReferrer { referrer: "reddit.com".to_string(), count: 1, uniques: 1 },
      ],
    )
    .await?;
    db.insert_referrers(
      &repo,
      "2024-01-02T00:00:00Z",
      &[
        RepoReferrer { referrer: "google.com".to_string(), count: 14, uniques: 3 },
        RepoReferrer { referrer: "reddit.com".to_string(), count: 1, uniques: 1 },
      ],
    )
    .await?;
    db.insert_paths(
      &repo,
      "2024-01-01T00:00:00Z",
      &[RepoPopularPath { path: "/".to_string(), title: "Home".to_string(), count: 7, uniques: 4 }],
    )
    .await?;
    db.insert_paths(
      &repo,
      "2024-01-02T00:00:00Z",
      &[RepoPopularPath { path: "/".to_string(), title: "Home".to_string(), count: 5, uniques: 2 }],
    )
    .await?;

    db.update_deltas().await?;

    let by_count =
      PopularFilter { sort: PopularSort::Count, direction: Direction::Desc, period: 0 };
    let refs = db.get_popular_items("owner/repo", &PopularKind::Refs, &by_count).await?;
    assert_eq!(
      refs.iter().map(|x| (&x.name, x.count, x.uniques)).collect::<Vec<_>>(),
      vec![(&"google.com".to_string(), 14, 5), (&"reddit.com".to_string(), 1, 1),]
    );

    let paths = db.get_popular_items("owner/repo", &PopularKind::Path, &by_count).await?;
    assert_eq!(
      paths.iter().map(|x| (&x.name, x.count, x.uniques)).collect::<Vec<_>>(),
      vec![(&"/".to_string(), 7, 4),]
    );

    Ok(())
  }
}
