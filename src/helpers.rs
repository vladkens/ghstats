use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::Request;
use chrono::DateTime;

use crate::db_client::DbClient;
use crate::gh_client::{GhClient, PullRequest, Repo, StarsWeek};
use crate::state::AppState;
use crate::types::Res;

pub fn get_header<'a>(req: &'a Request, name: &'a str) -> Option<&'a str> {
  match req.headers().get(name) {
    Some(x) => Some(x.to_str().unwrap_or_default()),
    None => None,
  }
}

async fn check_hidden_repos(db: &DbClient, repos: &[Repo]) -> Res {
  let now_ids = repos.iter().map(|r| r.id as i64).collect::<Vec<_>>();
  let was_ids = db.get_repos_ids().await?;
  let hidden = was_ids.into_iter().filter(|id| !now_ids.contains(id)).collect::<Vec<_>>();
  db.mark_repo_hidden(&hidden).await?;

  Ok(())
}

pub async fn update_metrics(state: Arc<AppState>) -> Res {
  state.record_sync_start();
  let result = update_metrics_inner(state.clone()).await;
  match &result {
    Ok(()) => state.record_sync_success(),
    Err(error) => state.record_sync_error(error),
  }
  result
}

async fn update_metrics_inner(state: Arc<AppState>) -> Res {
  let stime = std::time::Instant::now();

  let date = chrono::Utc::now().to_utc().to_rfc3339();
  let date = date.split("T").next().unwrap().to_owned() + "T00:00:00Z";

  // after downtime the star history is fetched again, so the gap gets the stars
  let yesterday = (chrono::Utc::now() - chrono::Duration::days(1)).date_naive();
  let was_down = state.db.get_last_stats_date().await?.is_some_and(|x| x < yesterday);

  let repos = state.gh.get_repos(state.include_private).await?;
  check_hidden_repos(&state.db, &repos).await?;

  let repos = repos //
    .iter()
    .filter(|r| state.filter.is_included(&r.full_name, r.fork, r.archived))
    .collect::<Vec<_>>();

  let progress = |done| state.record_sync_progress("Updating traffic", done, repos.len());
  progress(0);
  for (idx, repo) in repos.iter().enumerate() {
    if let Err(e) = update_repo_metrics(&state.db, &state.gh, repo, &date).await {
      tracing::warn!("failed to update metrics for {}: {:?}", repo.full_name, e);
    }

    progress(idx + 1);
  }

  tracing::info!("update_metrics took {:?} for {} repos", stime.elapsed(), repos.len());
  state.db.update_deltas().await?;
  if was_down {
    state.db.mark_repos_stars_unsynced().await?;
  }

  sync_stars(&state.db, &state.gh, |done, total| {
    state.record_sync_progress("Star history", done, total)
  })
  .await?;

  Ok(())
}

async fn get_open_pull_requests(gh: &GhClient, repo: &Repo) -> Res<Vec<PullRequest>> {
  if repo.has_pull_requests == Some(false) {
    return Ok(vec![]);
  }

  match gh.get_open_pull_requests(&repo.full_name).await {
    Err(e) if GhClient::error_status(&e) == Some(reqwest::StatusCode::NOT_FOUND) => {
      tracing::info!("pull requests not available for {}, counting 0", repo.full_name);
      Ok(vec![])
    }
    rs => rs,
  }
}

async fn update_repo_metrics(db: &DbClient, gh: &GhClient, repo: &Repo, date: &str) -> Res {
  let prs = get_open_pull_requests(gh, repo).await?;
  let views = gh.traffic_views(&repo.full_name).await?;
  let clones = gh.traffic_clones(&repo.full_name).await?;
  let referrers = gh.traffic_refs(&repo.full_name).await?;

  let popular_paths = gh.traffic_paths(&repo.full_name).await?;

  db.insert_repo(repo).await?;
  db.insert_stats(repo, date, &prs).await?;
  db.insert_views(repo, &views).await?;
  db.insert_clones(repo, &clones).await?;
  db.insert_referrers(repo, date, &referrers).await?;
  db.insert_paths(repo, date, &popular_paths).await?;

  // traffic is kept even when releases fail
  if let Err(e) = update_repo_releases(db, gh, repo, date).await {
    tracing::warn!("failed to update releases for {}: {:?}", repo.full_name, e);
  }

  Ok(())
}

