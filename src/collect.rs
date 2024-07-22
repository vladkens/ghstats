use axum::routing::head;
use reqwest::header::{HeaderMap, HeaderValue};

pub type WithError<T> = Result<T, Box<dyn std::error::Error + Sync + Send>>;

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct Repo {
  full_name: String,
  stargazers_count: u32,
  forks_count: u32,
  watchers_count: u32,
  open_issues_count: u32,
}

pub struct ApiClient {
  client: reqwest::Client,
  base_url: String,
}

impl ApiClient {
  pub fn new() -> WithError<ApiClient> {
    let token = std::env::var("GITHUB_TOKEN")?;

    let mut headers = HeaderMap::new();
    headers.insert("Accept", HeaderValue::from_static("application/vnd.github+json"));
    headers.insert("X-GitHub-Api-Version", HeaderValue::from_static("2022-11-28"));
    headers.insert("Authorization", HeaderValue::from_str(format!("Bearer {}", token).as_str())?);
    headers.insert("User-Agent", "reqwest".parse()?);

    let client = reqwest::Client::builder().default_headers(headers).build()?;
    let base_url = "https://api.github.com".to_string();

    Ok(ApiClient { client, base_url })
  }

  pub async fn get_repos(&self, org: &str) -> WithError<Vec<Repo>> {
    let url = format!("{}/{}/repos?type=public,private&per_page=100", self.base_url, org);
    let rep = self.client.get(url).send().await?.error_for_status()?;
    let dat = rep.json::<Vec<Repo>>().await?;
    Ok(dat)
  }
}
