use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::{NaiveDate, Utc};
use maud::{Markup, PreEscaped, Render, html};
use serde::Deserialize;
use thousands::Separable;

use super::badge::Period;
use crate::AppState;
use crate::db_client::{self, RepoFilter, RepoPeriod, RepoTotals};
use crate::types::{AppError, Res};

const FONT: &str = "system-ui, -apple-system, 'Segoe UI', Roboto, Helvetica, Arial, sans-serif";

const W: i32 = 420;
const PAD: i32 = 16;
const INSET: i32 = 8; // text inside a row stays this far from the bar's ends
const ROWS_Y: i32 = 42;
const ROW_H: i32 = 26;
const ROW_GAP: i32 = 3;
const BASELINE: i32 = 18; // text baseline inside a row
const TEXT_L: i32 = PAD + INSET;
const TEXT_R: i32 = W - PAD - INSET;
const VALUE_R: i32 = TEXT_R - 52; // value column when the card has deltas
const BAR_MAX: i32 = W - 2 * PAD;
const BAR_MIN: i32 = 3;
const BAR_OPACITY: f32 = 0.12;
const HEAD_WEIGHT: Option<u32> = Some(600);
const NAME_MAX_DELTA: usize = 30; // chars, "…" included
const NAME_MAX_PLAIN: usize = 36;
const TOP: usize = 5;

const TOTALS_H: i32 = 108;
const TOTALS_H_PLAIN: i32 = 90; // without the delta row
const TOTALS_COL: f64 = (W - 2 * PAD) as f64 / 5.0;
const TOTALS_LABEL_Y: i32 = 52;
const TOTALS_VALUE_Y: i32 = 75;
const TOTALS_DELTA_Y: i32 = 93;

// the wordmark sits behind the rows, cut off by the bottom edge
const MARK_SIZE: f32 = 64.0;
const MARK_RIGHT: i32 = 10;
const MARK_DROP: i32 = 8;

// MARK: Types

struct Palette {
  card: &'static str,
  border: &'static str,
  head: &'static str,
  ink: &'static str,
  muted: &'static str,
  good: &'static str,
  bad: &'static str,
  stars: &'static str,
  forks: &'static str,
  clones: &'static str,
  views: &'static str,
  mark: f32, // about the same contrast to the card in both themes
}

const LIGHT: Palette = Palette {
  card: "#ffffff",
  border: "#e3e6ec",
  head: "#24292f",
  ink: "#373c44",
  muted: "#646b79",
  good: "#0f7a2a",
  bad: "#c23a3a",
  stars: "#2f95dc",
  forks: "#1baf7a",
  clones: "#eb6834",
  views: "#7c6ff0",
  mark: 0.05,
};

const DARK: Palette = Palette {
  card: "#181c25",
  border: "#262d3b",
  head: "#f0f1f3",
  ink: "#c2c7d0",
  muted: "#8891a4",
  good: "#3fbf5a",
  bad: "#ef7676",
  stars: "#4fb0f0",
  forks: "#22c48a",
  clones: "#f07a45",
  views: "#9a8cf0",
  mark: 0.04,
};

#[derive(Clone, Copy)]
enum Card {
  Stars,
  Forks,
  Clones,
  Views,
  Totals,
}

impl Card {
  fn from_path(value: &str) -> Option<Self> {
    match value {
      "stars.svg" => Some(Self::Stars),
      "forks.svg" => Some(Self::Forks),
      "clones.svg" => Some(Self::Clones),
      "views.svg" => Some(Self::Views),
      "totals.svg" => Some(Self::Totals),
      _ => None,
    }
  }

  fn title(self) -> &'static str {
    match self {
      Self::Stars => "Top stars",
      Self::Forks => "Top forks",
      Self::Clones => "Top clones",
      Self::Views => "Top views",
      Self::Totals => "Totals",
    }
  }

  fn bar(self, palette: &Palette) -> Option<&'static str> {
    match self {
      Self::Stars => Some(palette.stars),
      Self::Forks => Some(palette.forks),
      Self::Clones => Some(palette.clones),
      Self::Views => Some(palette.views),
      Self::Totals => None,
    }
  }
}

#[derive(Clone, Copy)]
enum Theme {
  Auto,
  Light,
  Dark,
}

impl Theme {
  fn parse(value: &str) -> Result<Self, AppError> {
    match value {
      "auto" => Ok(Self::Auto),
      "light" => Ok(Self::Light),
      "dark" => Ok(Self::Dark),
      _ => Err(AppError::status(StatusCode::BAD_REQUEST)),
    }
  }
}

#[derive(Deserialize)]
pub struct CardQuery {
  period: Option<String>,
  theme: Option<String>,
  brand: Option<String>,
}

struct CardOpts {
  period: Period,
  theme: Theme,
  brand: bool,
}

impl TryFrom<CardQuery> for CardOpts {
  type Error = AppError;

  fn try_from(query: CardQuery) -> Result<Self, AppError> {
    let brand = match query.brand.as_deref() {
      None | Some("1") => true,
      Some("0") => false,
      Some(_) => return Err(AppError::status(StatusCode::BAD_REQUEST)),
    };

    Ok(Self {
      period: Period::parse(query.period.as_deref().unwrap_or("30d"))?,
      theme: Theme::parse(query.theme.as_deref().unwrap_or("auto"))?,
      brand,
    })
  }
}

struct Row {
  name: String,
  value: i64,
  delta: Option<i64>,
}