async fn update_repo_releases(db: &DbClient, gh: &GhClient, repo: &Repo, date: &str) -> Res {
  let releases = gh.get_releases(&repo.full_name).await?;
  db.insert_release_assets(repo, date, &releases).await?;
  Ok(())
}

/// Daily stars from weekly history
/// vec![(date_str, acc_stars, new_stars), ...]
fn get_stars_history(weeks: &[StarsWeek]) -> Vec<(String, u32, u32)> {
  let mut dat: BTreeMap<String, u32> = BTreeMap::new();
  for week in weeks {
    for (idx, count) in week.days.iter().enumerate() {
      if *count == 0 {
        continue;
      }

      let Some(date) = DateTime::from_timestamp(week.week + idx as i64 * 86400, 0) else {
        continue;
      };

      let date = date.format("%Y-%m-%dT00:00:00Z").to_string(); // db stores dates as UTC midnight
      // a new week can start between page requests, so the same week may come twice
      dat.insert(date, *count);
    }
  }

  let mut acc_count = 0;
  dat
    .into_iter()
    .map(|(date, new_count)| {
      acc_count += new_count;
      (date, acc_count, new_count)
    })
    .collect()
}

pub async fn sync_stars(db: &DbClient, gh: &GhClient, progress: impl Fn(usize, usize)) -> Res {
  let mut requests_made = 0;

  let repos = db.repos_to_sync().await?;
  let total = repos.len();
  progress(0, total);
  for (idx, repo) in repos.into_iter().enumerate() {
    let stime = std::time::Instant::now();
    // tracing::info!("sync_stars for {}", repo.name);

    let (weeks, requests) = match gh.get_stars_weeks(&repo.name).await {
      Ok(rs) => rs,
      Err(e) => {
        tracing::warn!("failed to get stars for {}: {:?}", repo.name, e);
        break;
      }
    };

    let stars = get_stars_history(&weeks);
    db.insert_stars(repo.id, &stars).await?;
    db.mark_repo_stars_synced(repo.id).await?;
    progress(idx + 1, total);

    let stars_count = stars.iter().map(|(_, _, c)| c).sum::<u32>();
    tracing::info!(
      "sync_stars for {} done in {:?}, {stars_count} starts added",
      repo.name,
      stime.elapsed(),
    );

    // gh api rate limit is 5000 req/h, so this code will do up to 1000 req/h
    // to not block other possible user pipelines
    requests_made += requests;
    if requests_made > 1000 {
      tracing::info!("sync_stars: {} requests made, will continue next hour", requests_made);
      break;
    }
  }

  Ok(())
}

#[derive(Debug)]
pub struct GhsFilter {
  pub include_repos: Vec<String>,
  pub exclude_repos: Vec<String>,
  pub exclude_forks: bool,
  pub exclude_archs: bool,
  pub default_all: bool,
}

impl GhsFilter {
  pub fn new(rules: &str) -> Self {
    let mut default_all = false;
    let mut exclude_forks = false;
    let mut exclude_archs = false;
    let mut include_repos: Vec<&str> = Vec::new();
    let mut exclude_repos: Vec<&str> = Vec::new();

    let rules = rules.trim().to_lowercase();
    for rule in rules.split(",").map(|x| x.trim()) {
      if rule.is_empty() {
        continue;
      }

      if rule == "*" {
        default_all = true;
        continue;
      }

      if rule == "!fork" {
        exclude_forks = true;
        continue;
      }

      if rule == "!archived" {
        exclude_archs = true;
        continue;
      }

      if rule.matches('/').count() != 1 {
        continue;
      }

      if rule.starts_with('!') {
        exclude_repos.push(rule.strip_prefix('!').unwrap());
      } else {
        include_repos.push(rule);
      }
    }

    // if no repo rules, include all by default
    if exclude_repos.is_empty() && include_repos.is_empty() {
      default_all = true;
    }

    Self {
      include_repos: include_repos.into_iter().map(|x| x.to_string()).collect(),
      exclude_repos: exclude_repos.into_iter().map(|x| x.to_string()).collect(),
      exclude_forks,
      exclude_archs,
      default_all,
    }
  }

