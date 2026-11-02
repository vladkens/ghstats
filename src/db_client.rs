use std::sync::Mutex;

use anyhow::Context;
use chrono::{Days, NaiveDate};
use rusqlite::{Connection, OptionalExtension, Row, named_params, params};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_variant::to_variant_name;

use crate::gh_client::{
  PullRequest, Release, Repo, RepoClones, RepoPopularPath, RepoReferrer, RepoViews,
};
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

fn migrate_v4(db: &Connection) -> Res {
  let mut queries = vec![];

  // GitHub only exposes the current cumulative download_count, so each row is one asset's count on
  // one day, upserted on every sync. The key serves the previous-snapshot lookup, the index serves
  // per-date reads
  let qs = "CREATE TABLE IF NOT EXISTS repo_release_assets (
    repo_id INTEGER NOT NULL,
    asset_id INTEGER NOT NULL,
    date TEXT NOT NULL,
    release_id INTEGER NOT NULL,
    tag TEXT NOT NULL,
    asset_name TEXT NOT NULL,
    published_at TEXT DEFAULT NULL,
    created_at TEXT DEFAULT NULL,
    download_count INTEGER NOT NULL DEFAULT 0,
    -- in the day's last fetch; a removed asset keeps its row for the deltas
    current BOOLEAN NOT NULL DEFAULT TRUE,
    PRIMARY KEY (repo_id, asset_id, date)
  );";
  queries.push(qs);

  let qs = "CREATE INDEX IF NOT EXISTS repo_release_assets_date
    ON repo_release_assets (repo_id, date);";
  queries.push(qs);

  // date of the last successful releases fetch, its current rows are the repo's assets
  let qs = "ALTER TABLE repos ADD COLUMN releases_synced_at TEXT DEFAULT NULL;";
  queries.push(qs);

  for qs in queries {
    db.execute(qs, [])?;
  }

  Ok(())
}

fn migrate_v5(db: &Connection) -> Res {
  let queries = vec![
    "ALTER TABLE repos ADD COLUMN language TEXT DEFAULT NULL;",
    "ALTER TABLE repos ADD COLUMN license TEXT DEFAULT NULL;",
    "ALTER TABLE repos ADD COLUMN topics TEXT DEFAULT NULL;",
    "ALTER TABLE repos ADD COLUMN created_at TEXT DEFAULT NULL;",
    "ALTER TABLE repos ADD COLUMN pushed_at TEXT DEFAULT NULL;",
    "ALTER TABLE repos ADD COLUMN homepage TEXT DEFAULT NULL;",
    "ALTER TABLE repo_release_assets ADD COLUMN prerelease BOOLEAN NOT NULL DEFAULT FALSE;",
    // star history rows are written with FALSE: those days have no traffic data
    "ALTER TABLE repo_stats ADD COLUMN traffic BOOLEAN NOT NULL DEFAULT TRUE;",
    // only rows the old star history could have written: live stats rows have other counts
    "UPDATE repo_stats SET traffic = FALSE
    WHERE clones_count = 0 AND views_count = 0
      AND forks = 0 AND watchers = 0 AND issues = 0 AND prs = 0 AND date < (
      SELECT MIN(x.date) FROM repo_stats x
      WHERE x.repo_id = repo_stats.repo_id AND (x.clones_count > 0 OR x.views_count > 0)
    );",
    // re-run the star history once so gaps from past outages fill
    "UPDATE repos SET stars_synced = FALSE;",
  ];

  for qs in queries {
    db.execute(qs, [])?;
  }

  Ok(())
}

const MIGRATIONS: &[fn(&Connection) -> Res] =
  &[migrate_v1, migrate_v2, migrate_v3, migrate_v4, migrate_v5];

fn migrate(db: &Connection) -> Res {
  let version: i32 = db.query_row("PRAGMA user_version", [], |row| row.get(0))?;
  let latest = MIGRATIONS.len() as i32;
  if version >= latest {
    return Ok(());
  }

  // one transaction for all pending versions: a failed upgrade leaves the db as it was
  let tx = db.unchecked_transaction()?;
  for (idx, func) in MIGRATIONS.iter().enumerate().skip(version as usize) {
    let mig_ver = idx as i32 + 1;
    tracing::info!("running migration to v{}", mig_ver);
    func(&tx).with_context(|| format!("migration to v{mig_ver} failed"))?;
  }

  tx.execute_batch(&format!("PRAGMA user_version = {latest}"))?;
  tx.commit()?;
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

#[derive(Clone, Debug, Default, Serialize)]
pub struct RepoPeriod {
  pub name: String,
  pub views: i64,
  pub views_uniques: i64,
  pub clones: i64,
  pub clones_uniques: i64,
  pub cmp_views: i64,
  pub cmp_clones: i64,
  pub prev_views: i64,
  pub prev_clones: i64,
  pub cmp_days: i64,
  pub prev_days: i64,
  pub stars: i64,
  pub stars_start: i64,
  pub stars_synced: bool,
}

impl RepoPeriod {
  pub fn stars_gain(&self) -> Option<i64> {
    (self.stars_synced || self.stars_start > 0).then_some(self.stars - self.stars_start)
  }
}

/// `None` is a day without a row; in the backfill a missing row is a day without traffic
#[derive(Clone, Debug, Default, Serialize)]
pub struct RepoDaily {
  pub start: Option<String>,
  /// [clones_count, clones_uniques, views_count, views_uniques]
  pub days: Vec<Option<[i64; 4]>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RepoMeta {
  pub language: Option<String>,
  pub license: Option<String>,
  pub topics: Vec<String>,
  pub created_at: Option<String>,
  pub pushed_at: Option<String>,
  pub homepage: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RepoItem {
  pub id: i64,
  pub name: String,
  pub archived: bool,
  pub stars_synced: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DownloadsDaily {
  pub date: String,
  pub downloads: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReleaseDownloads {
  pub release_id: i64,
  pub tag: String,
  pub published_at: Option<String>,
  pub prerelease: bool,
  pub total: i64,
  pub period: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssetDownloads {
  pub name: String,
  pub total: i64,
  pub period: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RepoDownloads {
  pub id: i64,
  pub name: String,
  pub total: i64,
  pub period: i64,
}

// MARK: Filters

pub enum PopularKind {
  Refs,
  Path,
}

#[derive(Debug, Default, Clone, Copy, Deserialize, Serialize, PartialEq)]
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

#[derive(Debug, Default, Clone, Copy, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PopularSort {
  Name,
  #[default]
  Count,
  Uniques,
}

impl std::fmt::Display for PopularSort {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "{}", to_variant_name(self).unwrap())
  }
}

/// Days ending today; -1 is all time
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Period(pub i32);

impl Default for Period {
  fn default() -> Self {
    Self(30)
  }
}

impl Period {
  pub const ALL: [Period; 5] = [Period(7), Period(30), Period(90), Period(365), Period(-1)];

  // old links used plain days: 7, 14, 30, 90 and -1
  pub fn parse(val: &str) -> Self {
    match val.trim().to_lowercase().as_str() {
      "7d" | "7" => Self(7),
      "14d" | "14" => Self(14),
      "30d" | "30" => Self(30),
      "90d" | "90" => Self(90),
      "1y" | "365d" | "365" => Self(365),
      "all" | "-1" => Self(-1),
      _ => Self::default(),
    }
  }

  pub fn days(&self) -> Option<i64> {
    (self.0 > 0).then_some(self.0 as i64)
  }

  pub fn label(&self) -> String {
    match self.0 {
      365 => "1y".to_string(),
      x if x > 0 => format!("{x}d"),
      _ => "All".to_string(),
    }
  }

  pub fn title(&self) -> String {
    match self.0 {
      365 => "Last 12 months".to_string(),
      x if x > 0 => format!("Last {x} days"),
      _ => "All time".to_string(),
    }
  }

  pub fn windows(&self, today: NaiveDate) -> Option<Windows> {
    let days = Days::new(self.days()? as u64);
    let start = today.checked_sub_days(Days::new(self.0 as u64 - 1))?;
    let cmp_end = today.checked_sub_days(Days::new(CMP_LAG - 1))?;
    let cmp_start = cmp_end.checked_sub_days(days)?;
    let prev_start = cmp_start.checked_sub_days(days)?;
    Some(Windows { start, cmp_start, cmp_end, prev_start })
  }
}

impl std::fmt::Display for Period {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "{}", self.label().to_lowercase())
  }
}

impl Serialize for Period {
  fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
    ser.serialize_str(&self.to_string())
  }
}

impl<'de> Deserialize<'de> for Period {
  fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
    Ok(Self::parse(&String::deserialize(de)?))
  }
}

