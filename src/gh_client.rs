use std::{time::Duration, vec};

use reqwest::{
  header::{HeaderMap, HeaderValue},
  RequestBuilder,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

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

// MARK: GhClient

pub struct GhClient {
  client: reqwest::Client,
  base_url: String,
}

impl GhClient {
  pub fn new(token: String) -> Res<GhClient> {
    let user_agent = format!("{}/{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));

    let mut auth_header = HeaderValue::from_str(&format!("Bearer {}", token))?;
    auth_header.set_sensitive(true);

    let mut headers = HeaderMap::new();
    headers.insert("Accept", HeaderValue::from_static("application/vnd.github+json"));
    headers.insert("X-GitHub-Api-Version", HeaderValue::from_static("2022-11-28"));
    headers.insert("Authorization", auth_header);
    headers.insert("User-Agent", HeaderValue::from_str(&user_agent)?);

    let client = reqwest::Client::builder()
      .default_headers(headers)
      .read_timeout(Duration::from_secs(30))
      .build()?;

    let base_url = "https://api.github.com".to_string();
    Ok(GhClient { client, base_url })
  }

  async fn with_pagination<T: DeserializeOwned>(&self, req: RequestBuilder) -> Res<Vec<T>> {
    let mut items: Vec<T> = vec![];
    let per_page = 100;
    let mut page = 1;

    loop {
      let req = req.try_clone().unwrap();
      let req = req.query(&[("per_page", &per_page.to_string())]);
      let req = req.query(&[("page", &page.to_string())]);
      let rep = req.send().await?.error_for_status()?;

      let cur = match rep.headers().get("link") {
        Some(l) => l.to_str().unwrap().to_string(),
        None => "".to_string(),
      };

      let dat = rep.json::<Vec<T>>().await?;
      items.extend(dat);

      match cur.contains(r#"rel="next""#) {
        true => page += 1,
        false => break,
      }
    }

    Ok(items)
  }

  // https://docs.github.com/en/rest/repos/repos?apiVersion=2022-11-28#list-repositories-for-the-authenticated-user
  pub async fn get_repos(&self, include_private: bool) -> Res<Vec<Repo>> {
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
    let rep = self.client.get(url).send().await?.error_for_status()?;
    let dat = rep.json::<RepoClones>().await?;
    Ok(dat)
  }

  pub async fn traffic_views(&self, repo: &str) -> Res<RepoViews> {
    let url = format!("{}/repos/{}/traffic/views", self.base_url, repo);
    let rep = self.client.get(url).send().await?.error_for_status()?;
    let dat = rep.json::<RepoViews>().await?;
    Ok(dat)
  }

  pub async fn traffic_paths(&self, repo: &str) -> Res<Vec<RepoPopularPath>> {
    let url = format!("{}/repos/{}/traffic/popular/paths", self.base_url, repo);
    let rep = self.client.get(url).send().await?.error_for_status()?;
    let dat = rep.json::<Vec<RepoPopularPath>>().await?;
    Ok(dat)
  }

  pub async fn traffic_refs(&self, repo: &str) -> Res<Vec<RepoReferrer>> {
    let url = format!("{}/repos/{}/traffic/popular/referrers", self.base_url, repo);
    let rep = self.client.get(url).send().await?.error_for_status()?;
    let dat = rep.json::<Vec<RepoReferrer>>().await?;
    Ok(dat)
  }

  pub async fn get_latest_release_ver(&self, repo: &str) -> Res<String> {
    let url = format!("{}/repos/{}/releases/latest", self.base_url, repo);
    let rep = self.client.get(url).send().await?.error_for_status()?;
    let dat = rep.json::<serde_json::Value>().await?;
    let ver = dat["tag_name"].as_str().unwrap().to_string();
    let ver = ver.trim_start_matches("v").to_string();
    Ok(ver)
  }

  pub async fn get_stars(&self, repo: &str) -> Res<Vec<RepoStar>> {
    let url = format!("{}/repos/{}/stargazers", self.base_url, repo);
    let req = self.client.get(url).header("Accept", "application/vnd.github.v3.star+json");

    let dat: Vec<RepoStar> = self.with_pagination(req).await?;
    return Ok(dat);
  }
}
