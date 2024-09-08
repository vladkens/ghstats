mod api;
mod html;

use std::sync::Arc;

use axum::http::StatusCode;
use axum::{extract::Request, middleware::Next, response::IntoResponse, routing::get, Router};
use reqwest::Method;
use tower_http::cors::{Any, CorsLayer};

use crate::AppState;

async fn check_api_token(
  req: Request,
  next: Next,
) -> Result<impl IntoResponse, (StatusCode, String)> {
  let ghs_token = std::env::var("GHS_API_TOKEN").unwrap_or_default();
  let req_token = crate::helpers::get_header(&req, "x-api-token").unwrap_or_default();
  if ghs_token.is_empty() || req_token != ghs_token {
    return Err((StatusCode::UNAUTHORIZED, "unauthorized".to_string()));
  }

  let res = next.run(req).await;
  Ok(res)
}

pub fn api_routes() -> Router<Arc<AppState>> {
  let cors = CorsLayer::new().allow_methods([Method::GET]).allow_origin(Any);

  let router = Router::new()
    .route("/repos", get(api::api_get_repos))
    .layer(axum::middleware::from_fn(check_api_token))
    .layer(cors);

  router
}

pub fn html_routes() -> Router<Arc<AppState>> {
  Router::new().route("/", get(html::index)).route("/:owner/:repo", get(html::repo_page))
}