/// GitHub's last 2 days of traffic are incomplete, so compared windows end before them
pub const CMP_LAG: u64 = 2;

/// The first sync gets 14 days of traffic, only days that had any: a missing row there means 0
pub const BACKFILL_DAYS: u64 = 15;

/// Shown period `[start, today]`, compared `[cmp_start, cmp_end)` vs `[prev_start, cmp_start)`
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Windows {
  pub start: NaiveDate,
  pub cmp_start: NaiveDate,
  pub cmp_end: NaiveDate,
  pub prev_start: NaiveDate,
}

/// Unknown query values fall back to the default instead of failing the request
fn lenient<'de, D: Deserializer<'de>, T: serde::de::DeserializeOwned + Default>(
  de: D,
) -> Result<T, D::Error> {
  let val = String::deserialize(de)?;
  let de = serde::de::value::StrDeserializer::<serde::de::value::Error>::new(&val);
  Ok(T::deserialize(de).unwrap_or_default())
}

fn lenient_opt<'de, D: Deserializer<'de>, T: serde::de::DeserializeOwned>(
  de: D,
) -> Result<Option<T>, D::Error> {
  let val = String::deserialize(de)?;
  let de = serde::de::value::StrDeserializer::<serde::de::value::Error>::new(&val);
  Ok(T::deserialize(de).ok())
}

#[derive(Debug, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct RepoFilter {
  #[serde(deserialize_with = "lenient")]
  pub sort: RepoSort,
  #[serde(deserialize_with = "lenient")]
  pub direction: Direction,
  pub q: Option<String>,
  pub owner: Option<String>,
  pub period: Period,
}

#[derive(Debug, Default, Clone, Copy, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseSort {
  Tag,
  #[default]
  Published,
  Period,
  Total,
}

impl std::fmt::Display for ReleaseSort {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "{}", to_variant_name(self).unwrap())
  }
}

/// Repo page state; `sort`/`direction` is the old shared pair, the popular tables' fallback
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct PopularFilter {
  #[serde(deserialize_with = "lenient")]
  pub sort: PopularSort,
  #[serde(deserialize_with = "lenient")]
  pub direction: Direction,
  pub period: Period,
  #[serde(deserialize_with = "lenient_opt")]
  pub refs_sort: Option<PopularSort>,
  #[serde(deserialize_with = "lenient_opt")]
  pub refs_dir: Option<Direction>,
  #[serde(deserialize_with = "lenient_opt")]
  pub paths_sort: Option<PopularSort>,
  #[serde(deserialize_with = "lenient_opt")]
  pub paths_dir: Option<Direction>,
  #[serde(deserialize_with = "lenient")]
  pub rsort: ReleaseSort,
  #[serde(deserialize_with = "lenient")]
  pub rdir: Direction,
}

impl PopularFilter {
  pub fn sorting(&self, kind: &PopularKind) -> (PopularSort, Direction) {
    match kind {
      PopularKind::Refs => {
        (self.refs_sort.unwrap_or(self.sort), self.refs_dir.unwrap_or(self.direction))
      }
      PopularKind::Path => {
        (self.paths_sort.unwrap_or(self.sort), self.paths_dir.unwrap_or(self.direction))
      }
    }
  }

