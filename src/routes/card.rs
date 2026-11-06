use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::{Duration, NaiveDate, Utc};
use maud::{Markup, PreEscaped, Render, html};
use serde::Deserialize;
use thousands::Separable;

use super::badge::{Period, dated_stars, in_period_date};
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

// history cards fill the width of a README, as star-history.com charts do
const CHART_W: i32 = 800;
const CHART_H: i32 = 480;
const CHART_EMPTY_H: i32 = 128;
const CHART_PAD: i32 = 28;
const CHART_HEAD_Y: i32 = 50;
const CHART_L: i32 = 88; // the y labels stay to the left of it
const CHART_R: i32 = CHART_W - CHART_PAD;
const CHART_T: i32 = 84;
const CHART_B: i32 = 416;
const CHART_TITLE_TEXT: f32 = 26.0;
const CHART_NOTE_TEXT: f32 = 20.0;
const CHART_TEXT: f32 = 18.0; // sized to stay readable when the image is shown at 420
const CHART_POINTS: usize = 200;
const CHART_REPOS: usize = 4;
const CHART_TITLE: &str = "Star history";
const DOT: f64 = 12.0;
const X_LABEL_Y: i32 = 450;
const X_LABELS: i64 = 4;

const LEGEND_X: i32 = CHART_L + 12;
const LEGEND_Y: i32 = CHART_T + 12;
const LEGEND_PAD: i32 = 14;
const LEGEND_ROW: i32 = 30;
const LEGEND_OPACITY: f32 = 0.8; // lines stay visible behind it
const LEGEND_DOT: f64 = 10.0;
const LEGEND_TEXT: i32 = 20; // from the dot to the name
const LEGEND_CHAR: i32 = 10; // no text measuring in SVG: a wide guess per char

// the wordmark sits behind the rows, cut off by the bottom edge
const MARK_SIZE: f32 = 64.0;
const MARK_RIGHT: i32 = 10;
const MARK_DROP: i32 = 8;
const CHART_MARK_SIZE: f32 = 120.0;
const CHART_MARK_RIGHT: i32 = 19;
const CHART_MARK_DROP: i32 = 15;

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
  History,
}

impl Card {
  fn from_path(value: &str) -> Option<Self> {
    match value {
      "stars.svg" => Some(Self::Stars),
      "forks.svg" => Some(Self::Forks),
      "clones.svg" => Some(Self::Clones),
      "views.svg" => Some(Self::Views),
      "totals.svg" => Some(Self::Totals),
      "history.svg" => Some(Self::History),
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
      Self::History => "All repositories",
    }
  }

  fn bar(self, palette: &Palette) -> Option<&'static str> {
    match self {
      Self::Stars | Self::History => Some(palette.stars),
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
  repos: Option<String>,
  log: Option<String>,
  align: Option<String>,
  since: Option<String>,
}

struct CardOpts {
  period: Period,
  since: Option<NaiveDate>,
  theme: Theme,
  brand: bool,
  log: bool,
  align: bool,
}

fn parse_flag(value: Option<&str>) -> Result<bool, AppError> {
  match value {
    None | Some("0") => Ok(false),
    Some("1") => Ok(true),
    Some(_) => Err(AppError::status(StatusCode::BAD_REQUEST)),
  }
}

fn parse_since(value: &str) -> Result<NaiveDate, AppError> {
  value.parse().map_err(|_| AppError::status(StatusCode::BAD_REQUEST))
}

impl TryFrom<CardQuery> for CardOpts {
  type Error = AppError;

  fn try_from(query: CardQuery) -> Result<Self, AppError> {
    Ok(Self {
      period: Period::parse(query.period.as_deref().unwrap_or("30d"))?,
      since: query.since.as_deref().map(parse_since).transpose()?,
      theme: Theme::parse(query.theme.as_deref().unwrap_or("auto"))?,
      brand: parse_flag(query.brand.as_deref().or(Some("1")))?,
      log: parse_flag(query.log.as_deref())?,
      align: parse_flag(query.align.as_deref())?,
    })
  }
}

impl CardOpts {
  fn history(query: CardQuery) -> Result<Self, AppError> {
    let period = query.period.or(Some("tracked".to_string()));
    Self::try_from(CardQuery { period, ..query })
  }