  pub fn is_included(&self, repo: &str, is_fork: bool, is_arch: bool) -> bool {
    let repo = repo.trim().to_lowercase();
    if repo.is_empty()
      || repo.matches('/').count() != 1
      || repo.starts_with('/')
      || repo.ends_with('/')
    {
      return false;
    }

    for (flag, rules) in [(false, &self.exclude_repos), (true, &self.include_repos)] {
      for rule in rules {
        if rule == &repo {
          return flag;
        }

        // skip wildcards for forks / archived
        if (self.exclude_forks && is_fork) || (self.exclude_archs && is_arch) {
          continue;
        }

        if rule.ends_with("/*")
          && repo.starts_with(&rule[..rule.len() - 2])
          && repo.chars().nth(rule.len() - 2) == Some('/')
        {
          return flag;
        }
      }
    }

    if self.exclude_forks && is_fork {
      return false;
    }

    if self.exclude_archs && is_arch {
      return false;
    }

    self.default_all
  }
}

#[cfg(test)]
mod tests {
  use std::collections::HashMap;
  use std::sync::Mutex;
  use std::time::{SystemTime, UNIX_EPOCH};

  use axum::extract::{Path, Query, State};
  use axum::http::StatusCode;
  use axum::response::{IntoResponse, Response};
  use axum::routing::get;
  use axum::{Json, Router};
  use serde_json::{Value, json};

  use super::*;

  #[test]
  fn test_empty_fitler() {
    let r = &GhsFilter::new("");

    assert!(r.is_included("foo/bar", false, false));
    assert!(r.is_included("foo/baz", false, false));
    assert!(r.is_included("abc/123", false, false));
    assert!(r.is_included("abc/xyz-123", false, false));

    // exclude invalid names
    assert!(!r.is_included("foo/", false, false));
    assert!(!r.is_included("/bar", false, false));
    assert!(!r.is_included("foo", false, false));
    assert!(!r.is_included("foo/bar/baz", false, false));

    // include forks / archived
    assert!(r.is_included("foo/bar", true, false));
    assert!(r.is_included("foo/bar", false, true));
    assert!(r.is_included("foo/bar", true, true));
  }

  #[test]
  fn test_filter_names() {
    let r = &GhsFilter::new("foo/*,abc/xyz");

    assert!(r.is_included("foo/bar", false, false));
    assert!(r.is_included("foo/123", false, false));
    assert!(r.is_included("abc/xyz", false, false));

    assert!(!r.is_included("foo/bar/baz", false, false));
    assert!(!r.is_included("abc/123", false, false));

    // include forks / archived
    assert!(r.is_included("foo/bar", true, false));
    assert!(r.is_included("foo/bar", false, true));

    // exact org/user match
    let r = &GhsFilter::new("foo/*");
    assert!(!r.is_included("fooo/bar", false, false));
  }

  #[test]
  fn test_filter_names_case() {
    let r = &GhsFilter::new("foo/*,abc/xyz");
    assert!(r.is_included("FOO/BAR", false, false));
    assert!(r.is_included("Foo/Bar", false, false));

    let r = &GhsFilter::new("FOO/*,Abc/XYZ");
    assert!(r.is_included("foo/bar", false, false));
    assert!(r.is_included("foo/baz", false, false));
    assert!(r.is_included("abc/xyz", false, false));
  }

  #[test]
  fn test_filter_all_expect() {
    let r = &GhsFilter::new("*");
    assert!(r.is_included("foo/bar", false, false));
    assert!(r.is_included("abc/123", false, false));
    assert!(r.is_included("abc/123", true, false));
    assert!(r.is_included("abc/123", true, true));

    let r = &GhsFilter::new("-*"); // single rule invalid, include all
    assert!(r.is_included("foo/bar", false, false));
    assert!(r.is_included("abc/123", false, false));

    let r = &GhsFilter::new("*,!foo/bar,!abc/123");
    assert!(!r.is_included("foo/bar", false, false));
    assert!(!r.is_included("abc/123", false, false));
    assert!(r.is_included("foo/baz", false, false));
    assert!(r.is_included("abc/xyz", false, false));

    let r = &GhsFilter::new("*,!foo/*");
    assert!(!r.is_included("foo/bar", false, false));
    assert!(!r.is_included("foo/baz", false, false));
    assert!(r.is_included("abc/123", false, false));
    assert!(r.is_included("abc/xyz", false, false));
  }