  /// The filter one popular table queries with
  pub fn table(&self, kind: &PopularKind) -> Self {
    let (sort, direction) = self.sorting(kind);
    Self { sort, direction, ..self.clone() }
  }
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

fn repo_period_from_row(row: &Row<'_>) -> rusqlite::Result<RepoPeriod> {
  Ok(RepoPeriod {
    name: row.get("name")?,
    views: row.get("views")?,
    views_uniques: row.get("views_uniques")?,
    clones: row.get("clones")?,
    clones_uniques: row.get("clones_uniques")?,
    cmp_views: row.get("cmp_views")?,
    cmp_clones: row.get("cmp_clones")?,
    prev_views: row.get("prev_views")?,
    prev_clones: row.get("prev_clones")?,
    cmp_days: row.get("cmp_days")?,
    prev_days: row.get("prev_days")?,
    stars: row.get("stars")?,
    stars_start: row.get("stars_start")?,
    stars_synced: row.get("stars_synced")?,
  })
}

fn repo_meta_from_row(row: &Row<'_>) -> rusqlite::Result<RepoMeta> {
  let topics: Option<String> = row.get("topics")?;
  Ok(RepoMeta {
    language: row.get("language")?,
    license: row.get("license")?,
    topics: topics.and_then(|x| serde_json::from_str(&x).ok()).unwrap_or_default(),
    created_at: row.get("created_at")?,
    pushed_at: row.get("pushed_at")?,
    homepage: row.get("homepage")?,
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

fn downloads_daily_from_row(row: &Row<'_>) -> rusqlite::Result<DownloadsDaily> {
  Ok(DownloadsDaily { date: row.get("date")?, downloads: row.get("downloads")? })
}

fn release_downloads_from_row(row: &Row<'_>) -> rusqlite::Result<ReleaseDownloads> {
  Ok(ReleaseDownloads {
    release_id: row.get("release_id")?,
    tag: row.get("tag")?,
    published_at: row.get("published_at")?,
    prerelease: row.get("prerelease")?,
    total: row.get("total")?,
    period: row.get("period")?,
  })
}

fn asset_downloads_from_row(row: &Row<'_>) -> rusqlite::Result<AssetDownloads> {
  Ok(AssetDownloads {
    name: row.get("asset_name")?,
    total: row.get("total")?,
    period: row.get("period")?,
  })
}

fn repo_downloads_from_row(row: &Row<'_>) -> rusqlite::Result<RepoDownloads> {
  Ok(RepoDownloads {
    id: row.get("repo_id")?,
    name: row.get("repo_name")?,
    total: row.get("total")?,
    period: row.get("period")?,
  })
}

/// Release asset snapshots since `:since` of visible repos matching `cond`, with `delta` against
/// the asset's previous snapshot: a gap goes to the later date and a decrease is clamped to 0.
///
/// An asset's first snapshot counts in full when the asset became downloadable (the later of its
/// `created_at` and the release's `published_at`) on or after the repo's previous snapshot date: it
/// appeared after that day's last fetch, also across a sync outage. On the repo's first snapshot
/// the bar is the day before instead, so first-day downloads count but older assets are only the
/// baseline (NULL), as is an asset public before the previous snapshot yet missing from it.
fn downloads_cte(cond: &str) -> String {
  #[rustfmt::skip]
  let qs = format!("
  WITH deltas AS (
    SELECT ra.*, MAX(0, ra.download_count - COALESCE((
      SELECT p.download_count FROM repo_release_assets p
      WHERE p.repo_id = ra.repo_id AND p.asset_id = ra.asset_id AND p.date < ra.date
      ORDER BY p.date DESC LIMIT 1
    ), CASE
      -- multi-argument MAX is NULL when any argument is, so each side falls back to the other
      WHEN substr(MAX(
        COALESCE(ra.created_at, ra.published_at), COALESCE(ra.published_at, ra.created_at)
      ), 1, 10) >= COALESCE((
        SELECT substr(MAX(q.date), 1, 10) FROM repo_release_assets q
        WHERE q.repo_id = ra.repo_id AND q.date < ra.date
      ), date(substr(ra.date, 1, 10), '-1 day')) THEN 0
    END)) AS delta
    FROM repos r
    INNER JOIN repo_release_assets ra ON ra.repo_id = r.id
    WHERE r.hidden = FALSE AND {cond} AND ra.date >= :since
  )
  ");
  qs
}

/// Totals per `key`: all-time sums the assets of the last successful fetch (matches GitHub, deleted
/// assets drop out), period sums the deltas since `:since`
fn downloads_totals_query(cond: &str, key: &str, order_by: &str) -> String {
  // CROSS JOIN keeps repos as the outer loop, so current rows come from the date index
  #[rustfmt::skip]
  let qs = format!("
  {cte}
  SELECT c.*, r.name AS repo_name, SUM(c.download_count) AS total, COALESCE(p.period, 0) AS period
  FROM repos r
  CROSS JOIN repo_release_assets c
    ON c.repo_id = r.id AND c.date = r.releases_synced_at AND c.current = TRUE
  LEFT JOIN (SELECT {key}, SUM(delta) AS period FROM deltas GROUP BY {key}) p ON p.{key} = c.{key}
  WHERE r.hidden = FALSE AND {cond}
  GROUP BY c.{key}
  ORDER BY {order_by};
  ", cte = downloads_cte(cond));
  qs
}

/// Start date of the last `period` days, same as `date('now', '-N day')`; 0 is all time
fn period_since(period: i32) -> String {
  match period {
    x if x > 0 => {
      (chrono::Utc::now() - chrono::Duration::days(x as i64)).format("%Y-%m-%d").to_string()
    }
    _ => String::new(), // every date sorts after an empty string
  }
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

  pub async fn get_first_traffic(&self) -> Res<Option<NaiveDate>> {
    let qs = "
    SELECT MIN(substr(rs.date, 1, 10)) FROM repo_stats rs
    INNER JOIN repos r ON r.id = rs.repo_id
    WHERE r.hidden = FALSE AND (rs.clones_count > 0 OR rs.views_count > 0);
    ";

    let db = self.db.lock().unwrap();
    let day: Option<String> = db.query_row(qs, [], |row| row.get(0))?;
    Ok(day.and_then(|x| x.parse().ok()))
  }

  /// Unlike `get_stars`, rows without stars are skipped, not forward-filled
  pub async fn get_star_rows(&self, repo: &str) -> Res<Vec<RepoStars>> {
    let qs = "
    SELECT date, stars FROM repo_stats rs
    INNER JOIN repos r ON r.id = rs.repo_id
    WHERE r.hidden = FALSE AND r.name = $1 AND rs.stars > 0
    ORDER BY rs.date ASC;
    ";

    let db = self.db.lock().unwrap();
    let mut stmt = db.prepare(qs)?;
    let items =
      stmt.query_map(params![repo], repo_stars_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(items)
  }

  pub async fn get_repo_meta(&self, repo: &str) -> Res<RepoMeta> {
    let qs = "SELECT * FROM repos WHERE hidden = FALSE AND name = $1;";
    let db = self.db.lock().unwrap();
    let item = db.query_row(qs, params![repo], repo_meta_from_row).optional()?;
    Ok(item.unwrap_or_default())
  }

  pub async fn get_last_stats_date(&self) -> Res<Option<NaiveDate>> {
    let qs = "SELECT MAX(substr(date, 1, 10)) FROM repo_stats WHERE traffic;";
    let db = self.db.lock().unwrap();
    let day: Option<String> = db.query_row(qs, [], |row| row.get(0))?;
    Ok(day.and_then(|x| x.parse().ok()))
  }

  pub async fn get_periods(
    &self,
    repo: Option<&str>,
    period: Period,
    today: NaiveDate,
  ) -> Res<Vec<RepoPeriod>> {
    let [start, cmp_start, cmp_end, prev_start] = match period.windows(today) {
      Some(x) => [x.start, x.cmp_start, x.cmp_end, x.prev_start].map(|x| x.to_string()),
      None => ["0000"; 4].map(String::from),
    };

    // ?N, not $N: SQLite numbers $N params by first appearance
    let qs = "
    WITH ft AS (
      SELECT repo_id, first, MIN(date(first, '+' || ?6 || ' day'), date(last, '+1 day')) AS full
      FROM (
        SELECT repo_id, MAX(IIF(traffic, substr(date, 1, 10), NULL)) AS last,
          MIN(IIF(clones_count > 0 OR views_count > 0, substr(date, 1, 10), NULL)) AS first
        FROM repo_stats
        WHERE ?5 IS NULL OR repo_id IN (SELECT id FROM repos WHERE name = ?5)
        GROUP BY repo_id
      )
    )
    SELECT
      r.name, r.stars_synced,
      SUM(IIF(rs.date >= ?1, rs.views_count, 0)) AS views,
      SUM(IIF(rs.date >= ?1, rs.views_uniques, 0)) AS views_uniques,
      SUM(IIF(rs.date >= ?1, rs.clones_count, 0)) AS clones,
      SUM(IIF(rs.date >= ?1, rs.clones_uniques, 0)) AS clones_uniques,
      SUM(IIF(rs.date >= ?2 AND rs.date < ?3, rs.views_count, 0)) AS cmp_views,
      SUM(IIF(rs.date >= ?2 AND rs.date < ?3, rs.clones_count, 0)) AS cmp_clones,
      SUM(IIF(rs.date >= ?4 AND rs.date < ?2, rs.views_count, 0)) AS prev_views,
      SUM(IIF(rs.date >= ?4 AND rs.date < ?2, rs.clones_count, 0)) AS prev_clones,
      SUM(IIF(rs.date >= ?2 AND rs.date < ?3 AND rs.date >= ft.full AND rs.traffic, 1, 0)) + CAST(COALESCE(MAX(0,
        MIN(julianday(?3), julianday(ft.full)) - MAX(julianday(?2), julianday(ft.first))
      ), 0) AS INTEGER) AS cmp_days,
      SUM(IIF(rs.date >= ?4 AND rs.date < ?2 AND rs.date >= ft.full AND rs.traffic, 1, 0)) + CAST(COALESCE(MAX(0,
        MIN(julianday(?2), julianday(ft.full)) - MAX(julianday(?4), julianday(ft.first))
      ), 0) AS INTEGER) AS prev_days,
      COALESCE((
        SELECT x.stars FROM repo_stats x WHERE x.repo_id = r.id
        ORDER BY x.date DESC LIMIT 1
      ), 0) AS stars,
      COALESCE((
        SELECT x.stars FROM repo_stats x WHERE x.repo_id = r.id AND x.stars > 0 AND x.date < ?1
        ORDER BY x.date DESC LIMIT 1
      ), 0) AS stars_start
    FROM repos r
    INNER JOIN repo_stats rs ON rs.repo_id = r.id
    LEFT JOIN ft ON ft.repo_id = r.id
    WHERE r.hidden = FALSE AND (?5 IS NULL OR r.name = ?5)
    GROUP BY r.id;
    ";

    let db = self.db.lock().unwrap();
    let mut stmt = db.prepare(qs)?;
    let items = stmt
      .query_map(
        params![start, cmp_start, cmp_end, prev_start, repo, BACKFILL_DAYS as i64],
        repo_period_from_row,
      )?
      .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(items)
  }

  pub async fn get_daily(&self, repo: &str, today: NaiveDate) -> Res<RepoDaily> {
    let qs = "
    SELECT substr(rs.date, 1, 10) AS day,
      rs.clones_count, rs.clones_uniques, rs.views_count, rs.views_uniques
    FROM repo_stats rs
    INNER JOIN repos r ON r.id = rs.repo_id
    WHERE r.hidden = FALSE AND r.name = $1 AND rs.traffic AND rs.date >= (
      SELECT MIN(date) FROM repo_stats
      WHERE repo_id = r.id AND (clones_count > 0 OR views_count > 0)
    )
    ORDER BY rs.date ASC;
    ";

    let rows = {
      let db = self.db.lock().unwrap();
      let mut stmt = db.prepare(qs)?;
      stmt
        .query_map(params![repo], |row| {
          let day: String = row.get(0)?;
          Ok((day, [row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?]))
        })?
        .collect::<rusqlite::Result<Vec<(String, [i64; 4])>>>()?
    };

    let Some(start) = rows.first().and_then(|x| x.0.parse::<NaiveDate>().ok()) else {
      return Ok(RepoDaily::default());
    };

    // the backfill ends early when nothing was collected after it
    let last = rows.last().and_then(|x| x.0.parse::<NaiveDate>().ok()).unwrap_or(start);
    let backfill = ((last - start).num_days() + 1).min(BACKFILL_DAYS as i64) as usize;

    let len = (today - start).num_days().max(0) as usize + 1;
    let mut days = vec![None; len];
    for x in days.iter_mut().take(backfill) {
      *x = Some([0; 4]);
    }

    for (day, vals) in rows {
      let Ok(day) = day.parse::<NaiveDate>() else { continue };
      if let Some(x) = days.get_mut((day - start).num_days() as usize) {
        *x = Some(vals);
      }
    }

    Ok(RepoDaily { start: Some(start.to_string()), days })
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

    let time_where = match filter.period.days() {
      Some(x) => format!("date >= date('now', '-{} day')", x - 1),
      None => "1=1".to_string(),
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

  /// Downloads per day summed over the repo's assets; days with only baseline snapshots are skipped
  pub async fn get_downloads_daily(&self, repo: &str) -> Res<Vec<DownloadsDaily>> {
    #[rustfmt::skip]
    let qs = format!("
    {cte}
    SELECT date, SUM(delta) AS downloads FROM deltas
    WHERE delta IS NOT NULL
    GROUP BY date
    ORDER BY date ASC;
    ", cte = downloads_cte("r.name = :repo"));

    let db = self.db.lock().unwrap();
    let mut stmt = db.prepare(&qs)?;
    let items = stmt
      .query_map(named_params! { ":repo": repo, ":since": "" }, downloads_daily_from_row)?
      .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(items)
  }

  /// (day, release_id, downloads) for one repo
  pub async fn get_release_downloads_daily(&self, repo: &str) -> Res<Vec<(String, i64, i64)>> {
    #[rustfmt::skip]
    let qs = format!("
    {cte}
    SELECT substr(date, 1, 10) AS day, release_id, SUM(delta) AS downloads FROM deltas
    WHERE delta IS NOT NULL
    GROUP BY day, release_id
    ORDER BY day ASC;
    ", cte = downloads_cte("r.name = :repo"));

    let db = self.db.lock().unwrap();
    let mut stmt = db.prepare(&qs)?;
    let items = stmt
      .query_map(named_params! { ":repo": repo, ":since": "" }, |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
      })?
      .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(items)
  }

  /// (repo_id, day, downloads) for all repos since `since`
  pub async fn get_repos_downloads_daily(&self, since: &str) -> Res<Vec<(i64, String, i64)>> {
    #[rustfmt::skip]
    let qs = format!("
    {cte}
    SELECT repo_id, substr(date, 1, 10) AS day, SUM(delta) AS downloads FROM deltas
    WHERE delta IS NOT NULL
    GROUP BY repo_id, day;
    ", cte = downloads_cte("1=1"));

    let db = self.db.lock().unwrap();
    let mut stmt = db.prepare(&qs)?;
    let items = stmt
      .query_map(named_params! { ":since": since }, |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
      })?
      .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(items)
  }

  pub async fn get_release_downloads(&self, repo: &str, period: i32) -> Res<Vec<ReleaseDownloads>> {
    let qs = downloads_totals_query("r.name = :repo", "release_id", "published_at DESC");
    let db = self.db.lock().unwrap();
    let mut stmt = db.prepare(&qs)?;
    let items = stmt
      .query_map(
        named_params! { ":repo": repo, ":since": period_since(period) },
        release_downloads_from_row,
      )?
      .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(items)
  }

  /// Totals per current asset name; deltas follow the asset id, so a renamed asset keeps its
  /// history
  pub async fn get_asset_downloads(&self, repo: &str, period: i32) -> Res<Vec<AssetDownloads>> {
    #[rustfmt::skip]
    let qs = format!("
    {cte}
    SELECT c.asset_name, SUM(c.download_count) AS total, COALESCE(SUM(p.period), 0) AS period
    FROM repos r
    CROSS JOIN repo_release_assets c
      ON c.repo_id = r.id AND c.date = r.releases_synced_at AND c.current = TRUE
    LEFT JOIN (
      SELECT asset_id, SUM(delta) AS period FROM deltas GROUP BY asset_id
    ) p ON p.asset_id = c.asset_id
    WHERE r.hidden = FALSE AND r.name = :repo
    GROUP BY c.asset_name
    ORDER BY total DESC, c.asset_name ASC;
    ", cte = downloads_cte("r.name = :repo"));

    let db = self.db.lock().unwrap();
    let mut stmt = db.prepare(&qs)?;
    let items = stmt
      .query_map(
        named_params! { ":repo": repo, ":since": period_since(period) },
        asset_downloads_from_row,
      )?
      .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(items)
  }

  pub async fn get_repos_downloads(&self, period: i32) -> Res<Vec<RepoDownloads>> {
    let qs = downloads_totals_query("1=1", "repo_id", "total DESC, repo_name ASC");
    let db = self.db.lock().unwrap();
    let mut stmt = db.prepare(&qs)?;
    let items = stmt
      .query_map(named_params! { ":since": period_since(period) }, repo_downloads_from_row)?
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
    INSERT INTO repos (
      id, name, description, archived, fork,
      language, license, topics, created_at, pushed_at, homepage
    )
    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
    ON CONFLICT(id) DO UPDATE SET
      name = excluded.name,
      description = excluded.description,
      archived = excluded.archived,
      fork = excluded.fork,
      language = excluded.language,
      license = excluded.license,
      topics = excluded.topics,
      created_at = excluded.created_at,
      pushed_at = excluded.pushed_at,
      homepage = excluded.homepage,
      hidden = FALSE; -- reset hidden flag if repo was hidden and appeared again
    ";

    let license =
      repo.license.as_ref().and_then(|x| x.spdx_id.clone()).filter(|x| x != "NOASSERTION");
    let topics = serde_json::to_string(&repo.topics)?;
    let homepage = repo.homepage.as_ref().filter(|x| !x.is_empty());
    let db = self.db.lock().unwrap();
    db.execute(
      qs,
      params![
        repo.id as i64,
        &repo.full_name,
        &repo.description,
        repo.archived,
        repo.fork,
        &repo.language,
        license,
        topics,
        &repo.created_at,
        &repo.pushed_at,
        homepage,
      ],
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
      prs = MAX(t.prs, excluded.prs),
      traffic = TRUE;
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
    // an empty response never wipes the history
    if stars.is_empty() {
      return Ok(());
    }

    // the new history replaces the old one: counts drop when people unstar, mixing makes dips
    let clear = "DELETE FROM repo_stats WHERE repo_id = $1 AND traffic = FALSE;";
    let qs = "
    INSERT INTO repo_stats AS t (repo_id, date, stars, traffic)
    VALUES ((SELECT id FROM repos WHERE id = $1), $2, $3, FALSE)
    ON CONFLICT(repo_id, date) DO UPDATE SET
      stars = MAX(t.stars, excluded.stars);
    ";

    let mut db = self.db.lock().unwrap();
    let tx = db.transaction()?;
    tx.execute(clear, params![repo_id])?;
    for (date, acc_count, _) in stars {
      tx.execute(qs, params![repo_id, date, *acc_count as i32])?;
    }

    tx.commit()?;

    Ok(())
  }

  pub async fn insert_clones(&self, repo: &Repo, clones: &RepoClones) -> Res {
    let qs = "
    INSERT INTO repo_stats AS t (repo_id, date, clones_count, clones_uniques)
    VALUES ($1, $2, $3, $4)
    ON CONFLICT(repo_id, date) DO UPDATE SET
      clones_count = MAX(t.clones_count, excluded.clones_count),
      clones_uniques = MAX(t.clones_uniques, excluded.clones_uniques),
      traffic = TRUE;
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
      views_uniques = MAX(t.views_uniques, excluded.views_uniques),
      traffic = TRUE;
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

  pub async fn insert_release_assets(&self, repo: &Repo, date: &str, releases: &[Release]) -> Res {
    let qs = "
    INSERT INTO repo_release_assets AS t
      (repo_id, asset_id, date, release_id, tag, asset_name, published_at, created_at, download_count,
        prerelease)
    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
    ON CONFLICT(repo_id, asset_id, date) DO UPDATE SET
      tag = excluded.tag,
      prerelease = excluded.prerelease,
      asset_name = excluded.asset_name,
      published_at = excluded.published_at,
      created_at = excluded.created_at,
      download_count = MAX(t.download_count, excluded.download_count),
      current = TRUE;
    ";

    // one transaction, so the current snapshot of a repo is never partial
    let mut db = self.db.lock().unwrap();
    let tx = db.transaction()?;
    for rel in releases {
      for asset in &rel.assets {
        tx.execute(
          qs,
          params![
            repo.id as i64,
            asset.id as i64,
            date,
            rel.id as i64,
            &rel.tag_name,
            &asset.name,
            &rel.published_at,
            &asset.created_at,
            asset.download_count as i64,
            rel.prerelease,
          ],
        )?;
      }
    }

    // assets missing from this fetch leave the totals, their rows keep the deltas
    let ids = releases.iter().flat_map(|x| &x.assets).map(|x| (x.id as i64).to_string());
    let qs = format!(
      "UPDATE repo_release_assets SET current = FALSE
      WHERE repo_id = $1 AND date = $2 AND asset_id NOT IN ({});",
      ids.collect::<Vec<_>>().join(",")
    );
    tx.execute(&qs, params![repo.id as i64, date])?;

    let qs = "UPDATE repos SET releases_synced_at = $1 WHERE id = $2;";
    tx.execute(qs, params![date, repo.id as i64])?;

    tx.commit()?;
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

  pub async fn mark_repos_stars_unsynced(&self) -> Res {
    let qs = "UPDATE repos SET stars_synced = FALSE;";
    let db = self.db.lock().unwrap();
    db.execute(qs, [])?;
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
    PullRequest, Release, ReleaseAsset, Repo, RepoClones, RepoPopularPath, RepoReferrer, RepoViews,
    TrafficDaily,
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
      has_pull_requests: None,
      ..Default::default()
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
    assert_eq!(version, 5);

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
    assert!(tables.contains(&"repo_release_assets".to_string()));
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
      PopularFilter { sort: PopularSort::Count, period: Period(-1), ..Default::default() };
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

  #[test]
  fn period_parses_new_and_legacy_values() {
    let cases = [
      ("7d", 7),
      ("7", 7),
      ("14", 14),
      ("30d", 30),
      ("90", 90),
      ("1y", 365),
      ("All", -1),
      ("-1", -1),
      ("", 30),
      ("0", 30),
      ("junk", 30),
    ];
    for (val, days) in cases {
      assert_eq!(Period::parse(val), Period(days), "{val}");
    }

    assert_eq!(Period::ALL.map(|x| x.to_string()), ["7d", "30d", "90d", "1y", "all"]);

    let day = |x: &str| x.parse::<NaiveDate>().unwrap();
    let win = Period(30).windows(day("2026-11-02")).unwrap();
    assert_eq!(win.start, day("2026-10-04"));
    assert_eq!((win.cmp_start, win.cmp_end), (day("2026-10-02"), day("2026-11-01")));
    assert_eq!(win.prev_start, day("2026-09-02"));
    assert!(Period(-1).windows(day("2026-11-02")).is_none());
  }

  #[tokio::test]
  async fn periods_and_daily_series_are_gap_aware() -> Res {
    let db = test_db("periods").await?;
    let repo = sample_repo(1, "owner/repo");
    db.insert_repo(&repo).await?;

    // traffic on Jan 10, 11, 13 and 15
    let history = [("2024-01-01T00:00:00Z", 3, 3), ("2024-01-14T00:00:00Z", 8, 5)];
    db.insert_stars(1, &history.map(|(d, a, n)| (d.to_string(), a, n))).await?;
    let views = [(10, 20, 10), (11, 30, 12), (13, 40, 50), (15, 5, 1)];
    db.insert_views(
      &repo,
      &RepoViews {
        count: 0,
        uniques: 0,
        views: views
          .iter()
          .map(|(d, c, u)| daily(&format!("2024-01-{d}T00:00:00Z"), *c, *u))
          .collect(),
      },
    )
    .await?;
    let synced = Repo { stargazers_count: 8, ..sample_repo(1, "owner/repo") };
    db.insert_stats(&synced, "2024-01-15T00:00:00Z", &[]).await?;

    let today = NaiveDate::from_ymd_opt(2024, 1, 16).unwrap();
    let dat = db.get_daily("owner/repo", today).await?;
    assert_eq!(dat.start.as_deref(), Some("2024-01-10"));
    assert_eq!(
      dat.days.iter().map(|x| x.map(|v| v[2])).collect::<Vec<_>>(),
      vec![Some(20), Some(30), Some(0), Some(40), Some(0), Some(5), None]
    );

    // 3d: shown 14..16, compared 12..14 vs 9..11
    let rp = db.get_periods(Some("owner/repo"), Period(3), today).await?.remove(0);
    assert_eq!((rp.views, rp.views_uniques), (5, 1));
    assert_eq!((rp.cmp_views, rp.cmp_days, rp.prev_views, rp.prev_days), (40, 3, 50, 2));
    assert_eq!((rp.stars, rp.stars_start, rp.stars_gain()), (8, 3, Some(5)));

    let stars = db.get_star_rows("owner/repo").await?;
    assert_eq!(
      stars.iter().map(|x| (&x.date[..10], x.stars)).collect::<Vec<_>>(),
      vec![("2024-01-01", 3), ("2024-01-14", 8), ("2024-01-15", 8)]
    );

    let all = db.get_periods(None, Period(-1), today).await?;
    assert_eq!(all.len(), 1);
    assert_eq!((all[0].views, all[0].prev_views, all[0].prev_days), (95, 0, 0));

    assert!(db.get_daily("owner/missing", today).await?.days.is_empty());

    Ok(())
  }

  fn days_ago(n: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::days(n)).format("%Y-%m-%dT00:00:00Z").to_string()
  }

  fn release(id: u64, tag: &str, published_at: &str, assets: &[(u64, &str, u64)]) -> Release {
    Release {
      id,
      tag_name: tag.to_string(),
      draft: false,
      prerelease: false,
      published_at: Some(published_at.to_string()),
      assets: assets
        .iter()
        .map(|(id, name, count)| ReleaseAsset {
          id: *id,
          name: name.to_string(),
          download_count: *count,
          created_at: None, // falls back to published_at
        })
        .collect(),
    }
  }

  fn created(mut rel: Release, created_at: &str) -> Release {
    rel.assets.iter_mut().for_each(|x| x.created_at = Some(created_at.to_string()));
    rel
  }

  #[tokio::test]
  async fn first_day_rule_uses_when_an_asset_became_public() -> Res {
    let db = test_db("downloads-first-day").await?;
    let repo = sample_repo(1, "owner/repo");
    db.insert_repo(&repo).await?;

    let at = |n, time: &str| days_ago(n).replace("00:00:00", time);
    let old = "2024-01-01T00:00:00Z";
    let rels = [
      // uploaded to a draft on D-5, published on D-1: counts in full
      created(release(1, "draft", &at(4, "12:00:00"), &[(10, "a", 7)]), &at(8, "10:00:00")),
      // uploaded into an old release on D-2 23:59: before the window, baseline only
      created(release(2, "late", old, &[(20, "b", 3)]), &at(5, "23:59:59")),
      // uploaded into an old release on D-1 00:00: counts in full
      created(release(3, "edge", old, &[(30, "c", 2)]), &at(4, "00:00:00")),
    ];
    db.insert_release_assets(&repo, &days_ago(3), &rels).await?;

    let daily = db.get_downloads_daily("owner/repo").await?;
    let daily = daily.iter().map(|x| (x.date.clone(), x.downloads)).collect::<Vec<_>>();
    assert_eq!(daily, vec![(days_ago(3), 9)]);

    Ok(())
  }

  #[tokio::test]
  async fn assets_published_during_a_sync_gap_count_in_full() -> Res {
    let db = test_db("downloads-gap").await?;
    let repo = sample_repo(1, "owner/repo");
    db.insert_repo(&repo).await?;

    // tracked on day 10, no syncs on days 9..7, next snapshot on day 6
    let at = |n, time: &str| days_ago(n).replace("00:00:00", time);
    let old = "2024-01-01T00:00:00Z";
    db.insert_release_assets(&repo, &days_ago(10), &[release(1, "v1.0", old, &[(10, "a", 100)])])
      .await?;

    let rels = [
      release(1, "v1.0", old, &[(10, "a", 110)]),
      // published during the outage: counts in full
      release(2, "v1.1", &at(9, "12:00:00"), &[(20, "b", 20)]),
      // public on the previous snapshot's day but missing from it, so it came after that fetch
      created(release(3, "late", old, &[(30, "c", 4)]), &at(10, "20:00:00")),
      // public before the previous snapshot yet missing from it: unknown history, baseline only
      created(release(4, "odd", old, &[(40, "d", 5)]), &at(12, "08:00:00")),
    ];
    db.insert_release_assets(&repo, &days_ago(6), &rels).await?;

    let daily = db.get_downloads_daily("owner/repo").await?;
    let daily = daily.iter().map(|x| (x.date.clone(), x.downloads)).collect::<Vec<_>>();
    assert_eq!(daily, vec![(days_ago(6), 34)]);

    Ok(())
  }

  #[tokio::test]
  async fn same_day_refetch_keeps_only_the_last_fetch() -> Res {
    let db = test_db("downloads-refetch").await?;
    let repo = sample_repo(1, "owner/repo");
    db.insert_repo(&repo).await?;

    let (d4, d3) = (days_ago(4), days_ago(3));
    let rel = |assets: &[(u64, &str, u64)]| release(1, "v1.0", "2024-01-01T00:00:00Z", assets);
    db.insert_release_assets(&repo, &d4, &[rel(&[(10, "a", 10), (11, "b", 20), (12, "c", 30)])])
      .await?;
    db.insert_release_assets(&repo, &d3, &[rel(&[(10, "a", 12), (11, "b", 25), (12, "c", 31)])])
      .await?;

    // later the same day: b is deleted, c is re-uploaded under a new id
    let mut last = rel(&[(10, "a", 13)]);
    last.assets.extend(created(rel(&[(13, "c", 1)]), &d3.replace("T00", "T15")).assets);
    db.insert_release_assets(&repo, &d3, &[last]).await?;

    let daily = db.get_downloads_daily("owner/repo").await?;
    let daily = daily.iter().map(|x| (x.date.as_str(), x.downloads)).collect::<Vec<_>>();
    // b's and the old c's deltas from the morning stay in the history
    assert_eq!(daily, vec![(d3.as_str(), 10)]);

    // totals only list the last fetch, period deltas still include b and the old c
    let rels = db.get_release_downloads("owner/repo", 0).await?;
    assert_eq!(rels.iter().map(|x| (x.total, x.period)).collect::<Vec<_>>(), vec![(14, 10)]);

    let assets = db.get_asset_downloads("owner/repo", 0).await?;
    let assets = assets.iter().map(|x| (x.name.as_str(), x.total, x.period)).collect::<Vec<_>>();
    assert_eq!(assets, vec![("a", 13, 3), ("c", 1, 1)]);

    let repos = db.get_repos_downloads(0).await?;
    assert_eq!(
      repos.iter().map(|x| (x.id, x.total, x.period)).collect::<Vec<_>>(),
      vec![(1, 14, 10)]
    );

    // an empty fetch later the same day: nothing is current, the history stays
    db.insert_release_assets(&repo, &d3, &[]).await?;
    assert!(db.get_repos_downloads(0).await?.is_empty());
    assert!(db.get_release_downloads("owner/repo", 0).await?.is_empty());
    assert!(db.get_asset_downloads("owner/repo", 0).await?.is_empty());

    let daily = db.get_downloads_daily("owner/repo").await?;
    let daily = daily.iter().map(|x| (x.date.as_str(), x.downloads)).collect::<Vec<_>>();
    assert_eq!(daily, vec![(d3.as_str(), 10)]);

    Ok(())
  }

  #[tokio::test]
  async fn release_downloads_use_deltas_across_gaps_and_clamp_decreases() -> Res {
    let db = test_db("downloads").await?;
    let repo = sample_repo(1, "owner/repo");
    let other = sample_repo(2, "owner/other");
    db.insert_repo(&repo).await?;
    db.insert_repo(&other).await?;

    // tracking starts on day 10, days 8..4 are a gap
    let (d10, d9, d4, d3) = (days_ago(10), days_ago(9), days_ago(4), days_ago(3));
    // v1 predates tracking (baseline only), app-mac drops on day 9, on day 3 app-linux is renamed
    // and app-win is deleted
    let v1 = |assets: &[(u64, &str, u64)]| release(1, "v1.0", "2024-01-01T00:00:00Z", assets);
    // v2 is published the day before its first snapshot, v3 the same day: first-day downloads count
    let v2 = |count| release(2, "v1.1", &d10.replace("T00", "T18"), &[(200, "app-linux", count)]);
    let v3 = || release(3, "v1.2", &d3.replace("T00", "T01"), &[(300, "app-linux", 4)]);

    let s10 = [v1(&[(100, "app-linux", 100), (101, "app-mac", 50), (102, "app-win", 1)])];
    let s9 = [v2(5), v1(&[(100, "app-linux", 110), (101, "app-mac", 40), (102, "app-win", 3)])];
    let s3 = |linux, v2_count| {
      // and re-uploaded into old v1 the day before under a new id: its created_at counts day one
      let mut v1 = v1(&[(100, "app-linux-x64", linux), (101, "app-mac", 45)]);
      let created_at = Some(d4.replace("T00", "T20"));
      v1.assets.push(ReleaseAsset {
        id: 103,
        name: "app-win".to_string(),
        download_count: 6,
        created_at,
      });
      [v3(), v2(v2_count), v1]
    };

    db.insert_release_assets(&repo, &d10, &s10).await?;
    db.insert_release_assets(&repo, &d9, &s9).await?;
    db.insert_release_assets(&repo, &d3, &s3(130, 12)).await?;
    db.insert_release_assets(&repo, &d3, &s3(120, 10)).await?; // never goes down

    // all releases deleted: nothing is current any more
    let tool = release(9, "v0.1", "2024-01-01T00:00:00Z", &[(900, "tool", 7)]);
    db.insert_release_assets(&other, &d9, &[tool]).await?;
    db.insert_release_assets(&other, &d3, &[]).await?;

    let daily = db.get_downloads_daily("owner/repo").await?;
    let daily = daily.iter().map(|x| (x.date.as_str(), x.downloads)).collect::<Vec<_>>();
    assert_eq!(daily, vec![(d9.as_str(), 17), (d3.as_str(), 42)]);

    let rels = |items: Vec<ReleaseDownloads>| {
      items.into_iter().map(|x| (x.release_id, x.tag, x.total, x.period)).collect::<Vec<_>>()
    };
    assert_eq!(
      rels(db.get_release_downloads("owner/repo", 0).await?),
      vec![
        (3, "v1.2".to_string(), 4, 4),
        (2, "v1.1".to_string(), 12, 12),
        (1, "v1.0".to_string(), 181, 43)
      ]
    );
    assert_eq!(
      rels(db.get_release_downloads("owner/repo", 5).await?),
      vec![
        (3, "v1.2".to_string(), 4, 4),
        (2, "v1.1".to_string(), 12, 7),
        (1, "v1.0".to_string(), 181, 31)
      ]
    );
    assert!(db.get_release_downloads("owner/other", 0).await?.is_empty());

    let assets = |items: Vec<AssetDownloads>| {
      items.into_iter().map(|x| (x.name, x.total, x.period)).collect::<Vec<_>>()
    };
    let expected = |linux_x64, linux| {
      vec![
        ("app-linux-x64".to_string(), 130, linux_x64),
        ("app-mac".to_string(), 45, 5),
        ("app-linux".to_string(), 16, linux),
        ("app-win".to_string(), 6, 6),
      ]
    };
    assert_eq!(assets(db.get_asset_downloads("owner/repo", 0).await?), expected(30, 16));
    assert_eq!(assets(db.get_asset_downloads("owner/repo", 5).await?), expected(20, 11));

    let repos = |items: Vec<RepoDownloads>| {
      items.into_iter().map(|x| (x.id, x.name, x.total, x.period)).collect::<Vec<_>>()
    };
    assert_eq!(
      repos(db.get_repos_downloads(0).await?),
      vec![(1, "owner/repo".to_string(), 197, 59)]
    );
    assert_eq!(
      repos(db.get_repos_downloads(5).await?),
      vec![(1, "owner/repo".to_string(), 197, 42)]
    );

    db.mark_repo_hidden(&[1]).await?;
    assert!(db.get_repos_downloads(0).await?.is_empty());
    assert!(db.get_downloads_daily("owner/repo").await?.is_empty());

    Ok(())
  }

  fn db_at_version(test_name: &str, version: i32) -> Res<String> {
    let path = db_path(test_name);
    let conn = Connection::open(&path)?;
    for func in MIGRATIONS.iter().take(version as usize) {
      func(&conn)?;
    }

    conn.execute("INSERT INTO repos (id, name) VALUES (1, 'owner/repo')", [])?;
    conn.execute_batch(&format!("PRAGMA user_version = {version}"))?;
    Ok(path)
  }

  fn user_version(conn: &Connection) -> Res<i32> {
    Ok(conn.query_row("PRAGMA user_version", [], |row| row.get(0))?)
  }

  #[tokio::test]
  async fn failed_migration_leaves_db_unchanged() -> Res {
    let path = db_at_version("migration-failed", 1)?;
    let conn = Connection::open(&path)?;
    // v2 applies cleanly, then v3 fails on the duplicate column
    conn.execute("ALTER TABLE repo_stats ADD COLUMN prs INTEGER NOT NULL DEFAULT 0", [])?;

    assert!(DbClient::new(&path).await.is_err());

    let columns: i32 = conn.query_row(
      "SELECT COUNT(*) FROM pragma_table_info('repos')
      WHERE name IN ('stars_synced', 'fork', 'hidden')",
      [],
      |row| row.get(0),
    )?;
    assert_eq!(user_version(&conn)?, 1);
    assert_eq!(columns, 0);

    conn.execute("ALTER TABLE repo_stats DROP COLUMN prs", [])?;
    DbClient::new(&path).await?;
    assert_eq!(user_version(&conn)?, 5);

    Ok(())
  }

  #[tokio::test]
  async fn v5_keeps_old_rows_as_traffic_and_stores_repo_metadata() -> Res {
    let path = db_at_version("migration-v5", 4)?;
    let conn = Connection::open(&path)?;
    // an old star history row, a live stats row before the first traffic day, then traffic
    conn.execute_batch(
      "UPDATE repos SET stars_synced = TRUE;
      INSERT INTO repo_stats (repo_id, date, stars) VALUES (1, '2026-01-01T00:00:00Z', 1);
      INSERT INTO repo_stats (repo_id, date, stars, watchers) VALUES (1, '2026-01-02T00:00:00Z', 1, 1);
      INSERT INTO repo_stats (repo_id, date, views_count) VALUES (1, '2026-01-05T00:00:00Z', 3);",
    )?;

    let db = DbClient::new(&path).await?;
    let synced: bool = conn.query_row("SELECT stars_synced FROM repos", [], |row| row.get(0))?;
    assert!(!synced, "star history re-runs");

    db.insert_stars(1, &[("2025-12-01T00:00:00Z".into(), 1, 1)]).await?;
    db.insert_stars(1, &[]).await?;
    let rows: Vec<(String, bool)> = conn
      .prepare("SELECT substr(date, 1, 10), traffic FROM repo_stats ORDER BY date")?
      .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
      .collect::<rusqlite::Result<_>>()?;
    let want = [("2025-12-01", false), ("2026-01-02", true), ("2026-01-05", true)];
    assert_eq!(rows, want.map(|(d, t)| (d.to_string(), t)), "history replaced, live rows kept");

    let repo: Repo = serde_json::from_value(serde_json::json!({
      "id": 1, "full_name": "owner/repo", "description": null, "stargazers_count": 1,
      "forks_count": 0, "watchers_count": 0, "open_issues_count": 0, "fork": false, "archived": false,
      "language": "Rust", "license": { "spdx_id": "MIT" }, "topics": ["cli", "tui"],
      "created_at": "2024-06-16T08:34:34Z", "pushed_at": "2026-10-31T00:43:58Z", "homepage": ""
    }))?;
    db.insert_repo(&repo).await?;
    let meta = RepoMeta {
      language: Some("Rust".into()),
      license: Some("MIT".into()),
      topics: vec!["cli".into(), "tui".into()],
      created_at: Some("2024-06-16T08:34:34Z".into()),
      pushed_at: Some("2026-10-31T00:43:58Z".into()),
      homepage: None,
    };
    assert_eq!(db.get_repo_meta("owner/repo").await?, meta);

    Ok(())
  }
}
