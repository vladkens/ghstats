use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use badgelib::{Badge, Color};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::Deserialize;

use crate::AppState;
use crate::db_client::{RepoFilter, RepoMetrics, RepoStars, RepoTotals};
use crate::types::AppError;

#[derive(Clone, Copy)]
enum Metric {
  Views,
  Clones,
  Stars,
  StarsGrowth,
  StarsVelocity,
  TrafficPeak,
  Repos,
  Health,
  Updated,
  Tracking,
}

impl Metric {
  fn from_path(value: &str) -> Option<Self> {
    match value {
      "views.svg" => Some(Self::Views),
      "clones.svg" => Some(Self::Clones),
      "stars.svg" => Some(Self::Stars),
      "stars-growth.svg" => Some(Self::StarsGrowth),
      "stars-velocity.svg" => Some(Self::StarsVelocity),
      "traffic-peak.svg" => Some(Self::TrafficPeak),
      "repos.svg" => Some(Self::Repos),
      "health.svg" => Some(Self::Health),
      "updated.svg" => Some(Self::Updated),
      "tracking.svg" => Some(Self::Tracking),
      _ => None,
    }
  }

  fn supports_repo(self) -> bool {
    matches!(
      self,
      Self::Views
        | Self::Clones
        | Self::Stars
        | Self::StarsGrowth
        | Self::StarsVelocity
        | Self::TrafficPeak
    )
  }

  fn period(self, value: Option<&str>) -> Result<Option<Period>, AppError> {
    let default = match self {
      Self::Views | Self::Clones => Some(Period::Tracked),
      Self::StarsGrowth | Self::StarsVelocity | Self::TrafficPeak => Some(Period::Days(30)),
      _ => None,
    };

    match (default, value) {
      (None, None) => Ok(None),
      (None, Some(_)) => Err(AppError::status(StatusCode::BAD_REQUEST)),
      (Some(default), None) => Ok(Some(default)),
      (Some(_), Some(value)) => Period::parse(value).map(Some),
    }
  }
}

#[derive(Clone, Copy)]
enum Period {
  Days(i64),
  Tracked,
}

impl Period {
  fn parse(value: &str) -> Result<Self, AppError> {
    match value {
      "7d" => Ok(Self::Days(7)),
      "30d" => Ok(Self::Days(30)),
      "90d" => Ok(Self::Days(90)),
      "tracked" => Ok(Self::Tracked),
      _ => Err(AppError::status(StatusCode::BAD_REQUEST)),
    }
  }

  fn cutoff(self) -> Option<NaiveDate> {
    match self {
      Self::Days(days) => Some(Utc::now().date_naive() - Duration::days(days - 1)),
      Self::Tracked => None,
    }
  }

  fn suffix(self) -> String {
    match self {
      Self::Days(days) => format!(" {days}d"),
      Self::Tracked => String::new(),
    }
  }
}

#[derive(Default, Deserialize)]
pub struct BadgeQuery {
  period: Option<String>,
}

struct BadgeScope {
  repos: Vec<RepoTotals>,
  total: bool,
}

impl BadgeScope {
  fn label(&self, value: &str) -> String {
    if self.total { format!("total {value}") } else { value.to_string() }
  }
}

enum TrafficKind {
  Views,
  Clones,
}

struct StarTrend {
  delta: i64,
  days: i64,
}

pub async fn total_badge(
  State(state): State<Arc<AppState>>,
  Path(metric): Path<String>,
  Query(query): Query<BadgeQuery>,
) -> Result<Response, AppError> {
  let metric = Metric::from_path(&metric).ok_or_else(|| AppError::status(StatusCode::NOT_FOUND))?;
  let repos = state.get_repos_filtered(&RepoFilter::default()).await?;
  render_badge(&state, BadgeScope { repos, total: true }, metric, query).await
}

pub async fn repo_badge(
  State(state): State<Arc<AppState>>,
  Path((owner, repo, metric)): Path<(String, String, String)>,
  Query(query): Query<BadgeQuery>,
) -> Result<Response, AppError> {
  let metric = Metric::from_path(&metric).ok_or_else(|| AppError::status(StatusCode::NOT_FOUND))?;
  if !metric.supports_repo() {
    return Err(AppError::status(StatusCode::NOT_FOUND));
  }

  let repo = format!("{owner}/{repo}");
  let totals = state
    .db
    .get_repo_totals(&repo)
    .await?
    .filter(|totals| state.filter.is_included(&totals.name, totals.fork, totals.archived))
    .ok_or_else(|| AppError::status(StatusCode::NOT_FOUND))?;

  render_badge(&state, BadgeScope { repos: vec![totals], total: false }, metric, query).await
}