  #[test]
  fn test_filter_names_only() {
    let r = &GhsFilter::new("foo/*,!foo/bar");
    assert!(!r.is_included("abc/xyz", false, false));
    assert!(!r.is_included("foo/bar", false, false));
    assert!(!r.is_included("FOO/Bar", false, false));

    assert!(r.is_included("foo/abc", false, false));
    assert!(r.is_included("foo/abc", true, false));
    assert!(r.is_included("foo/abc", true, true));

    let r = &GhsFilter::new("foo/*,!foo/bar,!foo/baz,abc/xyz");
    assert!(!r.is_included("foo/bar", false, false));
    assert!(!r.is_included("foo/baz", false, false));
    assert!(!r.is_included("abc/123", false, false));

    assert!(r.is_included("foo/123", false, false));
    assert!(r.is_included("foo/123", true, false));
    assert!(r.is_included("foo/123", false, true));

    assert!(r.is_included("abc/xyz", false, false));
    assert!(r.is_included("abc/xyz", true, false));
    assert!(r.is_included("abc/xyz", false, true));
  }

  #[test]
  fn test_filter_meta() {
    let r = &GhsFilter::new("*,!fork,!archived,foo/baz");
    assert!(r.exclude_forks);
    assert!(r.exclude_archs);
    assert!(r.default_all);

    assert!(r.is_included("foo/bar", false, false));
    assert!(!r.is_included("foo/bar", true, false));
    assert!(!r.is_included("foo/bar", false, true));

    assert!(r.is_included("abc/123", false, false));
    assert!(!r.is_included("abc/123", true, false));
    assert!(!r.is_included("abc/123", false, true));

    // explicitly added
    assert!(r.is_included("foo/baz", false, false));
    assert!(r.is_included("foo/baz", true, false));
    assert!(r.is_included("foo/baz", false, true));
  }

  #[test]
  fn test_filter_meta_wildcard() {
    let r = &GhsFilter::new("!fork,abc/*,abc/xyz");
    assert!(!r.is_included("abc/123", true, false)); // no wildcard for forks
    assert!(r.is_included("abc/xyz", true, false)); // explicitly added

    let r = &GhsFilter::new("!archived,abc/*,abc/xyz");
    assert!(!r.is_included("abc/123", false, true)); // no wildcard for archived
    assert!(r.is_included("abc/xyz", false, true)); // explicitly added
  }

  #[test]
  fn test_issue18() {
    // test order of rules not affecting the result
    let rules = vec!["foo/*,!foo/bar", "!foo/bar,foo/*"];
    for r in rules {
      let r = &GhsFilter::new(r);
      assert!(!r.is_included("foo/bar", false, false)); // explicitly excluded
      assert!(!r.is_included("abc/abc", false, false)); // not included by default
      assert!(r.is_included("foo/baz", false, false)); // wildcard included
    }

    let rules = vec!["foo/*,!fork", "!fork,foo/*"];
    for r in rules {
      let r = &GhsFilter::new(r);
      assert!(r.is_included("foo/bar", false, false)); // wildcard included
      assert!(!r.is_included("foo/bar", true, false)); // forks excluded
      assert!(!r.is_included("abc/abc", false, false)); // not included by default
    }
  }

  const DATE: &str = "2024-01-01T00:00:00Z";

  #[derive(Clone, Default)]
  struct MockGh {
    pulls_calls: Arc<Mutex<Vec<String>>>,
    history_calls: Arc<Mutex<Vec<String>>>,
    releases_calls: Arc<Mutex<Vec<String>>>,
  }