type CardRepo = (RepoTotals, RepoPeriod);

// MARK: Data

pub async fn card(
  State(state): State<Arc<AppState>>,
  Path(name): Path<String>,
  Query(query): Query<CardQuery>,
) -> Result<Response, AppError> {
  let card = Card::from_path(&name).ok_or_else(|| AppError::status(StatusCode::NOT_FOUND))?;
  let opts = CardOpts::try_from(query)?;

  let repos = card_repos(&state, opts.period).await?;
  let svg = match card {
    Card::Totals => totals_card(&repos, &opts),
    _ => {
      let note = list_note(card, &repos, opts.period);
      list_card(card, &note, &top_rows(card, &repos, opts.period), &opts)
    }
  };

  let headers =
    [(header::CONTENT_TYPE, "image/svg+xml"), (header::CACHE_CONTROL, "public,max-age=3600")];
  Ok((headers, svg.into_string()).into_response())
}

/// Cards put repo names on public pages: only what GHS_FILTER and GHS_SHOW allow, never a private
/// repo, whatever GHS_INCLUDE_PRIVATE says
async fn card_repos(state: &Arc<AppState>, period: Period) -> Res<Vec<CardRepo>> {
  let mut repos = state.get_repos_shown(&RepoFilter::default()).await?;
  repos.retain(|x| !x.private);

  let days = match period {
    Period::Days(days) => db_client::Period(days as i32),
    Period::Tracked => db_client::Period(-1),
  };

  // the period of the home page: for traffic its start is the cutoff of the badges, star deltas
  // follow the home page
  let items = state.db.get_periods(None, days, Utc::now().date_naive()).await?;
  let mut items: HashMap<String, RepoPeriod> =
    items.into_iter().map(|x| (x.name.clone(), x)).collect();

  let repos = repos.into_iter().map(|x| {
    let per = items.remove(&x.name).unwrap_or_default();
    (x, per)
  });

  Ok(repos.collect())
}

fn forks_gain(repo: &RepoTotals, per: &RepoPeriod) -> i64 {
  let forks = i64::from(repo.forks);
  forks - per.forks_start.unwrap_or(forks)
}

fn top_rows(card: Card, repos: &[CardRepo], period: Period) -> Vec<Row> {
  let tracked = matches!(period, Period::Tracked);
  let mut rows: Vec<Row> = repos
    .iter()
    .map(|(repo, per)| {
      let (value, delta) = match card {
        Card::Stars => (i64::from(repo.stars), per.stars_gain()),
        Card::Forks => (i64::from(repo.forks), Some(forks_gain(repo, per))),
        Card::Clones => (per.clones, None),
        _ => (per.views, None),
      };

      Row { name: repo.name.clone(), value, delta: delta.filter(|_| !tracked) }
    })
    .collect();

  // no traffic in the period is not a top repo; stars and forks keep their zero rows
  if matches!(card, Card::Clones | Card::Views) {
    rows.retain(|x| x.value > 0);
  }

  rows.sort_by(|a, b| b.value.cmp(&a.value).then_with(|| a.name.cmp(&b.name)));
  rows.truncate(TOP);
  rows
}

fn change_note(period: Period) -> String {
  match period {
    Period::Days(days) => format!("{days}-day change"),
    Period::Tracked => "all time".to_string(),
  }
}

fn list_note(card: Card, repos: &[CardRepo], period: Period) -> String {
  let since = repos.iter().filter_map(|(_, per)| per.first_traffic.as_deref()).min();
  let since = since.and_then(|x| x.parse::<NaiveDate>().ok());

  match (card, period, since) {
    (Card::Clones | Card::Views, Period::Days(days), _) => format!("last {days} days"),
    (Card::Clones | Card::Views, Period::Tracked, Some(x)) => x.format("since %b %Y").to_string(),
    _ => change_note(period),
  }
}

// MARK: Format

/// Exact below 10,000, then short; rounding moves to the next unit: 99,950 is 100k, 999,500 is 1.0M
fn format_count(value: i64) -> String {
  if value < 10_000 {
    return value.separate_with_commas();
  }

  let tenths = (value + 50) / 100;
  if tenths < 1_000 {
    return format!("{}.{}k", tenths / 10, tenths % 10);
  }

  let thousands = (value + 500) / 1_000;
  if thousands < 1_000 {
    return format!("{thousands}k");
  }

  let tenths = (value + 50_000) / 100_000;
  format!("{}.{}M", tenths / 10, tenths % 10)
}

fn format_delta(value: i64) -> (String, &'static str) {
  match value {
    0 => ("0".to_string(), "muted"),
    x if x > 0 => (format!("+{}", format_count(x)), "good"),
    x => (format!("−{}", format_count(-x)), "bad"),
  }
}

fn cut_name(name: &str, limit: usize) -> String {
  if name.chars().count() <= limit {
    return name.to_string();
  }

  let name: String = name.chars().take(limit - 1).collect();
  format!("{name}…")
}

fn bar_width(value: i64, top: i64) -> i32 {
  let width = f64::from(BAR_MAX) * value as f64 / top.max(1) as f64;
  (width.round() as i32).max(BAR_MIN)
}

// MARK: SVG

