use std::sync::Arc;

use axum::extract::{Query, Request, State};
use axum::Json;

use crate::db_client::{RepoFilter, RepoTotals};
use crate::helpers::get_filtered_repos;
use crate::types::JsonRes;
use crate::AppState;

#[derive(Debug, serde::Serialize)]
pub struct ReposList {
  total_count: i32,
  total_stars: i32,
  total_forks: i32,
  total_views: i32,
  total_clones: i32,
  items: Vec<RepoTotals>,
}

pub async fn api_get_repos(State(state): State<Arc<AppState>>, req: Request) -> JsonRes<ReposList> {
  let db = &state.db;
  let qs: Query<RepoFilter> = Query::try_from_uri(req.uri())?;
  let repos = get_filtered_repos(&db, &qs).await?;

  let repos_list = ReposList {
    total_count: repos.len() as i32,
    total_stars: repos.iter().map(|r| r.stars).sum(),
    total_forks: repos.iter().map(|r| r.forks).sum(),
    total_views: repos.iter().map(|r| r.views_count).sum(),
    total_clones: repos.iter().map(|r| r.clones_count).sum(),
    items: repos,
  };

  Ok(Json(repos_list))
}