  async fn pulls(
    State(gh): State<MockGh>,
    Path((owner, repo)): Path<(String, String)>,
  ) -> Response {
    gh.pulls_calls.lock().unwrap().push(format!("{owner}/{repo}"));

    match repo.as_str() {
      "pulls-404" => StatusCode::NOT_FOUND.into_response(),
      "pulls-500" => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
      _ => Json(json!([])).into_response(),
    }
  }

  fn daily() -> Value {
    json!([{ "timestamp": DATE, "uniques": 1, "count": 2 }])
  }

  async fn views() -> Json<Value> {
    Json(json!({ "uniques": 1, "count": 2, "views": daily() }))
  }

  async fn clones() -> Json<Value> {
    Json(json!({ "uniques": 1, "count": 2, "clones": daily() }))
  }

  async fn empty_list() -> Json<Value> {
    Json(json!([]))
  }

  const WEEK0: i64 = 1704585600; // Sun 2024-01-07
  const WEEK: i64 = 7 * 86400;

  fn stars_week(n: i64, days: [u32; 7]) -> Value {
    json!({ "week": WEEK0 + n * WEEK, "total": days.iter().sum::<u32>(), "days": days })
  }

  async fn stars_history(
    State(gh): State<MockGh>,
    Path((owner, repo)): Path<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
  ) -> Response {
    let (per_page, page) = (&query["per_page"], &query["page"]);
    gh.history_calls
      .lock()
      .unwrap()
      .push(format!("{owner}/{repo}?per_page={per_page}&page={page}"));

    // full page of 30 weeks newest to oldest, then a short page with the creation week
    let weeks = match page.as_str() {
      "1" => (1..=30)
        .rev()
        .map(|n| match n {
          30 => stars_week(n, [0, 2, 0, 0, 0, 0, 0]),
          1 => stars_week(n, [1, 0, 0, 0, 0, 0, 0]),
          _ => stars_week(n, [0; 7]),
        })
        .collect(),
      "2" => vec![stars_week(0, [0, 0, 0, 0, 0, 0, 3])],
      _ => vec![],
    };

    Json(Value::Array(weeks)).into_response()
  }

  fn release(id: u64, tag: &str, draft: bool, assets: &[(u64, &str, u64)]) -> Value {
    let assets = assets
      .iter()
      .map(|(id, name, count)| json!({ "id": id, "name": name, "download_count": count }))
      .collect::<Vec<_>>();

    let published_at = if draft { Value::Null } else { json!(format!("2024-01-0{id}T00:00:00Z")) };
    json!({
      "id": id, "tag_name": tag, "name": tag, "draft": draft, "prerelease": tag.contains("-rc"),
      "published_at": published_at, "assets": assets
    })
  }

  async fn releases(
    State(gh): State<MockGh>,
    Path((_, repo)): Path<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
  ) -> Response {
    let page = query.get("page").map(String::as_str);
    let calls = {
      let mut calls = gh.releases_calls.lock().unwrap();
      calls.push(format!("{repo}?page={}", page.unwrap_or_default()));
      calls.len() as u64
    };

    match (repo.as_str(), page) {
      ("releases", Some("1")) => {
        let items = json!([
          release(3, "v2.0", true, &[(30, "app-linux", 9)]),
          release(2, "v1.1-rc", false, &[(20, "app-linux", 5), (21, "app-mac", 3)]),
        ]);
        let mut rep = Json(items).into_response();
        let link = "<http://example.test/releases?page=2>; rel=\"next\"";
        rep.headers_mut().insert("link", link.parse().unwrap());
        rep
      }
      ("releases", Some("2")) => {
        // grows with every request, like a live download counter
        let assets = [(10, "app-linux", 40 + calls)];
        Json(json!([release(1, "v1.0", false, &assets)])).into_response()
      }
      ("releases-500", _) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
      _ => Json(json!([])).into_response(),
    }
  }

  async fn mock_server(gh: MockGh) -> Res<(String, tokio::task::JoinHandle<()>)> {
    let router = Router::new()
      .route("/repos/{owner}/{repo}/pulls", get(pulls))
      .route("/repos/{owner}/{repo}/traffic/views", get(views))
      .route("/repos/{owner}/{repo}/traffic/clones", get(clones))
      .route("/repos/{owner}/{repo}/traffic/popular/referrers", get(empty_list))
      .route("/repos/{owner}/{repo}/traffic/popular/paths", get(empty_list))
      .route("/repos/{owner}/{repo}/stargazers/history", get(stars_history))
      .route("/repos/{owner}/{repo}/releases", get(releases))
      .with_state(gh);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let handle = tokio::spawn(async move {
      axum::serve(listener, router).await.unwrap();
    });
    Ok((format!("http://{addr}"), handle))
  }