  /// On the history charts `since` replaces the period
  fn in_range(&self, date: NaiveDate) -> bool {
    self.since.map_or(in_period_date(date, self.period), |x| date >= x)
  }
}

struct Row {
  name: String,
  value: i64,
  delta: Option<i64>,
}

type CardRepo = (RepoTotals, RepoPeriod);
type Point = (NaiveDate, i64);

struct Line {
  name: String,
  count: i64, // the current one, whatever the period
  points: Vec<Point>,
}

impl Line {
  fn new(name: &str, points: Vec<Point>) -> Self {
    let count = points.last().map_or(0, |x| x.1);
    Self { name: name.to_string(), count, points }
  }
}

// MARK: Data

pub async fn card(
  State(state): State<Arc<AppState>>,
  Path(name): Path<String>,
  Query(query): Query<CardQuery>,
) -> Result<Response, AppError> {
  let card = Card::from_path(&name).ok_or_else(|| AppError::status(StatusCode::NOT_FOUND))?;
  if matches!(card, Card::History) {
    let names = query.repos.clone();
    let opts = CardOpts::history(query)?;
    if let Some(names) = names {
      let names: Vec<&str> = names.split(',').collect();
      if names.len() > CHART_REPOS {
        return Err(AppError::status(StatusCode::BAD_REQUEST));
      }

      let lines = public_lines(&state, &names).await?;
      return Ok(svg_response(history_card(CHART_TITLE, lines, true, &opts)));
    }

    let repos = public_repos(&state).await?;
    let points = total_points(&state, &repos).await?;
    let title = shared_owner(repos.iter().map(|x| x.name.as_str())).unwrap_or(card.title());
    let lines = vec![Line::new(title, points)];
    return Ok(svg_response(history_card(title, lines, false, &opts)));
  }

  let opts = CardOpts::try_from(query)?;
  let repos = card_repos(&state, opts.period).await?;
  let svg = match card {
    Card::Totals => totals_card(&repos, &opts),
    _ => {
      let note = list_note(card, &repos, opts.period);
      list_card(card, &note, &top_rows(card, &repos, opts.period), &opts)
    }
  };

  Ok(svg_response(svg))
}

pub async fn repo_card(
  State(state): State<Arc<AppState>>,
  Path((owner, repo)): Path<(String, String)>,
  Query(query): Query<CardQuery>,
) -> Result<Response, AppError> {
  let opts = CardOpts::history(query)?;
  let repo = format!("{owner}/{repo}");
  let lines = public_lines(&state, &[&repo]).await?;
  Ok(svg_response(history_card(&repo, lines, false, &opts)))
}

fn svg_response(svg: Markup) -> Response {
  let headers =
    [(header::CONTENT_TYPE, "image/svg+xml"), (header::CACHE_CONTROL, "public,max-age=3600")];
  (headers, svg.into_string()).into_response()
}

/// Cards put repo names on public pages: only what GHS_FILTER and GHS_SHOW allow, never a private
/// repo, whatever GHS_INCLUDE_PRIVATE says
async fn public_repos(state: &Arc<AppState>) -> Res<Vec<RepoTotals>> {
  let mut repos = state.get_repos_shown(&RepoFilter::default()).await?;
  repos.retain(|x| !x.private);
  Ok(repos)
}

async fn card_repos(state: &Arc<AppState>, period: Period) -> Res<Vec<CardRepo>> {
  let repos = public_repos(state).await?;
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

/// A star line per name, a repo the cards can't show is a 404
async fn public_lines(state: &Arc<AppState>, names: &[&str]) -> Result<Vec<Line>, AppError> {
  let repos = public_repos(state).await?;
  let mut lines = Vec::new();
  for name in names {
    if !repos.iter().any(|x| x.name == *name) {
      return Err(AppError::status(StatusCode::NOT_FOUND));
    }

    let stars = dated_stars(state.db.get_stars(name).await?);
    lines.push(Line::new(name, stars.into_iter().map(|x| (x.0, x.1.into())).collect()));
  }

  Ok(lines)
}

/// Sum per date of each repo's latest known count
async fn total_points(state: &Arc<AppState>, repos: &[RepoTotals]) -> Res<Vec<Point>> {
  let mut changes = BTreeMap::<NaiveDate, i64>::new();
  for repo in repos {
    let mut prev = 0;
    for (date, stars) in dated_stars(state.db.get_stars(&repo.name).await?) {
      let stars = i64::from(stars);
      *changes.entry(date).or_default() += stars - prev;
      prev = stars;
    }
  }

  let mut total = 0;
  let mut points = Vec::new();
  for (date, change) in changes {
    total += change;
    points.push((date, total));
  }

  Ok(points)
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

/// Rounded up to a value whose half is round too
fn axis_top(value: i64) -> i64 {
  let value = value.max(2);
  let mut unit = 1;
  while unit * 10 < value {
    unit *= 10;
  }

  let tops = [12, 15, 20, 30, 40, 50, 60, 80, 100].map(|x| x * unit / 10);
  tops.into_iter().find(|x| *x >= value).unwrap_or(value)
}

/// Linear: zero, half and top. Log: powers of ten up to the next one, starting at 1
fn y_ticks(value: i64, log: bool) -> Vec<i64> {
  if !log {
    let top = axis_top(value);
    return vec![0, top / 2, top];
  }

  let mut ticks = vec![1, 10];
  while ticks[ticks.len() - 1] < value {
    ticks.push(ticks[ticks.len() - 1] * 10);
  }

  ticks
}

/// Even dates inside the chart, at most one per day of the range
fn x_labels(first: NaiveDate, last: NaiveDate) -> Vec<(NaiveDate, String)> {
  let days = (last - first).num_days();
  let format = if days > 300 { "%b %Y" } else { "%b %d" };
  let count = X_LABELS.min(days);
  let dates = (0..count).map(|x| first + Duration::days(days * (2 * x + 1) / (2 * count)));
  dates.map(|x| (x, x.format(format).to_string())).collect()
}

/// Round steps from the start of aligned lines: days, then years; the right edge stays free
fn day_labels(first: NaiveDate, last: NaiveDate) -> Vec<(NaiveDate, String)> {
  let days = (last - first).num_days();
  let steps = [1, 2, 5, 10, 20, 50, 100, 200, 365, 730, 1825];
  let step = steps.into_iter().find(|x| days / x <= X_LABELS + 1).unwrap_or(3650);

  let label = |x: i64| {
    if step < 365 { format!("Day {x}") } else { format!("Year {}", x / 365) }
  };

  let marks = (1..=(days - 1) / step).map(|x| x * step);
  marks.map(|x| (first + Duration::days(x), label(x))).collect()
}

/// Moves each line to start on the same date, the x axis then counts days from it
fn align_lines(lines: &mut [Line]) {
  let zero = NaiveDate::default();
  for line in lines {
    let start = line.points.first().map_or(zero, |x| x.0);
    for point in &mut line.points {
      point.0 = zero + (point.0 - start);
    }
  }
}

fn pick_points(points: &[Point]) -> Vec<Point> {
  if points.len() <= CHART_POINTS {
    return points.to_vec();
  }

  let last = points.len() - 1;
  (0..CHART_POINTS).map(|x| points[x * last / (CHART_POINTS - 1)]).collect()
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

  if matches!(card, Card::History) {
    css += &format!(
      ".line{{fill:none;stroke-width:4;stroke-linejoin:round;stroke-linecap:round}}\
      .grid{{stroke:{border}}}"
    );

    let lines = [palette.stars, palette.clones, palette.forks, palette.views];
    for (idx, color) in lines.iter().enumerate() {
      css += &format!(".line{idx}{{stroke:{color}}}.dot{idx}{{fill:{color}}}");
    }
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
  let (width, mark, right, drop) = match card {
    Card::History => (CHART_W, CHART_MARK_SIZE, CHART_MARK_RIGHT, CHART_MARK_DROP),
    _ => (W, MARK_SIZE, MARK_RIGHT, MARK_DROP),
  };

  html!(
    svg xmlns="http://www.w3.org/2000/svg" width=(width) height=(height)
      viewBox=(format!("0 0 {width} {height}")) font-family=(FONT) {
      title { (label) }
      // static text only: everything dynamic goes through maud's escaping
      style { (PreEscaped(style(opts.theme, card))) }
      rect x="0.5" y="0.5" width=(width - 1) height=(height - 1) rx="6" class="card" {}
      @if opts.brand {
        (text(width - right, height + drop, mark, "mark", Some(700), true, "ghstats"))
      }
      (body)
    }
  )
}

/// The owner of all `names`, when they have only one
fn shared_owner<'a>(names: impl Iterator<Item = &'a str>) -> Option<&'a str> {
  let mut owners = names.filter_map(|x| x.split('/').next());
  let first = owners.next()?;
  owners.all(|x| x == first).then_some(first)
}

/// `name` without its owner when all names on the card share one
fn short_name(name: &str, one_owner: bool) -> &str {
  name.split_once('/').filter(|_| one_owner).map_or(name, |x| x.1)
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

  let one_owner = shared_owner(rows.iter().map(|x| x.name.as_str())).is_some();
  let name = |x: &Row| cut_name(short_name(&x.name, one_owner), limit);

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

fn dot(x: f64, y: f64, size: f64, class: &str) -> Markup {
  html!(
    rect x=(format!("{:.1}", x - size / 2.0)) y=(format!("{:.1}", y - size / 2.0))
      width=(size) height=(size) rx=(size / 2.0) class=(class) {}
  )
}

fn legend(lines: &[Line]) -> Markup {
  let one_owner = shared_owner(lines.iter().map(|x| x.name.as_str())).is_some();
  let rows: Vec<(String, String)> = lines
    .iter()
    .map(|x| (cut_name(short_name(&x.name, one_owner), NAME_MAX_PLAIN), format_count(x.count)))
    .collect();

  let chars = rows.iter().map(|x| x.0.chars().count() + x.1.chars().count() + 1).max();
  let width = 2 * LEGEND_PAD + LEGEND_TEXT + chars.unwrap_or_default() as i32 * LEGEND_CHAR;
  let height = 2 * LEGEND_PAD + rows.len() as i32 * LEGEND_ROW - 6;

  html!(
    rect x=(LEGEND_X) y=(LEGEND_Y) width=(width) height=(height) rx="6" class="card"
      fill-opacity=(LEGEND_OPACITY) {}
    @for (idx, (name, count)) in rows.iter().enumerate() {
      @let y = LEGEND_Y + LEGEND_PAD + idx as i32 * LEGEND_ROW + 17;
      @let x = LEGEND_X + LEGEND_PAD;
      (dot(f64::from(x) + LEGEND_DOT / 2.0, f64::from(y - 6), LEGEND_DOT, &format!("dot{idx}")))
      text x=(x + LEGEND_TEXT) y=(y) font-size=(CHART_TEXT) class="ink" {
        (name) " " tspan class="muted" { (count) }
      }
    }
  )
}

fn chart(lines: &[Line], compare: bool, opts: &CardOpts) -> Markup {
  let points = || lines.iter().flat_map(|x| &x.points);
  let first = points().map(|x| x.0).min().unwrap_or_default();
  let last = points().map(|x| x.0).max().unwrap_or_default();
  let ticks = y_ticks(points().map(|x| x.1).max().unwrap_or_default(), opts.log);
  let top = ticks[ticks.len() - 1] as f64;

  let days = (last - first).num_days() as f64;
  let x_at = |date: NaiveDate| {
    f64::from(CHART_L) + f64::from(CHART_R - CHART_L) * (date - first).num_days() as f64 / days
  };

  let y_at = |value: i64| {
    let share = if opts.log { (value as f64).log10() / top.log10() } else { value as f64 / top };

    f64::from(CHART_B) - f64::from(CHART_B - CHART_T) * share
  };

  let aligned = compare && opts.align;
  let x_marks = if aligned { day_labels(first, last) } else { x_labels(first, last) };
  let area = !compare && !opts.log;

  html!(
    @for value in ticks {
      @let y = y_at(value);
      @let label = format_count(value);
      @let label = if opts.log { label.replace(".0", "") } else { label };
      line x1=(CHART_L) y1=(format!("{y:.1}")) x2=(CHART_R) y2=(format!("{y:.1}")) class="grid" {}
      (text(CHART_L - 8, y.round() as i32 + 6, CHART_TEXT, "muted", None, true, &label))
    }
    @for (idx, line) in lines.iter().enumerate() {
      @let picked = pick_points(&line.points);
      @let path: Vec<String> =
        picked.iter().map(|x| format!("{:.1} {:.1}", x_at(x.0), y_at(x.1))).collect();
      @let path = path.join("L");
      @if let Some(end) = picked.last() {
        @if area {
          path d=(format!("M{path}L{CHART_R} {CHART_B}L{CHART_L} {CHART_B}Z")) class="bar" {}
        }
        path d=(format!("M{path}")) class=(format!("line line{idx}")) {}
        (dot(x_at(end.0), y_at(end.1), DOT, &format!("dot{idx}")))
      }
    }
    @if compare {
      (legend(lines))
    }
    @for (date, label) in x_marks {
      text x=(format!("{:.1}", x_at(date))) y=(X_LABEL_Y) font-size=(CHART_TEXT) class="muted"
        text-anchor="middle" { (label) }
    }
  )
}

fn stars_note(count: i64) -> String {
  let unit = if count == 1 { "star" } else { "stars" };
  format!("{} {unit}", format_count(count))
}

fn chart_header(title: &str, note: &str) -> Markup {
  let left = text(CHART_PAD, CHART_HEAD_Y, CHART_TITLE_TEXT, "head", HEAD_WEIGHT, false, title);
  let right = text(CHART_R, CHART_HEAD_Y, CHART_NOTE_TEXT, "muted", None, true, note);
  html!((left) @if !note.is_empty() { (right) })
}

/// `compare` is the repos chart: a legend instead of the note, no area, the only one to align
fn history_card(title: &str, mut lines: Vec<Line>, compare: bool, opts: &CardOpts) -> Markup {
  let note = if compare { String::new() } else { stars_note(lines[0].count) };
  let label = if compare {
    let names: Vec<String> =
      lines.iter().map(|x| format!("{} {}", x.name, stars_note(x.count))).collect();
    format!("{title}: {}", names.join(", "))
  } else {
    format!("{title}, {note}")
  };

  let header = chart_header(&cut_name(title, NAME_MAX_PLAIN), &note);

  for line in &mut lines {
    line.points.retain(|x| opts.in_range(x.0));
  }

  if lines.iter().all(|x| x.points.len() < 2) {
    let empty = text(CHART_PAD, CHART_T + 8, CHART_NOTE_TEXT, "muted", None, false, "No data");
    return frame(CHART_EMPTY_H, &label, Card::History, opts, html!((header)(empty)));
  }

  if compare && opts.align {
    align_lines(&mut lines);
  }

  frame(CHART_H, &label, Card::History, opts, html!((header)(chart(&lines, compare, opts))))
}

#[cfg(test)]
mod tests {
  use std::time::{SystemTime, UNIX_EPOCH};

  use axum::body::to_bytes;
  use axum::http::Uri;

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

  async fn repo_response(
    state: &Arc<AppState>,
    repo: &str,
    query: &str,
  ) -> Result<Response, AppError> {
    let uri: Uri = format!("/card/{repo}/history.svg?{query}").parse().unwrap();
    let query = Query::try_from_uri(&uri).unwrap();
    let (owner, repo) = repo.split_once('/').unwrap();
    repo_card(State(state.clone()), Path((owner.to_string(), repo.to_string())), query).await
  }

  async fn repo_svg(state: &Arc<AppState>, repo: &str, query: &str) -> String {
    let rep = repo_response(state, repo, query).await.unwrap();
    let body = to_bytes(rep.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(body.to_vec()).unwrap()
  }

  async fn repo_status(state: &Arc<AppState>, repo: &str, query: &str) -> StatusCode {
    match repo_response(state, repo, query).await {
      Ok(x) => x.status(),
      Err(e) => e.into_response().status(),
    }
  }

  /// The drawn line as "x y" pairs
  fn line_points(svg: &str, idx: usize) -> Vec<&str> {
    let line = svg.split(&format!(r#" class="line line{idx}""#)).next().unwrap();
    line.rsplit_once(r#"d="M"#).unwrap().1.trim_end_matches('"').split('L').collect()
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
      "since=",
      "since=2023",
      "since=2023-13-01",
      "since=soon",
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

  #[tokio::test]
  async fn history_renders_the_chart_with_headers() {
    let state = test_state("history").await;
    seed_stats(&state, 1, "o/one", &[(40, 10, 0), (20, 30, 0), (0, 50, 0)]).await;
    seed_stats(&state, 2, "o/two", &[(40, 5, 0), (0, 7, 0)]).await;

    let rep = response(&state, "history.svg", "").await.unwrap();
    assert_eq!(rep.status(), StatusCode::OK);
    assert_eq!(rep.headers()[header::CONTENT_TYPE], "image/svg+xml");
    assert_eq!(rep.headers()[header::CACHE_CONTROL], "public,max-age=3600");

    let rep = repo_response(&state, "o/one", "").await.unwrap();
    assert_eq!(rep.headers()[header::CONTENT_TYPE], "image/svg+xml");
    assert_eq!(rep.headers()[header::CACHE_CONTROL], "public,max-age=3600");

    let total = svg(&state, "history.svg", "").await;
    assert!(total.contains(r#"width="800" height="480" viewBox="0 0 800 480""#));
    assert!(total.contains("<title>o, 57 stars</title>"));
    assert!(total.contains(">o<") && total.contains(">57 stars<"));
    assert_eq!(total.matches(r#"class="grid""#).count(), 3);
    assert_eq!(total.matches(r#"class="dot0""#).count(), 1);
    assert_eq!(total.matches(r#"text-anchor="middle""#).count(), 4);
    assert_eq!(line_points(&total, 0).len(), 3);
    assert_eq!(line_points(&total, 0)[0], "88.0 333.0", "15 of 60 at the left edge");
    assert_eq!(line_points(&total, 0)[2], "772.0 100.6", "57 of 60 at the right edge");

    let elements = total.split('<').skip(1).filter(|x| !x.starts_with('/'));
    let mut elements = elements.filter_map(|x| x.split([' ', '>']).next());
    let allowed = ["svg", "title", "style", "rect", "text", "line", "path"];
    assert!(elements.all(|x| allowed.contains(&x)));

    let one = repo_svg(&state, "o/one", "").await;
    assert!(one.contains(">o/one<") && one.contains(">50 stars<"));
    assert!(!one.contains(">o<"));

    for (theme, card, stars) in [("light", "#ffffff", "#2f95dc"), ("dark", "#181c25", "#4fb0f0")] {
      let body = svg(&state, "history.svg", &format!("theme={theme}")).await;
      assert!(body.contains(&format!(".card{{fill:{card}")), "{theme}");
      assert!(body.contains(&format!(".line0{{stroke:{stars}}}")), "{theme}");
      assert!(body.contains(&format!(".dot0{{fill:{stars}}}")), "{theme}");
    }

    let auto = svg(&state, "history.svg", "brand=0").await;
    assert_eq!(auto.matches(".line{").count(), 2, "both themes");
    assert!(!auto.contains(">ghstats<"));
    assert!(!svg(&state, "stars.svg", "").await.contains(".line{"));

    seed_stats(&state, 3, "x/three", &[(0, 1, 0)]).await;
    let total = svg(&state, "history.svg", "").await;
    assert!(total.contains("<title>All repositories, 58 stars</title>"));
    assert!(total.contains(">All repositories<") && total.contains(">58 stars<"));
    assert!(repo_svg(&state, "x/three", "").await.contains(">x/three<"));
  }

  #[tokio::test]
  async fn history_totals_carry_forward_and_sum() {
    let state = test_state("history-totals").await;
    // the zero row is a gap, not a drop to zero
    seed_stats(&state, 1, "o/a", &[(9, 0, 0), (8, 10, 0), (6, 0, 0), (4, 12, 0), (0, 15, 0)]).await;
    seed_stats(&state, 2, "o/b", &[(6, 100, 0), (2, 90, 0)]).await;

    let repos = public_repos(&state).await.unwrap();
    let day = |x: i64| Utc::now().date_naive() - Duration::days(x);
    let want = vec![(day(8), 10), (day(6), 110), (day(4), 112), (day(2), 102), (day(0), 105)];
    assert_eq!(total_points(&state, &repos).await.unwrap(), want);

    assert!(svg(&state, "history.svg", "").await.contains(">105 stars<"));
  }

  #[tokio::test]
  async fn history_period_limits_the_range() {
    let state = test_state("history-period").await;
    seed_stats(&state, 1, "o/r", &[(100, 1, 0), (50, 10, 0), (20, 20, 0), (5, 30, 0), (0, 40, 0)])
      .await;

    let cases =
      [("", 5), ("period=tracked", 5), ("period=90d", 4), ("period=30d", 3), ("period=7d", 2)];
    for (query, want) in cases {
      for body in [svg(&state, "history.svg", query).await, repo_svg(&state, "o/r", query).await] {
        assert_eq!(line_points(&body, 0).len(), want, "{query}");
        assert!(body.contains(">40 stars<"), "{query}");
      }
    }
  }

  #[tokio::test]
  async fn since_cuts_the_line_and_replaces_the_period() {
    let state = test_state("history-since").await;
    seed_stats(&state, 1, "o/r", &[(100, 1, 0), (50, 10, 0), (20, 20, 0), (5, 30, 0), (0, 40, 0)])
      .await;

    let since = |days_ago: i64| Utc::now().date_naive() - Duration::days(days_ago);
    let cases = [
      (format!("since={}", since(50)), 4),
      (format!("since={}", since(20)), 3),
      (format!("since={}&period=7d", since(50)), 4),
      (format!("since={}&period=tracked", since(20)), 3),
    ];

    for (query, want) in cases {
      for body in [svg(&state, "history.svg", &query).await, repo_svg(&state, "o/r", &query).await]
      {
        assert_eq!(line_points(&body, 0).len(), want, "{query}");
        assert!(body.contains(">40 stars<"), "{query}");
      }
    }

    let body = svg(&state, "history.svg", &format!("since={}", since(1))).await;
    assert!(body.contains(">No data<"), "one point left");
  }

  #[tokio::test]
  async fn history_leaves_out_private_and_show_hidden_repos() {
    let db = DbClient::new(&db_path("history-privacy")).await.unwrap();
    let mut state = AppState::for_test(db, "!o/filtered");
    state.show = Filter::parse("!o/hidden-by-show", true).unwrap();
    state.include_private = true;
    let state = Arc::new(state);

    seed_stats(&state, 1, "o/visible", &[(5, 5, 0), (0, 10, 0)]).await;
    seed_stats(&state, 2, "o/hidden-by-show", &[(5, 900, 0), (0, 1_000, 0)]).await;
    seed_stats(&state, 3, "o/secret", &[(5, 2_000, 0), (0, 3_000, 0)]).await;
    seed_stats(&state, 4, "o/filtered", &[(5, 4_000, 0), (0, 5_000, 0)]).await;

    let secret = Repo { private: true, ..sample_repo(3, "o/secret", 3_000, 0) };
    state.db.insert_repo(&secret).await.unwrap();

    assert!(svg(&state, "history.svg", "").await.contains(">10 stars<"));
    assert_eq!(repo_status(&state, "o/visible", "").await, StatusCode::OK);

    for repo in ["o/hidden-by-show", "o/secret", "o/filtered", "o/unknown"] {
      assert_eq!(repo_status(&state, repo, "").await, StatusCode::NOT_FOUND, "{repo}");
    }
  }

  #[tokio::test]
  async fn history_without_two_points_renders_no_data() {
    let state = test_state("history-empty").await;

    let body = svg(&state, "history.svg", "").await;
    assert_eq!(texts(&body).join(" "), "ghstats All repositories 0 stars No data");
    assert!(body.contains(r#"width="800" height="128""#));

    seed_stats(&state, 1, "o/one", &[(0, 1, 0)]).await;
    seed_stats(&state, 2, "o/old", &[(20, 4, 0), (10, 5, 0)]).await;

    let body = repo_svg(&state, "o/one", "").await;
    assert_eq!(texts(&body).join(" "), "ghstats o/one 1 star No data");
    assert!(body.contains("<title>o/one, 1 star</title>"));
    assert!(!body.contains(r#"class="line "#));

    let body = repo_svg(&state, "o/old", "period=7d").await;
    assert_eq!(texts(&body).join(" "), "ghstats o/old 5 stars No data", "the note stays current");
  }

  #[tokio::test]
  async fn repos_draw_a_line_each_with_a_legend() {
    let state = test_state("compare").await;
    seed_stats(&state, 1, "o/one", &[(40, 10, 0), (0, 50, 0)]).await;
    seed_stats(&state, 2, "o/two", &[(20, 5, 0), (0, 7, 0)]).await;
    seed_stats(&state, 3, "x/three", &[(3, 1, 0), (0, 2, 0)]).await;
    seed_stats(&state, 4, "x/old", &[(20, 4, 0), (10, 5, 0)]).await;

    let body = svg(&state, "history.svg", "repos=o/one,o/two").await;
    assert_eq!(body.matches(r#"class="line "#).count(), 2);
    assert_eq!(body.matches(r#"class="dot1""#).count(), 2, "the end dot and the legend");
    assert!(!body.contains(r#"class="bar""#));
    assert!(body.contains(">Star history<"));
    assert!(!texts(&body).iter().any(|x| x.ends_with(" stars")), "no note");
    assert!(body.contains(r#">one <tspan class="muted">50</tspan>"#));
    assert!(body.contains(r#">two <tspan class="muted">7</tspan>"#));
    assert!(body.contains("<title>Star history: o/one 50 stars, o/two 7 stars</title>"));

    assert_eq!(line_points(&body, 0)[0], "88.0 349.6", "the first date of all");
    assert_eq!(line_points(&body, 1)[0], "430.0 382.8", "its own first date, on the same scale");
    assert_eq!(line_points(&body, 1)[1], "772.0 369.5");

    let body = svg(&state, "history.svg", "repos=x/three,o/two").await;
    assert!(body.contains(r#">x/three <tspan"#), "two owners stay");
    assert_eq!(line_points(&body, 0).len(), 2, "colors follow the position in repos");
    assert!(line_points(&body, 0)[0].starts_with("669.4 "));

    // a repo without points in the period keeps its row, its color and no line
    let body = svg(&state, "history.svg", "repos=x/old,x/three&period=7d").await;
    assert!(!body.contains(r#"class="line line0""#) && body.contains(r#"class="line line1""#));
    assert!(body.contains(r#">old <tspan class="muted">5</tspan>"#));

    let body = svg(&state, "history.svg", "repos=x/old,o/two&period=7d&align=1").await;
    assert!(body.contains(">No data<") && body.contains(r#"height="128""#));
  }

  #[tokio::test]
  async fn repos_keep_to_four_and_flags_to_zero_or_one() {
    let state = test_state("compare-invalid").await;
    seed_stats(&state, 1, "o/r", &[(1, 1, 0), (0, 2, 0)]).await;

    let five = "repos=o/r,o/r,o/r,o/r,o/r";
    assert_eq!(status(&state, "history.svg", five).await, StatusCode::BAD_REQUEST);
    assert_eq!(status(&state, "history.svg", "repos=o/r,o/r,o/r,o/r").await, StatusCode::OK);

    for query in ["log=2", "log=", "log=true", "align=2", "align=", "align=true"] {
      assert_eq!(status(&state, "history.svg", query).await, StatusCode::BAD_REQUEST, "{query}");
      assert_eq!(repo_status(&state, "o/r", query).await, StatusCode::BAD_REQUEST, "{query}");

      let query = format!("repos=o/r&{query}");
      assert_eq!(status(&state, "history.svg", &query).await, StatusCode::BAD_REQUEST, "{query}");
    }

    let plain = svg(&state, "history.svg", "").await;
    for query in ["log=0", "align=0", "align=1"] {
      assert_eq!(svg(&state, "history.svg", query).await, plain, "{query}");
    }

    for query in ["log=2", "align=2"] {
      assert_eq!(status(&state, "stars.svg", query).await, StatusCode::BAD_REQUEST, "{query}");
    }
  }

  #[tokio::test]
  async fn repos_outside_the_public_set_are_a_404_without_their_name() {
    let db = DbClient::new(&db_path("compare-privacy")).await.unwrap();
    let mut state = AppState::for_test(db, "!o/filtered");
    state.show = Filter::parse("!o/hidden-by-show", true).unwrap();
    state.include_private = true;
    let state = Arc::new(state);

    seed_stats(&state, 1, "o/visible", &[(5, 5, 0), (0, 10, 0)]).await;
    seed_stats(&state, 2, "o/hidden-by-show", &[(5, 900, 0), (0, 1_000, 0)]).await;
    seed_stats(&state, 3, "o/secret", &[(5, 2_000, 0), (0, 3_000, 0)]).await;
    seed_stats(&state, 4, "o/filtered", &[(5, 4_000, 0), (0, 5_000, 0)]).await;

    let secret = Repo { private: true, ..sample_repo(3, "o/secret", 3_000, 0) };
    state.db.insert_repo(&secret).await.unwrap();

    assert_eq!(status(&state, "history.svg", "repos=o/visible").await, StatusCode::OK);

    for repo in ["o/hidden-by-show", "o/secret", "o/filtered", "o/unknown"] {
      let query = format!("repos=o/visible,{repo}");
      let rep = response(&state, "history.svg", &query).await.err().unwrap().into_response();
      assert_eq!(rep.status(), StatusCode::NOT_FOUND, "{repo}");

      let body = to_bytes(rep.into_body(), usize::MAX).await.unwrap();
      assert!(!String::from_utf8_lossy(&body).contains(repo), "{repo}");
    }
  }

  #[tokio::test]
  async fn log_axis_counts_powers_of_ten_without_an_area() {
    let state = test_state("log").await;
    seed_stats(&state, 1, "o/r", &[(20, 1, 0), (10, 10, 0), (0, 1_200, 0)]).await;

    let body = repo_svg(&state, "o/r", "log=1").await;
    assert_eq!(texts(&body)[3..8].join(" "), "1 10 100 1,000 10k");
    assert_eq!(body.matches(r#"class="grid""#).count(), 5);
    assert_eq!(line_points(&body, 0)[0], "88.0 416.0", "1 is the bottom");
    assert_eq!(line_points(&body, 0)[1], "430.0 333.0", "10 is one of four decades up");
    assert!(!body.contains(r#"class="bar""#));
    assert!(!svg(&state, "history.svg", "log=1").await.contains(r#"class="bar""#));

    let body = repo_svg(&state, "o/r", "log=0").await;
    assert_eq!(body, repo_svg(&state, "o/r", "").await);
    assert!(body.contains(r#"class="bar""#));

    let body = svg(&state, "history.svg", "repos=o/r&log=1").await;
    assert_eq!(body.matches(r#"class="grid""#).count(), 5);
  }

  #[tokio::test]
  async fn align_starts_every_line_at_day_zero() {
    let state = test_state("align").await;
    seed_stats(&state, 1, "o/one", &[(40, 1, 0), (0, 5, 0)]).await;
    seed_stats(&state, 2, "o/two", &[(20, 1, 0), (0, 3, 0)]).await;

    let body = svg(&state, "history.svg", "repos=o/one,o/two&align=1").await;
    assert!(line_points(&body, 0)[0].starts_with("88.0 "));
    assert!(line_points(&body, 1)[0].starts_with("88.0 "));
    assert!(line_points(&body, 0)[1].starts_with("772.0 "), "the longest life fills the axis");
    assert!(line_points(&body, 1)[1].starts_with("430.0 "), "20 of 40 days");
    assert!(body.contains(">Day 10<") && body.contains(">Day 30<"));

    let body = svg(&state, "history.svg", "repos=o/one,o/two&align=0").await;
    assert!(line_points(&body, 1)[0].starts_with("430.0 "));
    assert!(!body.contains(">Day "));
  }

  #[test]
  fn pick_points_spreads_evenly_and_keeps_the_last() {
    let start = Utc::now().date_naive();
    let points: Vec<Point> = (0..1_000).map(|x| (start + Duration::days(x), x)).collect();

    let picked = pick_points(&points);
    assert_eq!(picked.len(), CHART_POINTS);
    assert_eq!(picked[0], points[0]);
    assert_eq!(picked[CHART_POINTS - 1], points[999]);
    assert!(picked.windows(2).all(|x| (5..=6).contains(&(x[1].1 - x[0].1))));

    assert_eq!(pick_points(&points[..CHART_POINTS]), points[..CHART_POINTS]);
  }

  #[test]
  fn chart_axes_round_up_and_keep_the_label_count() {
    let cases = [
      (1, 2),
      (3, 3),
      (7, 8),
      (55, 60),
      (100, 100),
      (1_043, 1_200),
      (2_932, 3_000),
      (5_817, 6_000),
      (9_999, 10_000),
      (205_000, 300_000),
    ];

    for (value, want) in cases {
      assert_eq!(axis_top(value), want, "{value}");
    }

    let cases = [
      (1, "1 10"),
      (10, "1 10"),
      (11, "1 10 100"),
      (2_932, "1 10 100 1,000 10.0k"),
      (100_000, "1 10 100 1,000 10.0k 100k"),
    ];

    for (value, want) in cases {
      let ticks: Vec<String> = y_ticks(value, true).into_iter().map(format_count).collect();
      assert_eq!(ticks.join(" "), want, "{value}");
    }

    let cases = [
      (3, "Day 1 Day 2"),
      (40, "Day 10 Day 20 Day 30"),
      (100, "Day 20 Day 40 Day 60 Day 80"),
      (1_000, "Day 200 Day 400 Day 600 Day 800"),
      (1_276, "Year 1 Year 2 Year 3"),
      (3_650, "Year 2 Year 4 Year 6 Year 8"),
    ];

    for (days, want) in cases {
      let first = NaiveDate::default();
      let labels = day_labels(first, first + Duration::days(days));
      let labels: Vec<String> = labels.into_iter().map(|x| x.1).collect();
      assert_eq!(labels.join(" "), want, "{days}");
    }

    let cases = [
      ("2026-11-05", "2026-11-06", "Nov 05"),
      ("2026-11-04", "2026-11-06", "Nov 04 Nov 05"),
      ("2026-10-31", "2026-11-06", "Oct 31 Nov 02 Nov 03 Nov 05"),
      ("2026-01-01", "2026-10-28", "Feb 07 Apr 23 Jul 07 Sep 20"),
      ("2026-01-01", "2026-10-29", "Feb 2026 Apr 2026 Jul 2026 Sep 2026"),
      ("2023-05-09", "2026-11-05", "Oct 2023 Aug 2024 Jul 2025 May 2026"),
    ];

    for (first, last, want) in cases {
      let labels = x_labels(first.parse().unwrap(), last.parse().unwrap());
      let labels: Vec<String> = labels.into_iter().map(|x| x.1).collect();
      assert_eq!(labels.join(" "), want, "{first}");
    }
  }

  #[tokio::test]
  async fn history_rejects_invalid_params() {
    let state = test_state("history-invalid").await;
    seed_stats(&state, 1, "o/r", &[(1, 1, 0), (0, 2, 0)]).await;

    for query in ["period=year", "period=", "theme=blue", "brand=2", "since=2023-02-30"] {
      assert_eq!(status(&state, "history.svg", query).await, StatusCode::BAD_REQUEST, "{query}");
      assert_eq!(repo_status(&state, "o/r", query).await, StatusCode::BAD_REQUEST, "{query}");
    }
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
