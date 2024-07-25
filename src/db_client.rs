use anyhow::Ok;
use serde::{Deserialize, Serialize};
use sqlx::{sqlite::SqliteConnectOptions, FromRow, SqlitePool};

use crate::gh_client::{GhClient, Repo, RepoClones, RepoViews};
use crate::utils::Res;

async fn migrate(db: &SqlitePool) -> Res {
  let mut queries = vec![];

  let qs = "CREATE TABLE IF NOT EXISTS repos (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT DEFAULT NULL,
    archived BOOLEAN DEFAULT FALSE
  );";
  queries.push(qs);

  let qs = "CREATE TABLE IF NOT EXISTS repo_stats (
    id INTEGER NOT NULL,
    date TEXT NOT NULL,
    stars INTEGER NOT NULL DEFAULT 0,
    forks INTEGER NOT NULL DEFAULT 0,
    watchers INTEGER NOT NULL DEFAULT 0,
    issues INTEGER NOT NULL DEFAULT 0,
    clones_count INTEGER NOT NULL DEFAULT 0,
    clones_uniques INTEGER NOT NULL DEFAULT 0,
    views_count INTEGER NOT NULL DEFAULT 0,
    views_uniques INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (id, date)
    -- FOREIGN KEY (repo_id) REFERENCES repos(id)
  );";
  queries.push(qs);

  for qs in queries {
    sqlx::query(qs).execute(db).await?;
  }

  Ok(())
}

pub async fn get_db(db_path: &str) -> Res<SqlitePool> {
  let opts = SqliteConnectOptions::new().filename(db_path).create_if_missing(true);
  let pool = SqlitePool::connect_with(opts).await?;
  migrate(&pool).await?;
  Ok(pool)
}

// MARK: DTOs

#[derive(Clone, FromRow, Debug, Serialize, Deserialize)]
pub struct RepoMetrics {
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

#[derive(Clone, FromRow, Debug, Serialize, Deserialize)]
pub struct RepoStars {
  pub date: String,
  pub stars: i32,
}

// MARK: Inserters

pub async fn insert_repo(db: &SqlitePool, repo: &Repo) -> Res {
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
    .execute(db)
    .await?;

  Ok(())
}

pub async fn insert_stats(db: &SqlitePool, repo: &Repo, date: &str) -> Res {
  insert_repo(db, repo).await?;

  let qs = "
  INSERT INTO repo_stats AS t (id, date, stars, forks, watchers, issues)
  VALUES ($1, $2, $3, $4, $5, $6)
  ON CONFLICT(id, date) DO UPDATE SET
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
    .execute(db)
    .await?;

  Ok(())
}

pub async fn insert_clones(db: &SqlitePool, repo: &Repo, clones: &RepoClones) -> Res {
  let qs = "
  INSERT INTO repo_stats AS t (id, date, clones_count, clones_uniques)
  VALUES ($1, $2, $3, $4)
  ON CONFLICT(id, date) DO UPDATE SET
    clones_count = MAX(t.clones_count, excluded.clones_count),
    clones_uniques = MAX(t.clones_uniques, excluded.clones_uniques);
  ";

  for doc in &clones.clones {
    let _ = sqlx::query(qs)
      .bind(repo.id as i64)
      .bind(&doc.timestamp)
      .bind(doc.count as i32)
      .bind(doc.uniques as i32)
      .execute(db)
      .await?;
  }

  Ok(())
}

pub async fn insert_views(db: &SqlitePool, repo: &Repo, views: &RepoViews) -> Res {
  let qs = "
  INSERT INTO repo_stats AS t (id, date, views_count, views_uniques)
  VALUES ($1, $2, $3, $4)
  ON CONFLICT(id, date) DO UPDATE SET
    views_count = MAX(t.views_count, excluded.views_count),
    views_uniques = MAX(t.views_uniques, excluded.views_uniques);
  ";

  for doc in &views.views {
    let _ = sqlx::query(qs)
      .bind(repo.id as i64)
      .bind(&doc.timestamp)
      .bind(doc.count as i32)
      .bind(doc.uniques as i32)
      .execute(db)
      .await?;
  }

  Ok(())
}

// MARK: Updater

pub async fn update_metrics(db: &SqlitePool, gh: &GhClient) -> Res {
  let date = chrono::Utc::now().to_utc().to_rfc3339();
  let date = date.split("T").next().unwrap().to_owned() + "T00:00:00Z";

  let repos = gh.get_repos("users/vladkens").await?;
  for repo in repos {
    if repo.fork || repo.archived {
      continue;
    }

    let views = gh.traffic_views(&repo.full_name).await?;
    let clones = gh.traffic_clones(&repo.full_name).await?;

    insert_stats(db, &repo, &date).await?;
    insert_views(db, &repo, &views).await?;
    insert_clones(db, &repo, &clones).await?;
  }

  Ok(())
}

// MARK: Getters

const TOTAL_QUERY: &'static str = "
SELECT * FROM repos r
INNER JOIN (
	SELECT
		rs.id,
		SUM(clones_count) AS clones_count, SUM(clones_uniques) AS clones_uniques,
	    SUM(views_count) AS views_count, SUM(views_uniques) AS views_uniques,
	    latest.*
	FROM repo_stats rs
	INNER JOIN (
		SELECT id, MAX(date) AS date, stars, forks, watchers, issues
		FROM repo_stats GROUP BY id
	) latest ON latest.id = rs.id
	GROUP BY rs.id
) rs ON rs.id = r.id
";

pub async fn get_repo_totals(db: &SqlitePool, repo: &str) -> Res<RepoMetrics> {
  let qs = format!("{} WHERE r.name = $1;", TOTAL_QUERY);
  let item = sqlx::query_as(qs.as_str()).bind(repo).fetch_one(db).await?;
  Ok(item)
}

pub async fn get_repos(db: &SqlitePool) -> Res<Vec<RepoMetrics>> {
  let qs = format!("{} ORDER BY stars DESC", TOTAL_QUERY);
  let items = sqlx::query_as(qs.as_str()).fetch_all(db).await?;
  Ok(items)
}

pub async fn get_metrics(db: &SqlitePool, name: &str) -> Res<Vec<RepoMetrics>> {
  let qs = "
  SELECT * FROM repo_stats rs
  INNER JOIN repos r ON r.id = rs.id
  WHERE r.name = $1
  ORDER BY rs.date ASC;
  ";

  let items = sqlx::query_as(qs).bind(name).fetch_all(db).await?;
  Ok(items)
}

pub async fn get_stars(db: &SqlitePool, name: &str) -> Res<Vec<RepoStars>> {
  let qs = "
  SELECT date, stars FROM repo_stats rs
  INNER JOIN repos r ON r.id = rs.id
  WHERE r.name = $1
  ORDER BY rs.date ASC;
  ";

  let mut items: Vec<RepoStars> = sqlx::query_as(qs).bind(name).fetch_all(db).await?;

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
