use std::path::PathBuf;
use std::time::Duration;
use std::vec;

use anyhow::Context;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use reqwest::{Request, RequestBuilder};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::types::Res;

// MARK: Types

#[derive(Debug, Deserialize, Serialize)]
pub struct Repo {
  pub id: u64,
  pub full_name: String,
  pub description: Option<String>,
  pub stargazers_count: u32,
  pub forks_count: u32,
  pub watchers_count: u32,
  pub open_issues_count: u32,
  pub fork: bool,
  pub archived: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PullRequest {
  pub id: u64,
  pub title: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TrafficDaily {
  pub timestamp: String,
  pub uniques: u32,
  pub count: u32,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RepoClones {
  pub uniques: u32,
  pub count: u32,
  pub clones: Vec<TrafficDaily>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RepoViews {
  pub uniques: u32,
  pub count: u32,
  pub views: Vec<TrafficDaily>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RepoPopularPath {
  pub path: String,
  pub title: String,
  pub count: u32,
  pub uniques: u32,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RepoReferrer {
  pub referrer: String,
  pub count: u32,
  pub uniques: u32,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RepoStar {
  pub starred_at: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Page<T> {
  Items(Vec<T>),
  Repositories { repositories: Vec<T> },
}

impl<T> Page<T> {
  fn into_items(self) -> Vec<T> {
    match self {
      Self::Items(items) => items,
      Self::Repositories { repositories } => repositories,
    }
  }
}

#[derive(Deserialize)]
struct InstallationRepo {
  #[serde(flatten)]
  repo: Repo,
  private: bool,
}

enum TokenSource {
  Value(String),
  File(PathBuf),
}

impl TokenSource {
  async fn read(&self) -> Res<String> {
    match self {
      Self::Value(token) => Ok(token.clone()),
      Self::File(path) => {
        let token = tokio::fs::read_to_string(path)
          .await
          .with_context(|| format!("failed to read GitHub token file: {}", path.display()))?;
        let token = token.trim();
        if token.is_empty() {
          anyhow::bail!("GitHub token file is empty: {}", path.display());
        }
        Ok(token.to_string())
      }
    }
  }
}

// MARK: GhClient

pub struct GhClient {
  client: reqwest::Client,
  token: TokenSource,
  base_url: String,
}

impl GhClient {
  pub fn new(token: String) -> Res<Self> {
    Self::with_token(TokenSource::Value(token), "https://api.github.com".to_string())
  }

  pub fn from_token_file(path: String) -> Res<Self> {
    let path = PathBuf::from(path);
    let token = std::fs::read_to_string(&path)
      .with_context(|| format!("failed to read GitHub token file: {}", path.display()))?;
    if token.trim().is_empty() {
      anyhow::bail!("GitHub token file is empty: {}", path.display());
    }
    Self::with_token(TokenSource::File(path), "https://api.github.com".to_string())
  }

  fn with_token(token: TokenSource, base_url: String) -> Res<Self> {
    let user_agent = format!("{}/{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));

    let mut headers = HeaderMap::new();
    headers.insert("Accept", HeaderValue::from_static("application/vnd.github+json"));
    headers.insert("X-GitHub-Api-Version", HeaderValue::from_static("2022-11-28"));
    headers.insert("User-Agent", HeaderValue::from_str(&user_agent)?);

    let client = reqwest::Client::builder()
      .default_headers(headers)
      .read_timeout(Duration::from_secs(30))
      .build()?;

    Ok(Self { client, token, base_url })
  }

  async fn execute(&self, mut req: Request) -> Res<reqwest::Response> {
    let token = self.token.read().await?;
    let mut auth = HeaderValue::from_str(&format!("Bearer {token}"))?;
    auth.set_sensitive(true);
    req.headers_mut().insert(AUTHORIZATION, auth);
    Ok(self.client.execute(req).await?.error_for_status()?)
  }

  async fn send(&self, req: RequestBuilder) -> Res<reqwest::Response> {
    self.execute(req.build()?).await
  }

  async fn with_pagination<T: DeserializeOwned>(&self, req: RequestBuilder) -> Res<Vec<T>> {
    let mut items: Vec<T> = vec![];
    let per_page = 100;
    let mut page = 1;

    loop {
      let mut req = req.try_clone().unwrap().build()?;
      {
        let mut query = req.url_mut().query_pairs_mut();
        query.append_pair("per_page", &per_page.to_string());
        query.append_pair("page", &page.to_string());
      }
      let rep = self.execute(req).await?;

      let cur = match rep.headers().get("link") {
        Some(l) => l.to_str().unwrap().to_string(),
        None => "".to_string(),
      };

      let dat = rep.json::<Page<T>>().await?;
      items.extend(dat.into_items());

      match cur.contains(r#"rel="next""#) {
        true => page += 1,
        false => break,
      }
    }

    Ok(items)
  }

  // https://docs.github.com/en/rest/repos/repos?apiVersion=2022-11-28#list-repositories-for-the-authenticated-user
  // https://docs.github.com/en/rest/apps/installations?apiVersion=2022-11-28#list-repositories-accessible-to-the-app-installation
  pub async fn get_repos(&self, include_private: bool) -> Res<Vec<Repo>> {
    if self.token.read().await?.starts_with("ghs_") {
      let url = format!("{}/installation/repositories", self.base_url);
      let req = self.client.get(url);
      let dat: Vec<InstallationRepo> = self.with_pagination(req).await?;
      return Ok(
        dat
          .into_iter()
          .filter(|item| include_private || !item.private)
          .map(|item| item.repo)
          .collect(),
      );
    }

    let visibility = if include_private { "all" } else { "public" };
    let url = format!("{}/user/repos?visibility={}", self.base_url, visibility);
    let req = self.client.get(url);
    let dat: Vec<Repo> = self.with_pagination(req).await?;
    Ok(dat)
  }

  pub async fn get_open_pull_requests(&self, repo: &str) -> Res<Vec<PullRequest>> {
    let url = format!("{}/repos/{}/pulls?state=open", self.base_url, repo);
    let req = self.client.get(url);
    let dat: Vec<PullRequest> = self.with_pagination(req).await?;
    Ok(dat)
  }

  // https://docs.github.com/en/rest/metrics/traffic?apiVersion=2022-11-28
  pub async fn traffic_clones(&self, repo: &str) -> Res<RepoClones> {
    let url = format!("{}/repos/{}/traffic/clones", self.base_url, repo);
    let dat = self.send(self.client.get(url)).await?.json::<RepoClones>().await?;
    Ok(dat)
  }

  pub async fn traffic_views(&self, repo: &str) -> Res<RepoViews> {
    let url = format!("{}/repos/{}/traffic/views", self.base_url, repo);
    let dat = self.send(self.client.get(url)).await?.json::<RepoViews>().await?;
    Ok(dat)
  }

  pub async fn traffic_paths(&self, repo: &str) -> Res<Vec<RepoPopularPath>> {
    let url = format!("{}/repos/{}/traffic/popular/paths", self.base_url, repo);
    let dat = self.send(self.client.get(url)).await?.json::<Vec<RepoPopularPath>>().await?;
    Ok(dat)
  }

  pub async fn traffic_refs(&self, repo: &str) -> Res<Vec<RepoReferrer>> {
    let url = format!("{}/repos/{}/traffic/popular/referrers", self.base_url, repo);
    let dat = self.send(self.client.get(url)).await?.json::<Vec<RepoReferrer>>().await?;
    Ok(dat)
  }

  pub async fn get_latest_release_ver(&self, repo: &str) -> Res<String> {
    let url = format!("{}/repos/{}/releases/latest", self.base_url, repo);
    let dat = self.send(self.client.get(url)).await?.json::<serde_json::Value>().await?;
    let ver = dat["tag_name"].as_str().unwrap().to_string();
    let ver = ver.trim_start_matches("v").to_string();
    Ok(ver)
  }

  pub async fn get_stars(&self, repo: &str) -> Res<Vec<RepoStar>> {
    let url = format!("{}/repos/{}/stargazers", self.base_url, repo);
    let req = self.client.get(url).header("Accept", "application/vnd.github.v3.star+json");

    let dat: Vec<RepoStar> = self.with_pagination(req).await?;
    Ok(dat)
  }
}

#[cfg(test)]
mod tests {
  use std::collections::HashMap;
  use std::sync::{Arc, Mutex};
  use std::time::{SystemTime, UNIX_EPOCH};

  use axum::extract::{Query, State};
  use axum::http::{HeaderMap, HeaderValue};
  use axum::response::IntoResponse;
  use axum::routing::get;
  use axum::{Json, Router};

  use super::*;

  #[derive(Clone)]
  struct MockState {
    auth_headers: Arc<Mutex<Vec<String>>>,
    token_path: PathBuf,
  }

  async fn installation_repositories(
    State(state): State<MockState>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
  ) -> impl IntoResponse {
    state
      .auth_headers
      .lock()
      .unwrap()
      .push(headers.get(AUTHORIZATION).unwrap().to_str().unwrap().to_string());

    let page = query.get("page").and_then(|value| value.parse::<u32>().ok()).unwrap_or(1);
    if page == 1 {
      std::fs::write(&state.token_path, "ghs_second\n").unwrap();
    }

    let body = Json(serde_json::json!({
      "repositories": [{
        "id": page,
        "full_name": format!("owner/repo-{page}"),
        "description": null,
        "stargazers_count": 1,
        "forks_count": 2,
        "watchers_count": 3,
        "open_issues_count": 4,
        "fork": false,
        "archived": false,
        "private": page == 2
      }]
    }));

    if page == 1 {
      let mut response = body.into_response();
      response
        .headers_mut()
        .insert("link", HeaderValue::from_static("<http://example.test?page=2>; rel=\"next\""));
      response
    } else {
      body.into_response()
    }
  }

  async fn user_repositories(headers: HeaderMap) -> impl IntoResponse {
    assert_eq!(headers.get(AUTHORIZATION).unwrap(), "Bearer ghp_static");
    Json(serde_json::json!([]))
  }

  async fn mock_server(state: MockState) -> Res<(String, tokio::task::JoinHandle<()>)> {
    let router = Router::new()
      .route("/installation/repositories", get(installation_repositories))
      .route("/user/repos", get(user_repositories))
      .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let handle = tokio::spawn(async move {
      axum::serve(listener, router).await.unwrap();
    });
    Ok((format!("http://{addr}"), handle))
  }

  fn token_path() -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    std::env::temp_dir().join(format!("ghstats-token-{}-{nanos}", std::process::id()))
  }

  #[tokio::test]
  async fn token_file_is_reloaded_for_every_paginated_request() -> Res {
    let token_path = token_path();
    std::fs::write(&token_path, "ghs_first\n")?;
    let auth_headers = Arc::new(Mutex::new(vec![]));
    let state = MockState { auth_headers: auth_headers.clone(), token_path: token_path.clone() };
    let (base_url, server) = mock_server(state).await?;
    let client = GhClient::with_token(TokenSource::File(token_path.clone()), base_url)?;

    let repos = client.get_repos(false).await?;

    assert_eq!(repos.len(), 1);
    assert_eq!(repos[0].full_name, "owner/repo-1");
    assert_eq!(
      *auth_headers.lock().unwrap(),
      vec!["Bearer ghs_first".to_string(), "Bearer ghs_second".to_string()]
    );

    server.abort();
    std::fs::remove_file(token_path)?;
    Ok(())
  }

  #[tokio::test]
  async fn personal_token_keeps_using_user_repositories() -> Res {
    let token_path = token_path();
    let state = MockState { auth_headers: Arc::new(Mutex::new(vec![])), token_path };
    let (base_url, server) = mock_server(state).await?;
    let client = GhClient::with_token(TokenSource::Value("ghp_static".to_string()), base_url)?;

    assert!(client.get_repos(false).await?.is_empty());

    server.abort();
    Ok(())
  }
}