async fn render_badge(
  state: &Arc<AppState>,
  scope: BadgeScope,
  metric: Metric,
  query: BadgeQuery,
) -> Result<Response, AppError> {
  let period = metric.period(query.period.as_deref())?;
  let badge = match metric {
    Metric::Views => {
      let period = period.unwrap();
      let value = traffic_total(state, &scope.repos, period, TrafficKind::Views).await?;
      Badge::new().for_count(&period_label(&scope, "views", period), value.max(0) as u64)
    }
    Metric::Clones => {
      let period = period.unwrap();
      let value = traffic_total(state, &scope.repos, period, TrafficKind::Clones).await?;
      Badge::new().for_count(&period_label(&scope, "clones", period), value.max(0) as u64)
    }
    Metric::Stars => {
      let value = scope.repos.iter().map(|repo| i64::from(repo.stars)).sum::<i64>();
      Badge::new().for_count(&scope.label("stars"), value.max(0) as u64)
    }
    Metric::StarsGrowth => {
      let period = period.unwrap();
      let trend = star_trend(state, &scope.repos, period).await?;
      signed_badge(&period_label(&scope, "stars", period), trend.delta)
    }
    Metric::StarsVelocity => {
      let period = period.unwrap();
      let trend = star_trend(state, &scope.repos, period).await?;
      let velocity = trend.delta as f64 / trend.days.max(1) as f64;
      decimal_badge(&period_label(&scope, "stars velocity", period), velocity, "/day")
    }
    Metric::TrafficPeak => {
      let period = period.unwrap();
      let value = traffic_peak(state, &scope.repos, period).await?;
      Badge::new()
        .label(&period_label(&scope, "traffic peak", period))
        .value(&format!("{}/day", compact_number(value.max(0) as u64)))
        .value_color(Color::Blue)
    }
    Metric::Repos => Badge::new().for_count("repos", scope.repos.len() as u64),
    Metric::Health => health_badge(state),
    Metric::Updated => updated_badge(state),
    Metric::Tracking => tracking_badge(state, &scope.repos).await?,
  };

  Ok(badge.into_response())
}

fn period_label(scope: &BadgeScope, label: &str, period: Period) -> String {
  format!("{}{}", scope.label(label), period.suffix())
}

async fn traffic_total(
  state: &Arc<AppState>,
  repos: &[RepoTotals],
  period: Period,
  kind: TrafficKind,
) -> anyhow::Result<i64> {
  if matches!(period, Period::Tracked) {
    return Ok(
      repos
        .iter()
        .map(|repo| match kind {
          TrafficKind::Views => i64::from(repo.views_count),
          TrafficKind::Clones => i64::from(repo.clones_count),
        })
        .sum(),
    );
  }

  let mut total = 0_i64;
  for repo in repos {
    for metric in state.db.get_metrics(&repo.name).await? {
      if in_period(&metric.date, period) {
        total += match kind {
          TrafficKind::Views => i64::from(metric.views_count),
          TrafficKind::Clones => i64::from(metric.clones_count),
        };
      }
    }
  }
  Ok(total)
}

async fn traffic_peak(
  state: &Arc<AppState>,
  repos: &[RepoTotals],
  period: Period,
) -> anyhow::Result<i64> {
  let mut daily = BTreeMap::<NaiveDate, i64>::new();
  for repo in repos {
    for metric in state.db.get_metrics(&repo.name).await? {
      if let Some(date) = parse_date(&metric.date)
        && in_period_date(date, period)
      {
        *daily.entry(date).or_default() += i64::from(metric.views_count);
      }
    }
  }
  Ok(daily.into_values().max().unwrap_or_default())
}

