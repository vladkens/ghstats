use std::sync::Mutex;

use chrono::{Days, NaiveDate};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
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

/// Totals for a period ending today, plus the comparison windows from `Period::windows`
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
  /// collected days of the compared and previous windows: from the first traffic day, all of the
  /// backfill, then days with a row
  pub cmp_days: i64,
  pub prev_days: i64,
  /// stars of the latest row (as the repos list shows them) and the last real (non-zero) value
  /// before the period
  pub stars: i64,
  pub stars_start: i64,
  pub stars_synced: bool,
}

impl RepoPeriod {
  /// `None` when stars before the period are unknown: no real value then and the stargazer
  /// history isn't synced yet
  pub fn stars_gain(&self) -> Option<i64> {
    (self.stars_synced || self.stars_start > 0).then_some(self.stars - self.stars_start)
  }
}

/// Daily traffic from the first day with traffic to `today`; `None` is a day without a row, except
/// in the first `BACKFILL_DAYS` up to the last row, where a missing row is a day without traffic.
#[derive(Clone, Debug, Default, Serialize)]
pub struct RepoDaily {
  pub start: Option<String>,
  /// [clones_count, clones_uniques, views_count, views_uniques]
  pub days: Vec<Option<[i64; 4]>>,
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
  #[default]
  Count,
  Uniques,
}

impl std::fmt::Display for PopularSort {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "{}", to_variant_name(self).unwrap())
  }
}

/// Days ending today (inclusive); `-1` is all time.
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

  /// Window starts for the period ending today; `None` for all time
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

/// GitHub's traffic for today and yesterday is still incomplete, so changes compare two equal
/// windows that end this many days before today.
pub const CMP_LAG: u64 = 2;

/// On the first sync GitHub returns the last 14 days of traffic, only days that had any, so a
/// missing row in the first days after the first traffic day means no traffic, not an outage.
pub const BACKFILL_DAYS: u64 = 15;

/// Shown period `[start, today]`, compared `[cmp_start, cmp_end)` vs `[prev_start, cmp_start)`
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Windows {
  pub start: NaiveDate,
  pub cmp_start: NaiveDate,
  pub cmp_end: NaiveDate,
  pub prev_start: NaiveDate,
}

#[derive(Debug, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct RepoFilter {
  pub sort: RepoSort,
  pub direction: Direction,
  pub q: Option<String>,
  pub owner: Option<String>,
  pub period: Period,
}

#[derive(Debug, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct PopularFilter {
  pub sort: PopularSort,
  pub direction: Direction,
  pub period: Period,
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

  /// Rows with a real stars value; unlike `get_stars`, days left at zero (such as GitHub's traffic
  /// backfill after an outage) are skipped instead of forward-filled.
  /// The first day with traffic over all visible repos
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

  /// Period totals for one repo (`Some(name)`) or all of them, for the period ending `today`
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

    // ?1 start, ?2 cmp_start, ?3 cmp_end (exclusive), ?4 prev_start, ?5 repo or all, ?6 backfill
    // days; current stars from the latest row like TOTAL_QUERY, stars before the period from
    // non-zero rows only; collected days: every day of the backfill [first, full) (cut at the last
    // row), then days with a row
    let qs = "
    WITH ft AS (
      SELECT repo_id, first, MIN(date(first, '+' || ?6 || ' day'), date(last, '+1 day')) AS full
      FROM (
        SELECT repo_id, MAX(substr(date, 1, 10)) AS last,
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
      SUM(IIF(rs.date >= ?2 AND rs.date < ?3 AND rs.date >= ft.full, 1, 0)) + CAST(COALESCE(MAX(0,
        MIN(julianday(?3), julianday(ft.full)) - MAX(julianday(?2), julianday(ft.first))
      ), 0) AS INTEGER) AS cmp_days,
      SUM(IIF(rs.date >= ?4 AND rs.date < ?2 AND rs.date >= ft.full, 1, 0)) + CAST(COALESCE(MAX(0,
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
    WHERE r.hidden = FALSE AND r.name = $1 AND rs.date >= (
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
      has_pull_requests: None,
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
      PopularFilter { sort: PopularSort::Count, direction: Direction::Desc, period: Period(-1) };
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

    // stars history before traffic starts, then traffic on days 1, 2, 4 and 6 (3 and 5 missing)
    db.insert_stars(1, &[("2024-01-01T00:00:00Z".to_string(), 3, 3)]).await?;
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
    db.insert_stars(1, &[("2024-01-14T00:00:00Z".to_string(), 8, 5)]).await?;
    let synced = Repo { stargazers_count: 8, ..sample_repo(1, "owner/repo") };
    db.insert_stats(&synced, "2024-01-15T00:00:00Z", &[]).await?;

    let today = NaiveDate::from_ymd_opt(2024, 1, 16).unwrap();
    let dat = db.get_daily("owner/repo", today).await?;
    assert_eq!(dat.start.as_deref(), Some("2024-01-10"));
    assert_eq!(
      dat.days.iter().map(|x| x.map(|v| v[2])).collect::<Vec<_>>(),
      vec![Some(20), Some(30), Some(0), Some(40), Some(0), Some(5), None],
      "missing days in the backfill are days without traffic, up to the last row"
    );

    // 3d: shown 14..16; compared 12..14 vs 9..11 (9 is before traffic starts); all in the backfill
    let rp = db.get_periods(Some("owner/repo"), Period(3), today).await?.remove(0);
    assert_eq!((rp.views, rp.views_uniques), (5, 1));
    assert_eq!((rp.cmp_views, rp.cmp_days, rp.prev_views, rp.prev_days), (40, 3, 50, 2));
    assert_eq!((rp.stars, rp.stars_start, rp.stars_gain()), (8, 3, Some(5)));

    // real stars only: the traffic rows stay out, unlike get_stars' forward fill
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
}
