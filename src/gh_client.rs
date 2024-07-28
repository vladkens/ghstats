use reqwest::header::{HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};

use crate::utils::Res;

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

    let client = reqwest::Client::builder().default_headers(headers).build()?;
    let base_url = "https://api.github.com".to_string();

    Ok(GhClient { client, base_url })
  }

  // https://docs.github.com/en/rest/repos/repos?apiVersion=2022-11-28#list-repositories-for-the-authenticated-user
  pub async fn get_repos(&self) -> Res<Vec<Repo>> {
    let mut items: Vec<Repo> = vec![];
    let mut page = 1;

    loop {
      let url = format!("{}/user/repos?type=owner&per_page=100&page={}", self.base_url, page);
      let rep = self.client.get(url).send().await?.error_for_status()?;

      let link = match rep.headers().get("link") {
        Some(l) => l.to_str().unwrap().to_string(),
        None => "".to_string(),
      };

      let dat = rep.json::<Vec<Repo>>().await?;
      items.extend(dat);

      match link.contains(r#"rel="next""#) {
        true => page += 1,
        false => break,
      }
    }

    Ok(items)
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
}