  async fn test_db(test_name: &str) -> Res<DbClient> {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let path =
      std::env::temp_dir().join(format!("ghstats-{test_name}-{}-{nanos}.db", std::process::id()));
    DbClient::new(&path.to_string_lossy()).await
  }

  fn sample_repo(full_name: &str, has_pull_requests: Option<bool>) -> Repo {
    Repo {
      id: 1,
      full_name: full_name.to_string(),
      description: None,
      stargazers_count: 1,
      forks_count: 0,
      watchers_count: 0,
      open_issues_count: 3,
      fork: false,
      archived: false,
      has_pull_requests,
      ..Default::default()
    }
  }

  #[tokio::test]
  async fn repo_with_pull_requests_disabled_is_saved_without_calling_pulls() -> Res {
    let mock = MockGh::default();
    let (base_url, server) = mock_server(mock.clone()).await?;
    let gh = GhClient::for_test(base_url)?;
    let db = test_db("prs-disabled").await?;
    let repo = sample_repo("owner/prs-disabled", Some(false));

    update_repo_metrics(&db, &gh, &repo, DATE).await?;

    let totals = db.get_repo_totals("owner/prs-disabled").await?.unwrap();
    assert_eq!(totals.prs, 0);
    assert_eq!(totals.issues, 3);
    assert_eq!(totals.views_count, 2);
    assert!(mock.pulls_calls.lock().unwrap().is_empty());
    assert!(db.get_repos_downloads(0).await?.is_empty()); // no releases, nothing stored

    server.abort();
    Ok(())
  }

  #[tokio::test]
  async fn repo_with_pulls_not_found_is_saved_with_zero_prs() -> Res {
    let mock = MockGh::default();
    let (base_url, server) = mock_server(mock.clone()).await?;
    let gh = GhClient::for_test(base_url)?;
    let db = test_db("pulls-404").await?;
    let repo = sample_repo("owner/pulls-404", None);

    update_repo_metrics(&db, &gh, &repo, DATE).await?;

    let totals = db.get_repo_totals("owner/pulls-404").await?.unwrap();
    assert_eq!(totals.prs, 0);
    assert_eq!(totals.issues, 3);
    assert_eq!(totals.views_count, 2);
    assert_eq!(*mock.pulls_calls.lock().unwrap(), vec!["owner/pulls-404".to_string()]);

    server.abort();
    Ok(())
  }

  #[tokio::test]
  async fn repo_with_other_pulls_error_is_still_skipped() -> Res {
    let (base_url, server) = mock_server(MockGh::default()).await?;
    let gh = GhClient::for_test(base_url)?;
    let db = test_db("pulls-500").await?;
    let repo = sample_repo("owner/pulls-500", None);

    assert!(update_repo_metrics(&db, &gh, &repo, DATE).await.is_err());
    assert!(db.get_repo_totals("owner/pulls-500").await?.is_none());

    server.abort();
    Ok(())
  }

