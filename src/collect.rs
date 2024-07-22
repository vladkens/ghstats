use reqwest::header::{HeaderMap, HeaderValue};

use crate::utils::WithError;

// MARK: Types

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct Repo {
  pub full_name: String,
  pub stargazers_count: u32,
  pub forks_count: u32,
  pub watchers_count: u32,
  pub open_issues_count: u32,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct TrafficDaily {
  pub timestamp: String,
  pub uniques: u32,
  pub count: u32,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct TrafficClones {
  pub uniques: u32,
  pub count: u32,
  pub clones: Vec<TrafficDaily>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct TrafficViews {
  pub uniques: u32,
  pub count: u32,
  pub views: Vec<TrafficDaily>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct TrafficPath {
  pub path: String,
  pub title: String,
  pub count: u32,
  pub uniques: u32,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct TrafficRefferer {
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
  pub fn new() -> WithError<GhClient> {
    let token = std::env::var("GITHUB_TOKEN")?;

    let mut headers = HeaderMap::new();
    headers.insert("Accept", HeaderValue::from_static("application/vnd.github+json"));
    headers.insert("X-GitHub-Api-Version", HeaderValue::from_static("2022-11-28"));
    headers.insert("Authorization", HeaderValue::from_str(format!("Bearer {}", token).as_str())?);
    headers.insert("User-Agent", "reqwest".parse()?);

    let client = reqwest::Client::builder().default_headers(headers).build()?;
    let base_url = "https://api.github.com".to_string();

    Ok(GhClient { client, base_url })
  }

  pub async fn get_repos(&self, org: &str) -> WithError<Vec<Repo>> {
    let url = format!("{}/{}/repos?type=public,private&per_page=100", self.base_url, org);
    let rep = self.client.get(url).send().await?.error_for_status()?;
    let dat = rep.json::<Vec<Repo>>().await?;
    Ok(dat)
  }

  pub async fn traffic_clones(&self, repo: &str) -> WithError<TrafficClones> {
    let url = format!("{}/repos/{}/traffic/clones", self.base_url, repo);
    let rep = self.client.get(url).send().await?.error_for_status()?;
    let dat = rep.json::<TrafficClones>().await?;
    Ok(dat)
  }

  pub async fn traffic_views(&self, repo: &str) -> WithError<TrafficViews> {
    let url = format!("{}/repos/{}/traffic/views", self.base_url, repo);
    let rep = self.client.get(url).send().await?.error_for_status()?;
    let dat = rep.json::<TrafficViews>().await?;
    Ok(dat)
  }

  pub async fn traffic_popular_paths(&self, repo: &str) -> WithError<Vec<TrafficPath>> {
    let url = format!("{}/repos/{}/traffic/popular/paths", self.base_url, repo);
    let rep = self.client.get(url).send().await?.error_for_status()?;
    let dat = rep.json::<Vec<TrafficPath>>().await?;
    Ok(dat)
  }

  pub async fn traffic_referrers(&self, repo: &str) -> WithError<Vec<TrafficRefferer>> {
    let url = format!("{}/repos/{}/traffic/popular/referrers", self.base_url, repo);
    let rep = self.client.get(url).send().await?.error_for_status()?;
    let dat = rep.json::<Vec<TrafficRefferer>>().await?;
    Ok(dat)
  }
}