fn rules(palette: &Palette, card: Card) -> String {
  let Palette { card: bg, border, head, ink, muted, good, bad, mark, .. } = palette;
  let mut css = format!(
    ".card{{fill:{bg};stroke:{border}}}.head{{fill:{head}}}.ink{{fill:{ink}}}\
    .muted{{fill:{muted}}}.good{{fill:{good}}}.bad{{fill:{bad}}}\
    .mark{{fill:{head};fill-opacity:{mark}}}"
  );

  if let Some(bar) = card.bar(palette) {
    css += &format!(".bar{{fill:{bar};fill-opacity:{BAR_OPACITY}}}");
  }

  css
}

fn style(theme: Theme, card: Card) -> String {
  let base = "text{font-variant-numeric:tabular-nums}";
  match theme {
    Theme::Auto => {
      let (light, dark) = (rules(&LIGHT, card), rules(&DARK, card));
      format!("{base}{light}@media (prefers-color-scheme:dark){{{dark}}}")
    }
    Theme::Light => format!("{base}{}", rules(&LIGHT, card)),
    Theme::Dark => format!("{base}{}", rules(&DARK, card)),
  }
}

fn text(
  x: impl Render,
  y: i32,
  size: f32,
  class: &str,
  weight: Option<u32>,
  end: bool,
  body: &str,
) -> Markup {
  html!(
    text x=(x) y=(y) font-size=(size) class=(class) font-weight=[weight]
      text-anchor=[end.then_some("end")] { (body) }
  )
}

fn frame(height: i32, label: &str, card: Card, opts: &CardOpts, body: Markup) -> Markup {
  html!(
    svg xmlns="http://www.w3.org/2000/svg" width=(W) height=(height)
      viewBox=(format!("0 0 {W} {height}")) font-family=(FONT) {
      title { (label) }
      // static text only: everything dynamic goes through maud's escaping
      style { (PreEscaped(style(opts.theme, card))) }
      rect x="0.5" y="0.5" width=(W - 1) height=(height - 1) rx="6" class="card" {}
      @if opts.brand {
        (text(W - MARK_RIGHT, height + MARK_DROP, MARK_SIZE, "mark", Some(700), true, "ghstats"))
      }
      (body)
    }
  )
}

fn card_header(title: &str, note: &str) -> Markup {
  let left = text(PAD, 27, 14.0, "head", HEAD_WEIGHT, false, title);
  let right = text(W - PAD, 27, 11.5, "muted", None, true, note);
  html!((left)(right))
}

fn list_card(card: Card, note: &str, rows: &[Row], opts: &CardOpts) -> Markup {
  let with_delta = rows.iter().any(|x| x.delta.is_some());
  let value_r = if with_delta { VALUE_R } else { TEXT_R };
  let limit = if with_delta { NAME_MAX_DELTA } else { NAME_MAX_PLAIN };
  let top = rows.iter().map(|x| x.value).max().unwrap_or_default();

  let one_owner =
    rows.windows(2).all(|x| x[0].name.split('/').next() == x[1].name.split('/').next());
  let name = |x: &Row| match x.name.split_once('/') {
    Some((_, repo)) if one_owner => cut_name(repo, limit),
    _ => cut_name(&x.name, limit),
  };

  let count = rows.len().max(1) as i32;
  let height = ROWS_Y + count * (ROW_H + ROW_GAP) - ROW_GAP + PAD;
  let body = html!(
    (card_header(card.title(), note))
    @if rows.is_empty() {
      (text(TEXT_L, ROWS_Y + BASELINE, 13.0, "muted", None, false, "No data"))
    }
    @for (idx, row) in rows.iter().enumerate() {
      @let y = ROWS_Y + idx as i32 * (ROW_H + ROW_GAP);
      rect x=(PAD) y=(y) width=(bar_width(row.value, top)) height=(ROW_H) rx="3" class="bar" {}
      (text(TEXT_L, y + BASELINE, 13.0, "ink", None, false, &name(row)))
      (text(value_r, y + BASELINE, 13.0, "ink", None, true, &format_count(row.value)))
      @if let Some(delta) = row.delta {
        @let (label, class) = format_delta(delta);
        (text(TEXT_R, y + BASELINE, 12.0, class, None, true, &label))
      }
    }
  );

  frame(height, &format!("{}, {note}", card.title()), card, opts, body)
}

fn totals_card(repos: &[CardRepo], opts: &CardOpts) -> Markup {
  let note = change_note(opts.period);
  if repos.is_empty() {
    return list_card(Card::Totals, &note, &[], opts);
  }

  let sum = |f: fn(&CardRepo) -> i64| repos.iter().map(f).sum::<i64>();
  let tracked = matches!(opts.period, Period::Tracked);
  let delta = |value: i64| (!tracked).then_some(value);

  // as on the home page: a repo without star history before the period adds nothing
  let gains: Vec<i64> = repos.iter().filter_map(|(_, per)| per.stars_gain()).collect();
  let stars = (!tracked && !gains.is_empty()).then(|| gains.iter().sum());

  let cols = [
    ("Stars", sum(|x| x.0.stars.into()), stars),
    ("Forks", sum(|x| x.0.forks.into()), delta(sum(|x| forks_gain(&x.0, &x.1)))),
    ("Views", sum(|x| x.0.views_count.into()), delta(sum(|x| x.1.views))),
    ("Clones", sum(|x| x.0.clones_count.into()), delta(sum(|x| x.1.clones))),
    ("Repos", repos.len() as i64, None),
  ];

  let summary: Vec<String> = cols
    .iter()
    .map(|(label, value, _)| format!("{} {}", format_count(*value), label.to_lowercase()))
    .collect();

  let height = if tracked { TOTALS_H_PLAIN } else { TOTALS_H };
  let body = html!(
    (card_header(Card::Totals.title(), &note))
    @for (idx, (label, value, delta)) in cols.iter().enumerate() {
      @let x = format!("{:.1}", f64::from(PAD) + idx as f64 * TOTALS_COL);
      (text(x.as_str(), TOTALS_LABEL_Y, 11.5, "muted", None, false, label))
      (text(x.as_str(), TOTALS_VALUE_Y, 20.0, "head", HEAD_WEIGHT, false, &format_count(*value)))
      @if let Some(delta) = delta {
        @let (label, class) = format_delta(*delta);
        (text(x.as_str(), TOTALS_DELTA_Y, 12.0, class, None, false, &label))
      }
    }
  );

  frame(height, &format!("Totals: {}", summary.join(", ")), Card::Totals, opts, body)
}