  #[tokio::test]
  async fn repo_releases_are_stored_every_sync_without_drafts() -> Res {
    let mock = MockGh::default();
    let (base_url, server) = mock_server(mock.clone()).await?;
    let gh = GhClient::for_test(base_url)?;
    let db = test_db("releases").await?;
    let repo = sample_repo("owner/releases", None);

    update_repo_metrics(&db, &gh, &repo, DATE).await?;
    update_repo_metrics(&db, &gh, &repo, DATE).await?; // same day, raises today's row
    assert_eq!(
      *mock.releases_calls.lock().unwrap(),
      vec!["releases?page=1", "releases?page=2", "releases?page=1", "releases?page=2"]
    );

    // no created_at in the mock, so published_at (day of or before the first snapshot) decides
    let rels = db.get_release_downloads("owner/releases", 0).await?;
    let rels = rels.iter().map(|x| (x.tag.as_str(), x.total, x.period)).collect::<Vec<_>>();
    assert_eq!(rels, vec![("v1.1-rc", 8, 8), ("v1.0", 44, 44)]);

    let assets = db.get_asset_downloads("owner/releases", 0).await?;
    let assets = assets.iter().map(|x| (x.name.as_str(), x.total)).collect::<Vec<_>>();
    assert_eq!(assets, vec![("app-linux", 49), ("app-mac", 3)]);

    update_repo_metrics(&db, &gh, &repo, "2024-01-02T00:00:00Z").await?; // next day, new row
    let daily = db.get_downloads_daily("owner/releases").await?;
    let daily = daily.iter().map(|x| (x.date.as_str(), x.downloads)).collect::<Vec<_>>();
    assert_eq!(daily, vec![(DATE, 52), ("2024-01-02T00:00:00Z", 2)]);

    server.abort();
    Ok(())
  }

  #[tokio::test]
  async fn repo_with_releases_error_still_saves_traffic() -> Res {
    let mock = MockGh::default();
    let (base_url, server) = mock_server(mock.clone()).await?;
    let gh = GhClient::for_test(base_url)?;
    let db = test_db("releases-500").await?;
    let repo = sample_repo("owner/releases-500", None);

    update_repo_metrics(&db, &gh, &repo, DATE).await?;

    let totals = db.get_repo_totals("owner/releases-500").await?.unwrap();
    assert_eq!(totals.issues, 3);
    assert_eq!(totals.views_count, 2);
    assert!(db.get_repos_downloads(0).await?.is_empty());

    // a failed fetch is retried on the next sync of the same day
    update_repo_metrics(&db, &gh, &repo, DATE).await?;
    assert_eq!(mock.releases_calls.lock().unwrap().len(), 2);

    server.abort();
    Ok(())
  }

  #[test]
  fn stars_history_from_weeks() {
    let weeks = vec![
      // newest page first, weeks within a page newest to oldest
      StarsWeek { week: WEEK0 + 2 * WEEK, days: vec![0; 7] },
      StarsWeek { week: WEEK0 + WEEK, days: vec![2, 0, 0, 0, 0, 0, 1] },
      StarsWeek { week: WEEK0, days: vec![0, 0, 0, 0, 0, 0, 4] },
    ];

    assert_eq!(
      get_stars_history(&weeks),
      vec![
        ("2024-01-13T00:00:00Z".to_string(), 4, 4),
        ("2024-01-14T00:00:00Z".to_string(), 6, 2),
        ("2024-01-20T00:00:00Z".to_string(), 7, 1),
      ]
    );
    assert!(get_stars_history(&weeks[..1]).is_empty());

    let mut repeated = weeks.clone();
    repeated.insert(2, StarsWeek { week: WEEK0 + WEEK, days: vec![2, 0, 0, 0, 0, 0, 1] });
    assert_eq!(get_stars_history(&repeated), get_stars_history(&weeks));
  }

  #[tokio::test]
  async fn sync_stars_stores_history_and_marks_repo_synced() -> Res {
    let mock = MockGh::default();
    let (base_url, server) = mock_server(mock.clone()).await?;
    let gh = GhClient::for_test(base_url)?;
    let db = test_db("stars-ok").await?;
    db.insert_repo(&sample_repo("owner/stars-ok", None)).await?;

    sync_stars(&db, &gh, |_, _| {}).await?;

    assert_eq!(
      *mock.history_calls.lock().unwrap(),
      vec!["owner/stars-ok?per_page=30&page=1", "owner/stars-ok?per_page=30&page=2"]
    );
    assert!(db.repos_to_sync().await?.is_empty());

    let stars = db.get_stars("owner/stars-ok").await?;
    let stars = stars.iter().map(|s| (s.date.as_str(), s.stars)).collect::<Vec<_>>();
    assert_eq!(
      stars,
      vec![("2024-01-13T00:00:00Z", 3), ("2024-01-14T00:00:00Z", 4), ("2024-08-05T00:00:00Z", 6)]
    );

    server.abort();
    Ok(())
  }
}