async fn star_trend(
  state: &Arc<AppState>,
  repos: &[RepoTotals],
  period: Period,
) -> anyhow::Result<StarTrend> {
  let cutoff = period.cutoff();
  let mut delta = 0_i64;
  let mut first_date = None::<NaiveDate>;
  let mut last_date = None::<NaiveDate>;

  for repo in repos {
    let history = dated_stars(state.db.get_stars(&repo.name).await?);
    let Some(last) = history.last() else {
      continue;
    };

    let first = match cutoff {
      Some(cutoff) => history
        .iter()
        .rev()
        .find(|(date, _)| *date <= cutoff)
        .or_else(|| history.iter().find(|(date, _)| *date >= cutoff)),
      None => history.first(),
    };
    let Some(first) = first else {
      continue;
    };

    let effective_first = cutoff.map_or(first.0, |cutoff| first.0.max(cutoff));
    if last.0 < effective_first {
      continue;
    }

    delta += i64::from(last.1) - i64::from(first.1);
    first_date = Some(first_date.map_or(effective_first, |date| date.min(effective_first)));
    last_date = Some(last_date.map_or(last.0, |date| date.max(last.0)));
  }

  let days = match (first_date, last_date) {
    (Some(first), Some(last)) => (last - first).num_days() + 1,
    _ => 1,
  };
  Ok(StarTrend { delta, days })
}

fn dated_stars(stars: Vec<RepoStars>) -> Vec<(NaiveDate, i32)> {
  stars
    .into_iter()
    .filter_map(|star| parse_date(&star.date).map(|date| (date, star.stars)))
    .collect()
}

fn in_period(value: &str, period: Period) -> bool {
  parse_date(value).is_some_and(|date| in_period_date(date, period))
}

fn in_period_date(date: NaiveDate, period: Period) -> bool {
  period.cutoff().is_none_or(|cutoff| date >= cutoff)
}

fn parse_date(value: &str) -> Option<NaiveDate> {
  NaiveDate::parse_from_str(value.get(..10)?, "%Y-%m-%d").ok()
}

fn signed_badge(label: &str, value: i64) -> Badge {
  Badge::new().label(label).value(&format_signed(value)).value_color(trend_color(value as f64))
}

fn decimal_badge(label: &str, value: f64, suffix: &str) -> Badge {
  let sign = if value > 0.0 { "+" } else { "" };
  Badge::new()
    .label(label)
    .value(&format!("{sign}{value:.1}{suffix}"))
    .value_color(trend_color(value))
}

fn trend_color(value: f64) -> Color {
  if value > 0.0 {
    Color::Green
  } else if value < 0.0 {
    Color::Red
  } else {
    Color::Gray
  }
}

fn format_signed(value: i64) -> String {
  let sign = if value > 0 {
    "+"
  } else if value < 0 {
    "−"
  } else {
    ""
  };
  format!("{sign}{}", compact_number(value.unsigned_abs()))
}

fn compact_number(value: u64) -> String {
  let mut value = value as f64;
  let mut unit = 0;
  let units = ["", "k", "M", "B", "T"];
  while value >= 1_000.0 && unit < units.len() - 1 {
    value /= 1_000.0;
    unit += 1;
  }

  let value = if value >= 100.0 { format!("{value:.0}") } else { format!("{value:.1}") };
  format!("{}{}", value.strip_suffix(".0").unwrap_or(&value), units[unit])
}

fn health_badge(state: &Arc<AppState>) -> Badge {
  let healthy = state.db_health().result.is_ok() && state.sync_status().last_error.is_none();
  Badge::new()
    .label("ghstats")
    .value(if healthy { "healthy" } else { "degraded" })
    .value_color(if healthy { Color::Green } else { Color::Red })
}

fn updated_badge(state: &Arc<AppState>) -> Badge {
  let status = state.sync_status();
  match status.last_success {
    Some(updated) => Badge::new()
      .label("updated")
      .value(&format_age(updated))
      .value_color(if status.last_error.is_some() { Color::Red } else { Color::Green }),
    None => Badge::new().label("updated").value("never").value_color(Color::Gray),
  }
}

async fn tracking_badge(state: &Arc<AppState>, repos: &[RepoTotals]) -> anyhow::Result<Badge> {
  let mut earliest = None::<NaiveDate>;
  for repo in repos {
    let metrics = state.db.get_metrics(&repo.name).await?;
    let stars = state.db.get_stars(&repo.name).await?;
    for date in metrics
      .iter()
      .filter_map(|metric: &RepoMetrics| parse_date(&metric.date))
      .chain(stars.iter().filter_map(|star| parse_date(&star.date)))
    {
      earliest = Some(earliest.map_or(date, |current| current.min(date)));
    }
  }

  Ok(match earliest {
    Some(date) => Badge::new()
      .label("tracking")
      .value(&format_days((Utc::now().date_naive() - date).num_days()))
      .value_color(Color::Blue),
    None => Badge::new().label("tracking").value("no data").value_color(Color::Gray),
  })
}