#[cfg(test)]
mod tests {
  use std::time::{SystemTime, UNIX_EPOCH};

  use axum::body::to_bytes;
  use axum::http::Uri;
  use chrono::Duration;

  use super::*;
  use crate::db_client::DbClient;
  use crate::filter::Filter;
  use crate::gh_client::{Repo, RepoClones, RepoViews, TrafficDaily};

  fn db_path(test_name: &str) -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    std::env::temp_dir()
      .join(format!("ghstats-card-{test_name}-{}-{nanos}.db", std::process::id()))
      .to_string_lossy()
      .into_owned()
  }

  async fn test_state(test_name: &str) -> Arc<AppState> {
    let db = DbClient::new(&db_path(test_name)).await.unwrap();
    Arc::new(AppState::for_test(db, ""))
  }

  fn sample_repo(id: u64, name: &str, stars: u32, forks: u32) -> Repo {
    Repo {
      id,
      full_name: name.to_string(),
      stargazers_count: stars,
      forks_count: forks,
      watchers_count: stars.max(1),
      ..Default::default()
    }
  }

  fn date(days_ago: i64) -> String {
    let date = Utc::now().date_naive() - Duration::days(days_ago);
    format!("{date}T00:00:00Z")
  }

  fn daily(timestamp: String, count: u32) -> TrafficDaily {
    TrafficDaily { timestamp, uniques: count, count }
  }

  /// Rows of (days ago, stars, forks), oldest first; the last one is the current value
  async fn seed_stats(state: &Arc<AppState>, id: u64, name: &str, rows: &[(i64, u32, u32)]) {
    let (_, stars, forks) = rows.last().copied().unwrap();
    state.db.insert_repo(&sample_repo(id, name, stars, forks)).await.unwrap();
    state.db.mark_repo_stars_synced(id as i64).await.unwrap();

    for (days_ago, stars, forks) in rows {
      let repo = sample_repo(id, name, *stars, *forks);
      state.db.insert_stats(&repo, &date(*days_ago), &[]).await.unwrap();
    }
  }

  async fn seed_views(state: &Arc<AppState>, id: u64, rows: &[(i64, u32)]) {
    let views = rows.iter().map(|x| daily(date(x.0), x.1)).collect();
    let views = RepoViews { uniques: 0, count: 0, views };
    state.db.insert_views(&sample_repo(id, "", 0, 0), &views).await.unwrap();
  }

  async fn seed_clones(state: &Arc<AppState>, id: u64, rows: &[(i64, u32)]) {
    let clones = rows.iter().map(|x| daily(date(x.0), x.1)).collect();
    let clones = RepoClones { uniques: 0, count: 0, clones };
    state.db.insert_clones(&sample_repo(id, "", 0, 0), &clones).await.unwrap();
  }

  async fn response(state: &Arc<AppState>, name: &str, query: &str) -> Result<Response, AppError> {
    let uri: Uri = format!("/card/{name}?{query}").parse().unwrap();
    let query = Query::try_from_uri(&uri).unwrap();
    card(State(state.clone()), Path(name.to_string()), query).await
  }

  async fn svg(state: &Arc<AppState>, name: &str, query: &str) -> String {
    let body = response(state, name, query).await.unwrap().into_body();
    let body = to_bytes(body, usize::MAX).await.unwrap();
    String::from_utf8(body.to_vec()).unwrap()
  }

  async fn status(state: &Arc<AppState>, name: &str, query: &str) -> StatusCode {
    match response(state, name, query).await {
      Ok(x) => x.status(),
      Err(e) => e.into_response().status(),
    }
  }

  fn texts(svg: &str) -> Vec<&str> {
    svg
      .split("<text ")
      .skip(1)
      .filter_map(|x| x.split_once('>')?.1.split_once('<'))
      .map(|x| x.0)
      .collect()
  }

  fn order(svg: &str, names: &[&str]) -> bool {
    let at: Vec<usize> = names.iter().map(|x| svg.find(&format!(">{x}<")).unwrap()).collect();
    at.is_sorted()
  }

  #[tokio::test]
  async fn renders_each_card_with_headers() {
    let state = test_state("headers").await;
    seed_stats(&state, 1, "owner/repo", &[(40, 10, 1), (0, 30, 2)]).await;
    seed_views(&state, 1, &[(5, 20)]).await;
    seed_clones(&state, 1, &[(5, 4)]).await;

    let cards = [
      ("stars.svg", "Top stars", "30-day change"),
      ("forks.svg", "Top forks", "30-day change"),
      ("clones.svg", "Top clones", "last 30 days"),
      ("views.svg", "Top views", "last 30 days"),
      ("totals.svg", "Totals", "30-day change"),
    ];

    for (name, title, note) in cards {
      let rep = response(&state, name, "").await.unwrap();
      assert_eq!(rep.status(), StatusCode::OK, "{name}");
      assert_eq!(rep.headers()[header::CONTENT_TYPE], "image/svg+xml", "{name}");
      assert_eq!(rep.headers()[header::CACHE_CONTROL], "public,max-age=3600", "{name}");

      let svg = svg(&state, name, "").await;
      assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"420\""), "{name}");
      assert!(svg.ends_with("</svg>"), "{name}");
      assert!(svg.contains(&format!(">{title}<")), "{name}");
      assert!(svg.contains(&format!(">{note}<")), "{name}");
    }
  }

  #[tokio::test]
  async fn ranks_by_value_then_name_and_keeps_five() {
    let state = test_state("ranking").await;
    for (id, name, stars) in
      [(1, "o/f", 1), (2, "o/b", 20), (3, "o/a", 20), (4, "o/e", 50), (5, "o/d", 3), (6, "o/c", 4)]
    {
      seed_stats(&state, id, name, &[(0, stars, 0)]).await;
    }

    seed_clones(&state, 1, &[(1, 5), (2, 6)]).await;
    seed_clones(&state, 2, &[(1, 10)]).await;
    seed_clones(&state, 3, &[(1, 2), (60, 1000)]).await;

    let svg_stars = svg(&state, "stars.svg", "").await;
    assert!(order(&svg_stars, &["e", "a", "b", "c", "d"]));
    assert!(!svg_stars.contains(">f<"), "only the top 5");

    let svg_clones = svg(&state, "clones.svg", "").await;
    assert!(order(&svg_clones, &["f", "b", "a"]), "by the sum in the period");
    assert!(svg_clones.contains(">11<"));

    let svg_clones = svg(&state, "clones.svg", "period=tracked").await;
    assert!(order(&svg_clones, &["a", "f", "b"]), "by the sum over all history");
    assert!(svg_clones.contains(">1,002<"));
  }

  #[tokio::test]
  async fn shows_positive_zero_and_negative_deltas() {
    let state = test_state("deltas").await;
    seed_stats(&state, 1, "o/up", &[(40, 100, 30), (0, 125, 34)]).await;
    seed_stats(&state, 2, "o/flat", &[(40, 90, 20), (0, 90, 20)]).await;
    seed_stats(&state, 3, "o/down", &[(40, 80, 15), (0, 70, 12)]).await;

    let stars = svg(&state, "stars.svg", "").await;
    assert!(stars.contains(r#"class="good" text-anchor="end">+25<"#));
    assert!(stars.contains(r#"class="muted" text-anchor="end">0<"#));
    assert!(stars.contains(r#"class="bad" text-anchor="end">−10<"#));

    let forks = svg(&state, "forks.svg", "").await;
    assert!(forks.contains(r#"class="good" text-anchor="end">+4<"#));
    assert!(forks.contains(r#"class="muted" text-anchor="end">0<"#));
    assert!(forks.contains(r#"class="bad" text-anchor="end">−3<"#));

    let totals = svg(&state, "totals.svg", "").await;
    assert_eq!(
      texts(&totals).join(" "),
      "ghstats Totals 30-day change Stars 285 +15 Forks 66 +1 Views 0 0 Clones 0 0 Repos 3"
    );
  }

  #[tokio::test]
  async fn stars_delta_is_the_home_page_gain() {
    let state = test_state("stars-home").await;
    // the home page counts stars gained on the first day of the period
    seed_stats(&state, 1, "o/edge", &[(30, 10, 0), (29, 15, 0), (0, 15, 0)]).await;
    // a synced history starting in the period gained all its stars there
    seed_stats(&state, 2, "o/new", &[(3, 7, 0)]).await;
    // an unsynced one doesn't know: no number at all, as on the home page
    seed_stats(&state, 3, "o/unsynced", &[(3, 5, 0)]).await;
    state.db.mark_repos_stars_unsynced().await.unwrap();
    state.db.mark_repo_stars_synced(1).await.unwrap();
    state.db.mark_repo_stars_synced(2).await.unwrap();

    let today = Utc::now().date_naive();
    let gains = state.db.get_periods(None, db_client::Period(30), today).await.unwrap();
    let gain = |name: &str| gains.iter().find(|x| x.name == name).unwrap().stars_gain();
    assert_eq!((gain("o/edge"), gain("o/new"), gain("o/unsynced")), (Some(5), Some(7), None));

    let body = svg(&state, "stars.svg", "").await;
    assert_eq!(
      texts(&body)[3..].join(" "),
      "edge 15 +5 new 7 +7 unsynced 5",
      "a row without a known gain leaves the cell empty"
    );

    let body = svg(&state, "totals.svg", "").await;
    assert!(body.contains(">+12<"), "the same sum as the home page strip");
  }

  #[tokio::test]
  async fn period_picks_the_window_and_the_note() {
    let state = test_state("periods").await;
    seed_stats(&state, 1, "o/r", &[(100, 1, 1), (50, 10, 2), (20, 20, 3), (0, 40, 4)]).await;
    // the cutoff day counts, the day before doesn't: the same cutoff as the badges
    seed_views(&state, 1, &[(6, 1), (7, 10), (29, 100), (30, 1_000), (89, 2_000), (90, 4_000)])
      .await;

    let cases = [
      ("", "30-day change", "+30", "last 30 days", "111"),
      ("period=30d", "30-day change", "+30", "last 30 days", "111"),
      ("period=7d", "7-day change", "+20", "last 7 days", "1"),
      ("period=90d", "90-day change", "+39", "last 90 days", "3,111"),
    ];

    for (query, change, delta, window, views) in cases {
      let stars = svg(&state, "stars.svg", query).await;
      assert!(stars.contains(&format!(">{change}<")), "{query}");
      assert!(stars.contains(&format!(">{delta}<")), "{query}");

      let body = svg(&state, "views.svg", query).await;
      assert!(body.contains(&format!(">{window}<")), "{query}");
      assert!(body.contains(&format!(">{views}<")), "{query}");
    }
  }

  #[tokio::test]
  async fn tracked_drops_deltas_and_names_the_start() {
    let state = test_state("tracked").await;
    seed_stats(&state, 1, "o/r", &[(40, 10, 1), (0, 30, 2)]).await;
    seed_stats(&state, 2, "o/other", &[(40, 10, 1), (0, 20, 2)]).await;

    let repo = sample_repo(1, "o/r", 0, 0);
    let views = vec![daily("2024-06-18T00:00:00Z".to_string(), 3)];
    state.db.insert_views(&repo, &RepoViews { uniques: 0, count: 0, views }).await.unwrap();

    // star history reaches further back, but without traffic it doesn't start the card
    let stars = [("2023-01-01T00:00:00Z".to_string(), 1, 1)];
    state.db.insert_stars(1, &stars).await.unwrap();

    for name in ["stars.svg", "forks.svg"] {
      let body = svg(&state, name, "period=tracked").await;
      assert!(body.contains(">all time<"), "{name}");
      assert_eq!(texts(&body).len(), 3 + 2 * 2, "{name}: name and value in each row");
      assert!(body.contains(r#"x="396" y="60" font-size="13""#), "{name}: value takes the column");
    }

    let body = svg(&state, "views.svg", "period=tracked").await;
    assert!(body.contains(">since Jun 2024<"));

    let body = svg(&state, "clones.svg", "period=tracked").await;
    assert!(body.contains(">since Jun 2024<"));

    let body = svg(&state, "totals.svg", "period=tracked").await;
    assert!(body.contains(">all time<"));
    assert!(body.contains(r#"height="90""#), "no delta row");
    assert!(!body.contains(r#"y="93""#));
  }

  #[tokio::test]
  async fn theme_and_brand_change_the_style_and_wordmark() {
    let state = test_state("theme").await;
    seed_stats(&state, 1, "o/r", &[(0, 30, 2)]).await;

    let auto = svg(&state, "stars.svg", "").await;
    assert_eq!(auto, svg(&state, "stars.svg", "theme=auto").await);
    assert!(auto.contains("@media (prefers-color-scheme:dark)"));
    assert!(auto.contains(".card{fill:#ffffff;stroke:#e3e6ec}"));
    assert!(auto.contains(".card{fill:#181c25;stroke:#262d3b}"));
    assert!(auto.contains(".mark{fill:#24292f;fill-opacity:0.05}"));
    assert!(auto.contains(".mark{fill:#f0f1f3;fill-opacity:0.04}"));

    let light = svg(&state, "stars.svg", "theme=light").await;
    assert!(!light.contains("@media"));
    assert!(light.contains(".card{fill:#ffffff"));
    assert!(!light.contains("#181c25"));

    let dark = svg(&state, "stars.svg", "theme=dark").await;
    assert!(!dark.contains("@media"));
    assert!(dark.contains(".card{fill:#181c25"));
    assert!(!dark.contains("#ffffff"));

    for name in ["stars.svg", "totals.svg"] {
      let body = svg(&state, name, "").await;
      assert!(body.contains(r#"class="mark" font-weight="700" text-anchor="end">ghstats<"#));
      assert_eq!(body, svg(&state, name, "brand=1").await);

      let body = svg(&state, name, "brand=0").await;
      assert!(!body.contains(">ghstats<"), "{name}");
    }
  }

  #[tokio::test]
  async fn rejects_invalid_params_and_unknown_cards() {
    let state = test_state("invalid").await;

    let cases = [
      "period=year",
      "period=",
      "period=7",
      "theme=blue",
      "theme=",
      "theme=Dark",
      "brand=2",
      "brand=",
      "brand=false",
    ];

    for name in ["stars.svg", "totals.svg"] {
      for query in cases {
        assert_eq!(status(&state, name, query).await, StatusCode::BAD_REQUEST, "{query}");
      }
    }

    for name in ["nope.svg", "stars", "stars.png", "stars-growth.svg"] {
      assert_eq!(status(&state, name, "").await, StatusCode::NOT_FOUND, "{name}");
    }
  }

  #[tokio::test]
  async fn private_and_show_hidden_repos_stay_out() {
    let db = DbClient::new(&db_path("privacy")).await.unwrap();
    let mut state = AppState::for_test(db, "");
    state.show = Filter::parse("!o/hidden-by-show", true).unwrap();
    // the setting that lets private repos into the app never lets them into a card
    state.include_private = true;
    let state = Arc::new(state);

    seed_stats(&state, 1, "o/visible", &[(0, 10, 1)]).await;
    seed_stats(&state, 2, "o/hidden-by-show", &[(0, 1_000, 200)]).await;
    seed_stats(&state, 3, "o/secret", &[(0, 3_000, 400)]).await;

    let secret = Repo { private: true, ..sample_repo(3, "o/secret", 3_000, 400) };
    state.db.insert_repo(&secret).await.unwrap();

    seed_views(&state, 1, &[(1, 5)]).await;
    seed_clones(&state, 1, &[(1, 6)]).await;
    for id in [2, 3] {
      seed_views(&state, id, &[(1, 7_000)]).await;
      seed_clones(&state, id, &[(1, 8_000)]).await;

      let repo = sample_repo(id, "", 0, 0);
      let views = vec![daily("2020-02-02T00:00:00Z".to_string(), 7_000)];
      state.db.insert_views(&repo, &RepoViews { uniques: 0, count: 0, views }).await.unwrap();
    }

    for name in ["stars.svg", "forks.svg", "clones.svg", "views.svg"] {
      for query in ["", "period=tracked", "period=7d"] {
        let body = svg(&state, name, query).await;
        assert!(!body.contains("secret"), "{name} {query}");
        assert!(!body.contains("hidden-by-show"), "{name} {query}");
        assert_eq!(body.matches(r#"class="bar""#).count(), 1, "{name} {query}");
      }
    }

    // their traffic doesn't move the card's start either
    let body = svg(&state, "views.svg", "period=tracked").await;
    assert!(!body.contains("2020"));

    let body = svg(&state, "totals.svg", "").await;
    assert!(body.contains("<title>Totals: 10 stars, 1 forks, 5 views, 6 clones, 1 repos</title>"));
    assert_eq!(
      texts(&body)[3..].join(" "),
      "Stars 10 +10 Forks 1 0 Views 5 +5 Clones 6 +6 Repos 1"
    );

    let body = svg(&state, "totals.svg", "period=tracked").await;
    assert!(body.contains("<title>Totals: 10 stars, 1 forks, 5 views, 6 clones, 1 repos</title>"));
  }

  #[tokio::test]
  async fn forks_baseline_skips_rows_without_fork_data() {
    let state = test_state("forks-baseline").await;
    let star_history = |days: &[i64]| {
      days.iter().enumerate().map(|(idx, x)| (date(*x), idx as u32 + 1, 1)).collect::<Vec<_>>()
    };

    // a star history row at the cutoff holds forks = 0, the last real row before it is 10
    seed_stats(&state, 1, "o/gap", &[(60, 50, 10), (5, 60, 11), (0, 60, 12)]).await;
    state.db.insert_stars(1, &star_history(&[40, 29])).await.unwrap();

    // no real row before the cutoff: the earliest real row is the baseline
    seed_stats(&state, 2, "o/late", &[(5, 60, 7), (0, 60, 9)]).await;
    state.db.insert_stars(2, &star_history(&[100, 50, 40])).await.unwrap();

    // a traffic row on a star history day sets `traffic`, the forks stay 0
    seed_stats(&state, 3, "o/flagged", &[(60, 50, 4), (0, 60, 6)]).await;
    state.db.insert_stars(3, &star_history(&[35])).await.unwrap();
    seed_views(&state, 3, &[(35, 9), (31, 9)]).await;

    // a real 0 still counts: the first fork came in the period
    seed_stats(&state, 4, "o/first", &[(60, 50, 0), (0, 50, 1)]).await;

    // nothing but star history and traffic
    state.db.insert_repo(&sample_repo(5, "o/none", 0, 0)).await.unwrap();
    state.db.insert_stars(5, &star_history(&[50])).await.unwrap();
    seed_views(&state, 5, &[(1, 9)]).await;

    let today = Utc::now().date_naive();
    let per = state.db.get_periods(None, db_client::Period(30), today).await.unwrap();
    let start = |name: &str| per.iter().find(|x| x.name == name).unwrap().forks_start;
    assert_eq!(start("o/gap"), Some(10));
    assert_eq!(start("o/late"), Some(7));
    assert_eq!(start("o/flagged"), Some(4));
    assert_eq!(start("o/first"), Some(0));
    assert_eq!(start("o/none"), None);

    let body = svg(&state, "forks.svg", "").await;
    assert_eq!(texts(&body)[3..].join(" "), "gap 12 +2 late 9 +2 flagged 6 +2 first 1 +1 none 0 0");

    let body = svg(&state, "totals.svg", "").await;
    assert!(body.contains(">+7<"));
  }

  #[tokio::test]
  async fn forks_gained_on_the_first_day_of_the_period_count() {
    let state = test_state("forks-first-day").await;
    // 30d starts 29 days ago
    seed_stats(&state, 1, "o/r", &[(30, 5, 2), (29, 5, 6), (0, 5, 6)]).await;

    let body = svg(&state, "forks.svg", "").await;
    assert_eq!(texts(&body)[3..].join(" "), "r 6 +4");
  }

  #[tokio::test]
  async fn traffic_cards_drop_zero_rows() {
    let state = test_state("zero-rows").await;
    seed_stats(&state, 1, "o/busy", &[(0, 5, 2)]).await;
    seed_stats(&state, 2, "o/quiet", &[(0, 0, 0)]).await;
    seed_views(&state, 1, &[(1, 7)]).await;
    seed_clones(&state, 1, &[(1, 4)]).await;

    for name in ["views.svg", "clones.svg"] {
      let body = svg(&state, name, "").await;
      assert!(body.contains(">busy<"), "{name}");
      assert!(!body.contains("quiet"), "{name}");
      assert!(!body.contains("No data"), "{name}");
    }

    for name in ["stars.svg", "forks.svg"] {
      assert!(svg(&state, name, "").await.contains(">quiet<"), "{name}");
    }

    // traffic only before the period: an all-zero card has nothing to list
    let state = test_state("zero-card").await;
    seed_stats(&state, 1, "o/old", &[(0, 5, 2)]).await;
    seed_views(&state, 1, &[(20, 7)]).await;
    seed_clones(&state, 1, &[(20, 4)]).await;

    for name in ["views.svg", "clones.svg"] {
      let body = svg(&state, name, "period=7d").await;
      assert!(body.contains(r#"class="muted">No data<"#), "{name}");
      assert!(!body.contains("old"), "{name}");
    }
  }

  #[tokio::test]
  async fn empty_database_renders_no_data() {
    let state = test_state("empty").await;

    for name in ["stars.svg", "forks.svg", "clones.svg", "views.svg", "totals.svg"] {
      for query in ["", "period=tracked"] {
        let body = svg(&state, name, query).await;
        assert!(body.contains(r#"class="muted">No data<"#), "{name}");
        assert!(body.contains(r#"height="84""#), "{name}");
        assert!(!body.contains(r#"class="bar""#), "{name}");
      }
    }

    assert!(svg(&state, "clones.svg", "period=tracked").await.contains(">all time<"));
  }

  #[tokio::test]
  async fn height_follows_the_rows_and_names_lose_a_shared_owner() {
    let state = test_state("height").await;
    seed_stats(&state, 1, "o/one", &[(0, 3, 0)]).await;
    seed_stats(&state, 2, "o/two", &[(0, 2, 0)]).await;

    let body = svg(&state, "stars.svg", "").await;
    assert!(body.contains(r#"height="113" viewBox="0 0 420 113""#));
    assert!(body.contains(">one<") && body.contains(">two<"));

    seed_stats(&state, 3, "x/three", &[(0, 1, 0)]).await;
    let body = svg(&state, "stars.svg", "").await;
    assert!(body.contains(r#"height="142""#));
    assert!(body.contains(">o/one<") && body.contains(">x/three<"));
  }

  #[tokio::test]
  async fn escapes_dynamic_text() {
    let state = test_state("escape").await;
    seed_stats(&state, 1, "o/tom&jerry<beta>", &[(0, 3, 0)]).await;

    let body = svg(&state, "stars.svg", "").await;
    assert!(body.contains(">tom&amp;jerry&lt;beta&gt;<"));
    assert!(!body.contains("tom&jerry"));
  }

  #[test]
  fn format_count_boundaries() {
    let cases = [
      (0, "0"),
      (612, "612"),
      (5_815, "5,815"),
      (9_999, "9,999"),
      (10_000, "10.0k"),
      (52_744, "52.7k"),
      (93_478, "93.5k"),
      (99_949, "99.9k"),
      (99_950, "100k"),
      (99_999, "100k"),
      (100_000, "100k"),
      (204_519, "205k"),
      (293_492, "293k"),
      (999_499, "999k"),
      (999_500, "1.0M"),
      (999_999, "1.0M"),
      (1_000_000, "1.0M"),
      (1_200_000, "1.2M"),
      (1_249_999, "1.2M"),
      (1_250_000, "1.3M"),
    ];

    for (value, want) in cases {
      assert_eq!(format_count(value), want, "{value}");
    }
  }

  #[test]
  fn format_delta_signs() {
    let cases = [
      (0, "0", "muted"),
      (3, "+3", "good"),
      (10_872, "+10.9k", "good"),
      (-2, "−2", "bad"),
      (-1_250, "−1,250", "bad"),
      (-99_950, "−100k", "bad"),
    ];

    for (value, want, class) in cases {
      assert_eq!(format_delta(value), (want.to_string(), class), "{value}");
    }
  }

  #[test]
  fn cut_name_counts_chars() {
    assert_eq!(cut_name("short", 30), "short");
    assert_eq!(cut_name(&"a".repeat(30), 30), "a".repeat(30));
    assert_eq!(cut_name(&"a".repeat(31), 30), format!("{}…", "a".repeat(29)));

    // 2, 3 and 4 bytes per char, a byte slice would panic or cut early
    for x in ["я", "語", "🦀"] {
      assert_eq!(cut_name(&x.repeat(30), 30), x.repeat(30));
      assert_eq!(cut_name(&x.repeat(40), 30), format!("{}…", x.repeat(29)));
      assert_eq!(cut_name(&x.repeat(40), 30).chars().count(), 30);
    }
  }

  #[test]
  fn bar_width_is_linear_with_a_minimum() {
    let cases =
      [(100, 100, 388), (50, 100, 194), (2_089, 2_932, 276), (1, 1_000, 3), (0, 100, 3), (0, 0, 3)];

    for (value, top, want) in cases {
      assert_eq!(bar_width(value, top), want, "{value}/{top}");
    }
  }

  #[tokio::test]
  async fn name_budget_depends_on_the_delta_column() {
    let state = test_state("budget").await;
    let name = format!("o/{}", "n".repeat(40));
    seed_stats(&state, 1, &name, &[(0, 3, 0)]).await;

    let body = svg(&state, "stars.svg", "").await;
    assert!(body.contains(&format!(">{}…<", "n".repeat(29))));

    let body = svg(&state, "stars.svg", "period=tracked").await;
    assert!(body.contains(&format!(">{}…<", "n".repeat(35))));
  }
}
