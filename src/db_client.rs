use crate::{
  gh_client::{Repo, TrafficClones, TrafficPath, TrafficRefferer, TrafficViews},
  utils::{self, WithError},
};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool};

// MARK: Migrations

async fn migration_1(db: &SqlitePool) -> WithError {
  let mut queries = vec![];

  let qs = "CREATE TABLE IF NOT EXISTS repos (
    repo_id INTEGER PRIMARY KEY,
    full_name TEXT NOT NULL
  );";
  queries.push(qs);

  let qs = "CREATE TABLE IF NOT EXISTS gh_traffic_views (
    repo_id INTEGER NOT NULL,
    count INTEGER NOT NULL,
    uniques INTEGER NOT NULL,
    date TEXT NOT NULL,
    PRIMARY KEY (repo_id, date)
    -- FOREIGN KEY (repo_id) REFERENCES repos(id)
  );";
  queries.push(qs);

  let qs = "CREATE TABLE IF NOT EXISTS gh_traffic_clones (
    repo_id INTEGER NOT NULL,
    count INTEGER NOT NULL,
    uniques INTEGER NOT NULL,
    date TEXT NOT NULL,
    PRIMARY KEY (repo_id, date)
    -- FOREIGN KEY (repo_id) REFERENCES repos(id)
  );";
  queries.push(qs);

  let qs = "CREATE TABLE IF NOT EXISTS gh_traffic_paths (
    repo_id INTEGER NOT NULL,
    path TEXT NOT NULL,
    title TEXT NOT NULL,
    count INTEGER NOT NULL,
    uniques INTEGER NOT NULL,
    date TEXT NOT NULL,
    PRIMARY KEY (repo_id, path, date)
    -- FOREIGN KEY (repo_id) REFERENCES repos(id)
  );";
  queries.push(qs);

  let qs = "CREATE TABLE IF NOT EXISTS gh_traffic_referrers (
    repo_id INTEGER NOT NULL,
    referrer TEXT NOT NULL,
    count INTEGER NOT NULL,
    uniques INTEGER NOT NULL,
    date TEXT NOT NULL,
    PRIMARY KEY (repo_id, referrer, date)
    -- FOREIGN KEY (repo_id) REFERENCES repos(id)
  );";
  queries.push(qs);

  for qs in queries {
    sqlx::query(qs).execute(db).await?;
  }

  Ok(())
}

pub async fn get_db(db_path: &str) -> WithError<SqlitePool> {
  let opts = SqliteConnectOptions::new().filename(db_path).create_if_missing(true);
  let pool = SqlitePool::connect_with(opts).await?;
  migration_1(&pool).await?;
  Ok(pool)
}

// MARK: DbClient

pub struct DbClient {
  pool: SqlitePool,
}

impl DbClient {
  pub async fn new(db_path: &str) -> WithError<Self> {
    let pool = get_db(db_path).await?;
    Ok(Self { pool })
  }

  pub async fn insert_repo(&self, repo: &Repo) -> WithError<u64> {
    let qs = "
    INSERT INTO repos (repo_id, full_name) VALUES ($1, $2) ON CONFLICT DO NOTHING;
    ";

    let _ = sqlx::query(qs) //
      .bind(repo.id as i64)
      .bind(&repo.full_name)
      .execute(&self.pool)
      .await?;

    Ok(repo.id)
  }

  pub async fn insert_traffic_clones(&self, rid: u64, data: &TrafficClones) -> WithError {
    let qs = "
    INSERT INTO gh_traffic_clones (repo_id, count, uniques, date)
    VALUES ($1, $2, $3, $4)
    ON CONFLICT(repo_id, date) DO UPDATE SET (count, uniques) = (excluded.count, excluded.uniques);
    ";

    for clone in &data.clones {
      let _ = sqlx::query(qs) //
        .bind(rid as i64)
        .bind(clone.count as i32)
        .bind(clone.uniques as i32)
        .bind(&clone.timestamp)
        .execute(&self.pool)
        .await?;
    }

    Ok(())
  }

  pub async fn insert_traffic_views(&self, rid: u64, data: &TrafficViews) -> WithError {
    let qs = "
    INSERT INTO gh_traffic_views (repo_id, count, uniques, date)
    VALUES ($1, $2, $3, $4)
    ON CONFLICT(repo_id, date) DO UPDATE SET (count, uniques) = (excluded.count, excluded.uniques);
    ";

    for view in &data.views {
      let _ = sqlx::query(qs) //
        .bind(rid as i64)
        .bind(view.count as i32)
        .bind(view.uniques as i32)
        .bind(&view.timestamp)
        .execute(&self.pool)
        .await?;
    }

    Ok(())
  }

  pub async fn insert_traffic_paths(&self, rid: u64, items: &Vec<TrafficPath>) -> WithError {
    let qs = "
    INSERT INTO gh_traffic_paths (repo_id, path, title, count, uniques, date)
    VALUES ($1, $2, $3, $4, $5, $6)
    ON CONFLICT(repo_id, path, date) DO UPDATE SET (title, count, uniques) = (excluded.title, excluded.count, excluded.uniques);
    ";

    let date = utils::get_utc_hours().to_rfc3339();
    for item in items {
      let _ = sqlx::query(qs) //
        .bind(rid as i64)
        .bind(&item.path)
        .bind(&item.title)
        .bind(item.count as i32)
        .bind(item.uniques as i32)
        .bind(&date)
        .execute(&self.pool)
        .await?;
    }

    Ok(())
  }

  pub async fn insert_traffic_refs(&self, rid: u64, items: &Vec<TrafficRefferer>) -> WithError {
    let qs = "
    INSERT INTO gh_traffic_referrers (repo_id, referrer, count, uniques, date)
    VALUES ($1, $2, $3, $4, $5)
    ON CONFLICT(repo_id, referrer, date) DO UPDATE SET (count, uniques) = (excluded.count, excluded.uniques);
    ";

    let date = utils::get_utc_hours().to_rfc3339();
    for item in items {
      let _ = sqlx::query(qs) //
        .bind(rid as i64)
        .bind(&item.referrer)
        .bind(item.count as i32)
        .bind(item.uniques as i32)
        .bind(&date)
        .execute(&self.pool)
        .await?;
    }

    Ok(())
  }
}