fn format_age(value: DateTime<Utc>) -> String {
  let seconds = Utc::now().signed_duration_since(value).num_seconds().max(0);
  match seconds {
    0..=59 => "just now".to_string(),
    60..=3_599 => format!("{} min ago", seconds / 60),
    3_600..=86_399 => format!("{} hr ago", seconds / 3_600),
    86_400..=172_799 => "1 day ago".to_string(),
    _ => format!("{} days ago", seconds / 86_400),
  }
}

fn format_days(days: i64) -> String {
  match days.max(0) {
    0 => "today".to_string(),
    1 => "1 day".to_string(),
    2..=59 => format!("{days} days"),
    60..=729 => format!("{} months", days / 30),
    _ => format!("{} years", days / 365),
  }
}

#[cfg(test)]
mod tests {
  use std::time::{SystemTime, UNIX_EPOCH};

  use axum::body::to_bytes;
  use axum::http::header;

  use super::*;
  use crate::db_client::DbClient;
  use crate::gh_client::{Repo, RepoClones, RepoViews, TrafficDaily};
  use crate::helpers::GhsFilter;

  fn db_path(test_name: &str) -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    std::env::temp_dir()
      .join(format!("ghstats-badge-{test_name}-{}-{nanos}.db", std::process::id()))
      .to_string_lossy()
      .into_owned()
  }

  async fn test_state(test_name: &str, filter: &str) -> Arc<AppState> {
    let db = DbClient::new(&db_path(test_name)).await.unwrap();
    Arc::new(AppState::for_test(db, GhsFilter::new(filter)))
  }

  fn sample_repo(id: u64, name: &str, stars: u32) -> Repo {
    Repo {
      id,
      full_name: name.to_string(),
      description: None,
      stargazers_count: stars,
      forks_count: 0,
      watchers_count: 0,
      open_issues_count: 0,
      fork: false,
      archived: false,
    }
  }

  fn date(days_ago: i64) -> String {
    let date = Utc::now().date_naive() - Duration::days(days_ago);
    format!("{date}T00:00:00Z")
  }

  fn daily(days_ago: i64, count: u32) -> TrafficDaily {
    TrafficDaily { timestamp: date(days_ago), uniques: count, count }
  }

  async fn seed_repo(state: &Arc<AppState>, id: u64, name: &str) {
    let repo = sample_repo(id, name, 30);
    state.db.insert_repo(&repo).await.unwrap();

    for (days_ago, stars) in [(40, 10), (5, 20), (0, 30)] {
      state.db.insert_stats(&sample_repo(id, name, stars), &date(days_ago), &[]).await.unwrap();
    }

    state
      .db
      .insert_views(
        &repo,
        &RepoViews {
          uniques: 0,
          count: 0,
          views: vec![daily(40, 100), daily(5, 20), daily(0, 30)],
        },
      )
      .await
      .unwrap();
    state
      .db
      .insert_clones(
        &repo,
        &RepoClones { uniques: 0, count: 0, clones: vec![daily(40, 10), daily(5, 4), daily(0, 6)] },
      )
      .await
      .unwrap();
  }

  async fn repo_response(
    state: Arc<AppState>,
    owner: &str,
    repo: &str,
    metric: &str,
    period: Option<&str>,
  ) -> Result<Response, AppError> {
    repo_badge(
      State(state),
      Path((owner.to_string(), repo.to_string(), format!("{metric}.svg"))),
      Query(BadgeQuery { period: period.map(str::to_string) }),
    )
    .await
  }

  async fn total_response(
    state: Arc<AppState>,
    metric: &str,
    period: Option<&str>,
  ) -> Result<Response, AppError> {
    total_badge(
      State(state),
      Path(format!("{metric}.svg")),
      Query(BadgeQuery { period: period.map(str::to_string) }),
    )
    .await
  }

  async fn svg_text(response: Response) -> String {
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(body.to_vec()).unwrap()
  }

  #[tokio::test]
  async fn renders_repo_counts_with_default_cache_and_periods() {
    let state = test_state("repo-counts", "").await;
    seed_repo(&state, 1, "owner/repo").await;

    let response = repo_response(state.clone(), "owner", "repo", "views", None).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "image/svg+xml");
    assert_eq!(
      response.headers()[header::CACHE_CONTROL],
      "public,max-age=0,s-maxage=300,stale-while-revalidate=0"
    );
    let svg = svg_text(response).await;
    assert!(svg.contains(">views<"));
    assert!(svg.contains(">150<"));

    let response =
      repo_response(state.clone(), "owner", "repo", "views", Some("30d")).await.unwrap();
    let svg = svg_text(response).await;
    assert!(svg.contains(">views 30d<"));
    assert!(svg.contains(">50<"));

    let response = repo_response(state, "owner", "repo", "clones", None).await.unwrap();
    let svg = svg_text(response).await;
    assert!(svg.contains(">clones<"));
    assert!(svg.contains(">20<"));
  }

  #[tokio::test]
  async fn renders_repo_star_trends_and_traffic_peak() {
    let state = test_state("repo-trends", "").await;
    seed_repo(&state, 1, "owner/repo").await;

    let response = repo_response(state.clone(), "owner", "repo", "stars", None).await.unwrap();
    let svg = svg_text(response).await;
    assert!(svg.contains(">stars<"));
    assert!(svg.contains(">30<"));

    let response =
      repo_response(state.clone(), "owner", "repo", "stars-growth", None).await.unwrap();
    let svg = svg_text(response).await;
    assert!(svg.contains(">stars 30d<"));
    assert!(svg.contains(">+20<"));

    let response =
      repo_response(state.clone(), "owner", "repo", "stars-velocity", None).await.unwrap();
    let svg = svg_text(response).await;
    assert!(svg.contains(">stars velocity 30d<"));
    assert!(svg.contains(">+0.7/day<"));

    let response = repo_response(state, "owner", "repo", "traffic-peak", None).await.unwrap();
    let svg = svg_text(response).await;
    assert!(svg.contains(">traffic peak 30d<"));
    assert!(svg.contains(">30/day<"));
  }

  #[tokio::test]
  async fn aggregates_only_repositories_allowed_by_filter() {
    let state = test_state("aggregate-filter", "owner/repo").await;
    seed_repo(&state, 1, "owner/repo").await;
    seed_repo(&state, 2, "other/hidden").await;

    let response = total_response(state.clone(), "views", None).await.unwrap();
    let svg = svg_text(response).await;
    assert!(svg.contains(">total views<"));
    assert!(svg.contains(">150<"));

    let response = total_response(state.clone(), "repos", None).await.unwrap();
    assert!(svg_text(response).await.contains(">1<"));

    let response = total_response(state.clone(), "stars-growth", None).await.unwrap();
    assert!(svg_text(response).await.contains(">+20<"));

    let response = total_response(state.clone(), "traffic-peak", None).await.unwrap();
    assert!(svg_text(response).await.contains(">30/day<"));

    let error = repo_response(state, "other", "hidden", "views", None).await.unwrap_err();
    assert_eq!(error.into_response().status(), StatusCode::NOT_FOUND);
  }

  #[tokio::test]
  async fn renders_service_badges_and_rejects_invalid_periods() {
    let state = test_state("service", "").await;
    seed_repo(&state, 1, "owner/repo").await;
    state.record_sync_success();

    let response = total_response(state.clone(), "health", None).await.unwrap();
    assert!(svg_text(response).await.contains(">healthy<"));

    let response = total_response(state.clone(), "updated", None).await.unwrap();
    assert!(svg_text(response).await.contains(">just now<"));

    let response = total_response(state.clone(), "tracking", None).await.unwrap();
    assert!(svg_text(response).await.contains(">40 days<"));

    state.record_sync_error(&anyhow::anyhow!("sync failed"));
    let response = total_response(state.clone(), "health", None).await.unwrap();
    assert!(svg_text(response).await.contains(">degraded<"));

    let error = total_response(state.clone(), "views", Some("year")).await.unwrap_err();
    assert_eq!(error.into_response().status(), StatusCode::BAD_REQUEST);

    let error = total_response(state, "stars", Some("30d")).await.unwrap_err();
    assert_eq!(error.into_response().status(), StatusCode::BAD_REQUEST);
  }
}
