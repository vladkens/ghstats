use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::HeaderMap;
use chrono::{Datelike, Days, NaiveDate, Utc};
use maud::{DOCTYPE, Markup, PreEscaped, html};
use serde::Serialize;
use thousands::Separable;
use url::form_urlencoded::Serializer;

use crate::AppState;
use crate::db_client::{
  BACKFILL_DAYS, CMP_LAG, DbClient, Direction, Period, PopularFilter, PopularKind, PopularSort,
  RepoDaily, RepoFilter, RepoPeriod, RepoPopularItem, RepoSort, RepoTotals,
};
use crate::routes::assets::vendor_url;
use crate::types::{AppError, HtmlRes};

const UNIQUE_TIP: &str =
  "Sum of GitHub's daily unique counts: someone who comes back on three days counts three times.";

// MARK: Helpers

fn get_hx_target(headers: &HeaderMap) -> Option<&str> {
  headers.get("hx-target").and_then(|value| value.to_str().ok())
}

fn get_custom_links() -> Vec<(String, String)> {
  let links = std::env::var("GHS_CUSTOM_LINKS").unwrap_or_default();
  let links: Vec<(String, String)> = links
    .split(",")
    .filter_map(|x| {
      let parts: Vec<&str> = x.split("|").collect();
      if parts.len() != 2 {
        return None;
      }

      if parts[0].is_empty() || parts[1].is_empty() {
        return None;
      }

      Some((parts[0].to_string(), parts[1].to_string()))
    })
    .collect();

  links
}

fn today() -> NaiveDate {
  Utc::now().date_naive()
}

fn num(x: i64) -> String {
  x.separate_with_commas()
}

fn signed(x: i64) -> String {
  match x {
    x if x > 0 => format!("+{}", num(x)),
    x if x < 0 => format!("−{}", num(-x)),
    _ => "0".to_string(),
  }
}

/// Direction a header click switches to: flip the current column, start names A-Z, numbers 9-0
fn next_dir(is_cur: bool, cur: &Direction, is_name: bool) -> &'static str {
  match (is_cur, cur, is_name) {
    (true, Direction::Desc, _) => "asc",
    (true, Direction::Asc, _) => "desc",
    (false, _, true) => "asc",
    (false, _, false) => "desc",
  }
}

fn aria_sort(is_cur: bool, dir: &Direction) -> Option<&'static str> {
  match (is_cur, dir) {
    (false, _) => None,
    (true, Direction::Asc) => Some("ascending"),
    (true, Direction::Desc) => Some("descending"),
  }
}

fn period_nav(cur: Period, oob: bool, url: impl Fn(Period) -> String) -> Markup {
  html!(
    nav id="period_nav" class="seg" aria-label="Period" hx-swap-oob=[oob.then_some("true")] {
      @for p in Period::ALL {
        a href=(url(p)) title=(p.title()) aria-current=[(p == cur).then_some("page")] {
          (p.label())
        }
      }
    }
  )
}

fn period_range(period: Period, today: NaiveDate, first: Option<NaiveDate>) -> String {
  let from = match period.windows(today) {
    Some(x) => x.start,
    None => first.unwrap_or(today),
  };

  match from.year() == today.year() {
    true => format!("{} – {}", from.format("%b %-d"), today.format("%b %-d, %Y")),
    false => format!("{} – {}", from.format("%b %-d, %Y"), today.format("%b %-d, %Y")),
  }
}

// MARK: Change

/// "Oct 25–31", "Sep 28–Oct 4" or "Nov 1, 2025–Oct 31, 2026"
fn short_range(from: NaiveDate, to: NaiveDate) -> String {
  match (from.year() == to.year(), from.month() == to.month()) {
    (true, true) => format!("{}–{}", from.format("%b %-d"), to.format("%-d")),
    (true, false) => format!("{}–{}", from.format("%b %-d"), to.format("%b %-d")),
    _ => format!("{}–{}", from.format("%b %-d, %Y"), to.format("%b %-d, %Y")),
  }
}

/// Change between the two compared windows of `Period::windows`; only when they are comparable:
/// both at least half collected and a previous value of 10 or more.
#[derive(Debug, PartialEq)]
struct Change {
  pct: f64,
  coverage: f64,
  title: String,
}

impl Change {
  /// `days`: collected days of the less collected window
  fn new(cur: i64, prev: i64, days: i64, period: Period, today: NaiveDate) -> Option<Self> {
    let (len, win) = (period.days()?, period.windows(today)?);
    let coverage = days as f64 / len as f64;
    if coverage < 0.5 || prev < 10 {
      return None;
    }

    let day = Days::new(1);
    let mut title = format!(
      "{} ({}) vs {} ({}) · last {CMP_LAG} days excluded, still incomplete",
      num(cur),
      short_range(win.cmp_start, win.cmp_end - day),
      num(prev),
      short_range(win.prev_start, win.cmp_start - day),
    );

    if coverage < 0.9 {
      title.push_str(&format!(" · only {:.0}% of days collected", coverage * 100.0));
    }

    Some(Self { pct: (cur - prev) as f64 / prev as f64 * 100.0, coverage, title })
  }

  fn approx(&self) -> bool {
    self.coverage < 0.9
  }

  fn class(&self) -> String {
    let dir = match self.pct {
      x if x.abs() < 1.0 => "",
      x if x > 0.0 => " up",
      _ => " down",
    };

    format!("delta{dir}{}", if self.approx() { " warn" } else { "" })
  }

  /// Whole percents from `whole_from` up ("12%"), one decimal below ("4.0%"); `None` when the
  /// change rounds to zero
  fn abs_text(&self, whole_from: f64) -> Option<String> {
    let abs = self.pct.abs();
    let tenths = (abs * 10.0).round() / 10.0;
    match tenths >= whole_from {
      true => Some(abs.round()).filter(|x| *x > 0.0).map(|x| format!("{x:.0}%")),
      false => Some(tenths).filter(|x| *x > 0.0).map(|x| format!("{x:.1}%")),
    }
  }

  fn tile(&self, period: Period) -> Markup {
    let val = match (self.abs_text(10.0), self.pct > 0.0) {
      (None, _) => "±0%".to_string(),
      (Some(x), true) => format!("▲ {x}"),
      (Some(x), false) => format!("▼ {x}"),
    };

    html!(span class=(self.class()) title=(self.title) { (val) " vs prev " (period.label()) })
  }

  fn short(&self) -> Markup {
    let val = match (self.abs_text(0.0), self.pct > 0.0) {
      (None, _) => "±0%".to_string(),
      (Some(x), true) => format!("+{x}"),
      (Some(x), false) => format!("−{x}"),
    };

    html!(small class=(self.class()) title=(self.title) { (val) })
  }
}

// MARK: Layout

fn base(state: &Arc<AppState>, navs: Vec<(String, Option<String>)>, inner: Markup) -> Markup {
  let (app_name, app_version) = (env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));

  let last_release = state.last_release.lock().unwrap().clone();
  let is_new_release = last_release != app_version;

  let title = match navs.len() {
    0 => app_name,
    _ => &format!("{} · {}", navs.last().unwrap().0, app_name),
  };

  let favicon = include_str!("../../assets/favicon.svg")
    .replace("\n", "")
    .replace("\"", "%22")
    .replace("#", "%23");
  let favicon = format!("data:image/svg+xml,{}", favicon);

  html!(
    (DOCTYPE)
    html {
      head {
        meta charset="utf-8";
        meta name="viewport" content="width=device-width, initial-scale=1";
        title { (title) }

        link rel="icon" type="image/svg+xml" href=(PreEscaped(favicon)) {}
        script src=(vendor_url("chart.umd.js")) {}
        script src=(vendor_url("luxon.min.js")) {}
        script src=(vendor_url("chartjs-adapter-luxon.umd.min.js")) {}
        script src=(vendor_url("htmx.min.js")) {}
        script { "htmx.config.historyCacheSize = 0;" }
        script { (PreEscaped(include_str!("../../assets/app.js"))) }
        style { (PreEscaped(include_str!("../../assets/app.css"))) }
      }
      body {
        div class="wrap" {
          header class="topbar" {
            nav class="crumbs" aria-label="Breadcrumb" {
              @if navs.is_empty() {
                span { "Repos" }
              } @else {
                a href="/" { "Repos" }
              }

              @for (name, url) in &navs {
                span class="sep" { "›" }
                @match url {
                  Some(url) => a href=(url) { (name) },
                  None => span { (name) },
                }
              }
            }

            div class="meta" {
              @for (name, url) in &get_custom_links() {
                a href=(url) target="_blank" { (name) }
              }

              @if is_new_release {
                a href=(format!("https://github.com/vladkens/ghstats/releases/tag/v{last_release}"))
                  target="_blank" title="New release available!" { "🚨" }
              }

              a class="ver" href="https://github.com/vladkens/ghstats" target="_blank" {
                (format!("{} v{}", app_name, app_version))
              }
            }
          }

          main { (inner) }
        }
      }
    }
  )
}

// MARK: Popular tables

#[derive(Debug)]
struct PopularRow {
  name: String,
  url: Option<String>,
  views: i64,
  uniques: i64,
}

impl PopularRow {
  fn new(repo: &str, kind: &PopularKind, x: RepoPopularItem) -> Self {
    // GitHub's 14-day rolling counters make summed uniques exceed views sometimes
    let uniques = x.uniques.min(x.count);
    let (name, url) = match kind {
      PopularKind::Refs => (x.name, None),
      PopularKind::Path => {
        let name = x.name.replacen(&format!("/{repo}"), "", 1);
        let name = if name.is_empty() { "/".to_string() } else { name };
        (name, Some(format!("https://github.com{}", x.name)))
      }
    };

    Self { name, url, views: x.count, uniques }
  }
}

fn popular_html(
  repo: &str,
  kind: &PopularKind,
  rows: Vec<PopularRow>,
  qs: &PopularFilter,
) -> Markup {
  let mut rows: Vec<PopularRow> = rows.into_iter().filter(|x| x.views > 0).collect();
  if rows.is_empty() {
    return html!();
  }

  // the row fill follows the sorted column, views when sorted by name
  let val = |x: &PopularRow| match qs.sort {
    PopularSort::Uniques => x.uniques,
    _ => x.views,
  };

  rows.sort_by(|a, b| {
    let ord = match qs.sort {
      PopularSort::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
      _ => val(a).cmp(&val(b)),
    };

    match qs.direction {
      Direction::Asc => ord,
      Direction::Desc => ord.reverse(),
    }
  });

  let max = rows.iter().map(val).max().unwrap_or(1).max(1);

  let (title, html_id) = match kind {
    PopularKind::Refs => ("Referring sites", "refs_table"),
    PopularKind::Path => ("Popular paths", "path_table"),
  };

  let cols =
    [(title, PopularSort::Name), ("Views", PopularSort::Count), ("Unique", PopularSort::Uniques)];

  let sort_url = |col: &PopularSort| {
    let dir = next_dir(qs.sort == *col, &qs.direction, *col == PopularSort::Name);
    format!("/{}?sort={}&direction={}&period={}", repo, col, dir, qs.period)
  };

  html!(
    div id=(html_id) class="card flush" {
      table class="data" {
        colgroup { col; col class="n"; col class="n"; }
        thead {
          tr {
            @for (name, col) in &cols {
              th scope="col" aria-sort=[aria_sort(qs.sort == *col, &qs.direction)] {
                button type="button"
                  hx-get=(sort_url(col))
                  hx-target=(format!("#{html_id}"))
                  hx-swap="outerHTML"
                  hx-replace-url="true"
                { (name) }
              }
            }
          }
        }

        tbody {
          @for x in &rows {
            tr style=(format!("--w:{:.1}%", val(x) as f64 / max as f64 * 100.0)) {
              td class="name" title=(x.name) {
                @match &x.url {
                  Some(url) => a href=(url) { (x.name) },
                  None => (x.name),
                }
              }
              td { (num(x.views)) }
              td { (num(x.uniques)) }
            }
          }
        }
      }
    }
  )
}

async fn popular_table(
  db: &DbClient,
  repo: &str,
  kind: &PopularKind,
  qs: &PopularFilter,
) -> HtmlRes {
  let items = db.get_popular_items(repo, kind, qs).await?;
  let rows = items.into_iter().map(|x| PopularRow::new(repo, kind, x)).collect();
  Ok(popular_html(repo, kind, rows, qs))
}

async fn repo_popular_tables(db: &DbClient, repo: &str, qs: &PopularFilter) -> HtmlRes {
  let tables = [
    popular_table(db, repo, &PopularKind::Refs, qs).await?,
    popular_table(db, repo, &PopularKind::Path, qs).await?,
  ];
  let tables: Vec<Markup> = tables.into_iter().filter(|x| !x.0.is_empty()).collect();

  let html = html!(
    div id="popular_tables" class=[(tables.len() > 1).then_some("grid2")] hidden[tables.is_empty()] {
      @for x in &tables { (x) }
    }
  );

  Ok(html)
}

// MARK: Repo page

/// Runs of 3+ days without a row, as (last day before, first day after)
fn outages(dat: &RepoDaily) -> Vec<(NaiveDate, NaiveDate)> {
  let Some(start) = dat.start.as_deref().and_then(|x| x.parse::<NaiveDate>().ok()) else {
    return vec![];
  };

  let day = |idx: usize| start + Days::new(idx as u64);
  let have: Vec<usize> =
    dat.days.iter().enumerate().filter(|x| x.1.is_some()).map(|x| x.0).collect();
  have.windows(2).filter(|x| x[1] - x[0] > 3).map(|x| (day(x[0]), day(x[1]))).collect()
}

/// Days without stars: traffic outages, plus runs of more than 3 days between real stars rows that
/// end after daily collection runs; before that (the first traffic day + BACKFILL_DAYS) star
/// history has rows only on days with new stars.
fn star_gaps(
  first: Option<NaiveDate>,
  outages: &[(NaiveDate, NaiveDate)],
  stars: &[NaiveDate],
) -> Vec<(NaiveDate, NaiveDate)> {
  let mut all = outages.to_vec();
  if let Some(first) = first {
    let from = first + Days::new(BACKFILL_DAYS);
    let runs = stars.windows(2).filter(|x| x[1] > from && (x[1] - x[0]).num_days() > 3);
    all.extend(runs.map(|x| (x[0], x[1])));
  }

  all.sort();
  let mut out: Vec<(NaiveDate, NaiveDate)> = vec![];
  for (a, b) in all {
    match out.last_mut() {
      Some(last) if a <= last.1 => last.1 = last.1.max(b),
      _ => out.push((a, b)),
    }
  }

  out
}

/// "+162 in 30d", approximate when the day before the period falls inside a stars gap or the
/// latest stars value is stale
fn stars_gain(
  rp: &RepoPeriod,
  p: Period,
  today: NaiveDate,
  gaps: &[(NaiveDate, NaiveDate)],
  last: Option<NaiveDate>,
) -> Option<(String, bool)> {
  let before = p.windows(today)?.start - Days::new(1);
  let inside = gaps.iter().any(|(a, b)| *a < before && before < *b);
  let stale = last.is_some_and(|x| (today - x).num_days() > 3);
  Some((format!("{} in {}", signed(rp.stars_gain()), p.label()), inside || stale))
}

const STARS_APPROX: &str =
  "Stars weren't collected on some days around the start or end of this period";

#[derive(Serialize)]
struct ChartData<'a> {
  today: String,
  period: i32,
  daily: &'a RepoDaily,
  stars: &'a [(&'a str, i32)],
  gaps: &'a [(NaiveDate, NaiveDate)],
  /// label of the shaded period on the stars chart, e.g. "+162 in 30d"
  window_label: Option<String>,
}

fn tile(title: &str, value: i64, sub: Markup, change: Option<Markup>) -> Markup {
  html!(
    div class="card kpi" {
      h2 { (title) }
      div class="value" { (num(value)) }
      div class="sub" { (sub) }
      @if let Some(x) = change { (x) }
    }
  )
}

fn traffic_tile(
  title: &str,
  total: i64,
  uniques: i64,
  change: Option<Change>,
  p: Period,
) -> Markup {
  let sub = html!(abbr title=(UNIQUE_TIP) { (num(uniques.min(total))) " unique" });
  tile(title, total, sub, change.map(|x| x.tile(p)))
}

fn traffic_card(title: &str, canvas_id: &str) -> Markup {
  html!(
    section class="card" {
      div class="chart-h" {
        h2 { (title) }
        div class="legend" {
          span { i class="sw total" {} "Total" }
          span { i class="sw unique" {} "Unique" }
          span class="gap-key" hidden { i class="sw hatch" {} "No data" }
        }
      }
      div class="chart" { canvas id=(canvas_id) role="img" aria-label=(format!("{title}: total and unique")) {} }
    }
  )
}

pub async fn repo_page(
  State(state): State<Arc<AppState>>,
  Path((owner, repo)): Path<(String, String)>,
  OriginalUri(uri): OriginalUri,
  headers: HeaderMap,
) -> HtmlRes {
  let repo = format!("{}/{}", owner, repo);
  let qs: Query<PopularFilter> = Query::try_from_uri(&uri)?;
  let db = &state.db;

  match get_hx_target(&headers) {
    Some("refs_table") => return popular_table(db, &repo, &PopularKind::Refs, &qs).await,
    Some("path_table") => return popular_table(db, &repo, &PopularKind::Path, &qs).await,
    Some("popular_tables") => return repo_popular_tables(db, &repo, &qs).await,
    _ => {}
  }

  let totals = match db.get_repo_totals(&repo).await? {
    Some(x) => x,
    None => return AppError::not_found(),
  };

  if !state.filter.is_included(&totals.name, totals.fork, totals.archived) {
    return AppError::not_found();
  }

  let (p, today) = (qs.period, today());
  let rp = db.get_periods(Some(&repo), p, today).await?.pop().unwrap_or_default();
  let daily = db.get_daily(&repo, today).await?;
  let stars = db.get_star_rows(&repo).await?;
  let stars: Vec<(&str, i32)> =
    stars.iter().filter_map(|x| Some((x.date.get(..10)?, x.stars))).collect();

  let has_views = daily.days.iter().flatten().any(|x| x[2] > 0);
  let has_clones = daily.days.iter().flatten().any(|x| x[0] > 0);
  let first = daily.start.as_deref().and_then(|x| x.parse::<NaiveDate>().ok());
  let star_days: Vec<NaiveDate> = stars.iter().filter_map(|x| x.0.parse().ok()).collect();
  let gaps = star_gaps(first, &outages(&daily), &star_days);

  let mut tiles = vec![];
  if has_views {
    let days = rp.prev_days.min(rp.cmp_days);
    let change = Change::new(rp.cmp_views, rp.prev_views, days, p, today);
    tiles.push(traffic_tile("Views", rp.views, rp.views_uniques, change, p));
  }

  if has_clones {
    let days = rp.prev_days.min(rp.cmp_days);
    let change = Change::new(rp.cmp_clones, rp.prev_clones, days, p, today);
    tiles.push(traffic_tile("Clones", rp.clones, rp.clones_uniques, change, p));
  }

  let gain = stars_gain(&rp, p, today, &gaps, star_days.last().copied());
  if !stars.is_empty() || rp.stars > 0 {
    let sub = match &gain {
      Some((x, true)) => html!(span title=(STARS_APPROX) { "≈ " (x) }),
      Some((x, false)) => html!((x)),
      None => html!(),
    };

    tiles.push(tile("Stars", rp.stars, sub, None));
  }

  let window_label = match (&gain, stars.is_empty()) {
    (Some((x, approx)), false) => Some(format!("{}{x}", if *approx { "≈ " } else { "" })),
    _ => None,
  };

  let data = ChartData {
    today: today.to_string(),
    period: p.0,
    daily: &daily,
    stars: &stars,
    gaps: &gaps,
    window_label,
  };

  let period_url =
    |x: Period| format!("/{}?sort={}&direction={}&period={}", repo, qs.sort, qs.direction, x);

  let charts: Vec<Markup> =
    [(has_views, "Views", "chart_views"), (has_clones, "Clones", "chart_clones")]
      .into_iter()
      .filter(|x| x.0)
      .map(|x| traffic_card(x.1, x.2))
      .collect();

  let html = html!(
    div class="stack" {
      div class="repo-head" {
        h1 { a href=(format!("https://github.com/{}", repo)) { (totals.name) } }
        @if let Some(desc) = totals.description.as_deref().filter(|x| !x.is_empty()) {
          p { (desc) }
        }
      }

      div class="periodbar" {
        span class="range" { (period_range(p, today, first)) }
        (period_nav(p, false, period_url))
      }

      @if !tiles.is_empty() {
        div class=(format!("kpis c{}", tiles.len())) {
          @for x in &tiles { (x) }
        }
      }

      @if !charts.is_empty() {
        div class=[(charts.len() > 1).then_some("grid2")] {
          @for x in &charts { (x) }
        }
      }

      @if !stars.is_empty() {
        section class="card" {
          div class="chart-h" {
            h2 { "Stars" }
            @if !gaps.is_empty() {
              div class="legend" { span { i class="sw hatch" {} "No data" } }
            }
          }
          div class="chart short" { canvas id="chart_stars" role="img" aria-label="Stars over time" {} }
        }
      }

      (repo_popular_tables(db, &repo, &qs).await?)
    }

    script { "window.renderRepo(" (PreEscaped(serde_json::to_string(&data)?)) ");" }
  );

  Ok(base(&state, vec![(repo, None)], html))
}

// MARK: Index

struct IndexRow<'a> {
  repo: &'a RepoTotals,
  per: RepoPeriod,
  today: NaiveDate,
}

type IndexCell = fn(&IndexRow, Period) -> Markup;

struct IndexCol {
  name: &'static str,
  sort: RepoSort,
  /// header names the period, e.g. "Views 30d"
  per: bool,
  class: &'static str,
  cell: IndexCell,
}

fn index_cols() -> Vec<IndexCol> {
  let name: IndexCell = |x, _| {
    let (owner, name) = x.repo.name.split_once('/').unwrap_or(("", &x.repo.name));
    html!(a href=(format!("/{}", x.repo.name)) title=(x.repo.name) {
      span class="owner" { (owner) "/" } (name)
    })
  };

  let views: IndexCell = |x, p| {
    let days = x.per.prev_days.min(x.per.cmp_days);
    let change = Change::new(x.per.cmp_views, x.per.prev_views, days, p, x.today);
    html!(span class="v" { (num(x.per.views)) } @if let Some(c) = change { (c.short()) })
  };

  let stars: IndexCell = |x, p| {
    let gain = x.per.stars_gain();
    html!(
      span class="v" { (num(x.per.stars)) }
      @if p.days().is_some() && gain != 0 { small class="muted" { (signed(gain)) } }
    )
  };

  vec![
    IndexCol { name: "Name", sort: RepoSort::Name, per: false, class: "c-name", cell: name },
    IndexCol { name: "Views", sort: RepoSort::Views, per: true, class: "c-views", cell: views },
    IndexCol {
      name: "Clones",
      sort: RepoSort::Clones,
      per: true,
      class: "c-clones opt2",
      cell: |x, _| html!((num(x.per.clones))),
    },
    IndexCol { name: "Stars", sort: RepoSort::Stars, per: false, class: "c-stars", cell: stars },
    IndexCol {
      name: "Forks",
      sort: RepoSort::Forks,
      per: false,
      class: "c-forks opt",
      cell: |x, _| html!((num(x.repo.forks as i64))),
    },
    IndexCol {
      name: "Issues",
      sort: RepoSort::Issues,
      per: false,
      class: "c-issues opt",
      cell: |x, _| html!((num(x.repo.issues as i64))),
    },
    IndexCol {
      name: "PRs",
      sort: RepoSort::Prs,
      per: false,
      class: "c-prs opt",
      cell: |x, _| html!((num(x.repo.prs as i64))),
    },
  ]
}

fn sort_index(rows: &mut [IndexRow], sort: &RepoSort, dir: &Direction) {
  rows.sort_by(|a, b| {
    let ord = match sort {
      RepoSort::Name => a.repo.name.to_lowercase().cmp(&b.repo.name.to_lowercase()),
      RepoSort::Stars => a.per.stars.cmp(&b.per.stars),
      RepoSort::Forks => a.repo.forks.cmp(&b.repo.forks),
      RepoSort::Watchers => a.repo.watchers.cmp(&b.repo.watchers),
      RepoSort::Issues => a.repo.issues.cmp(&b.repo.issues),
      RepoSort::Prs => a.repo.prs.cmp(&b.repo.prs),
      RepoSort::Clones => a.per.clones.cmp(&b.per.clones),
      RepoSort::Views => a.per.views.cmp(&b.per.views),
    };

    match dir {
      Direction::Asc => ord,
      Direction::Desc => ord.reverse(),
    }
  });
}

fn index_url(qs: &RepoFilter, sort: &RepoSort, dir: &str, period: Period) -> String {
  let mut query = Serializer::new(String::new());
  query.append_pair("sort", &sort.to_string());
  query.append_pair("direction", dir);
  query.append_pair("period", &period.to_string());
  if let Some(q) = qs.q.as_deref().filter(|x| !x.is_empty()) {
    query.append_pair("q", q);
  }

  if let Some(owner) = qs.owner.as_deref().filter(|x| !x.is_empty()) {
    query.append_pair("owner", owner);
  }

  format!("/?{}", query.finish())
}

// https://docs.rs/axum/latest/axum/extract/index.html#common-extractors
pub async fn index(
  State(state): State<Arc<AppState>>,
  OriginalUri(uri): OriginalUri,
  headers: HeaderMap,
) -> HtmlRes {
  let qs: Query<RepoFilter> = Query::try_from_uri(&uri)?;
  let (p, today) = (qs.period, today());
  let repos = state.get_repos_filtered(&qs).await?;
  let owners = state.get_owners().await?;

  let mut pers: HashMap<String, RepoPeriod> =
    state.db.get_periods(None, p, today).await?.into_iter().map(|x| (x.name.clone(), x)).collect();
  let mut rows: Vec<IndexRow> = repos
    .iter()
    .map(|x| IndexRow { repo: x, per: pers.remove(&x.name).unwrap_or_default(), today })
    .collect();
  sort_index(&mut rows, &qs.sort, &qs.direction);

  let cols = index_cols();
  let cur_q = qs.q.clone().unwrap_or_default();
  let cur_owner = qs.owner.clone().unwrap_or_default();

  let sort_url = |col: &RepoSort| {
    let dir = next_dir(qs.sort == *col, &qs.direction, *col == RepoSort::Name);
    index_url(&qs, col, dir, p)
  };

  let period_url = |x: Period| index_url(&qs, &qs.sort, &qs.direction.to_string(), x);

  let table_html = html!(
    table id="repos_table" class="data idx" {
      colgroup {
        @for col in &cols { col class=(col.class); }
      }
      thead {
        tr {
          @for col in &cols {
            th scope="col" class=(col.class) aria-sort=[aria_sort(qs.sort == col.sort, &qs.direction)] {
              button type="button"
                hx-get=(sort_url(&col.sort))
                hx-target="#repos_table"
                hx-swap="outerHTML"
                hx-replace-url="true"
              {
                (col.name)
                @if col.per { " " span class="per" { (p.label()) } }
              }
            }
          }
        }
      }
      tbody {
        @if rows.is_empty() && !cur_q.is_empty() {
          tr { td colspan=(cols.len()) class="muted" { "No repos match “" (cur_q) "”." } }
        }

        @for row in &rows {
          tr {
            @for col in &cols {
              td class=(col.class) { ((col.cell)(row, p)) }
            }
          }
        }
      }
    }
  );

  let filter_inputs = |oob: bool| {
    let oob = oob.then_some("true");
    html!(
      input type="hidden" id="filter_sort" name="sort" value=(qs.sort) hx-swap-oob=[oob] {}
      input type="hidden" id="filter_direction" name="direction" value=(qs.direction) hx-swap-oob=[oob] {}
      input type="hidden" id="filter_period" name="period" value=(p) hx-swap-oob=[oob] {}
    )
  };

  if let Some("repos_table") = get_hx_target(&headers) {
    return Ok(html!((table_html)(filter_inputs(true))(period_nav(p, true, period_url))));
  }

  let include = "#filter_sort, #filter_direction, #filter_period";
  let html = html!(
    div class="toolbar" {
      (filter_inputs(false))

      @if owners.len() > 1 {
        select name="owner" aria-label="Owner"
          hx-get="/"
          hx-target="#repos_table"
          hx-swap="outerHTML"
          hx-include=(format!("[name='q'], {include}"))
          hx-push-url="true"
        {
          option value="" selected[cur_owner.is_empty()] { "All owners" }
          @for owner in &owners {
            option value=(owner) selected[*owner == cur_owner] { (owner) }
          }
        }
      }

      input type="search" name="q" value=(cur_q)
        placeholder="Search repos…"
        aria-label="Search repos"
        hx-get="/"
        hx-trigger="keyup changed delay:300ms, search"
        hx-target="#repos_table"
        hx-swap="outerHTML"
        hx-include=(format!("[name='owner'], {include}"))
        hx-replace-url="true"
      {}

      (period_nav(p, false, period_url))
    }

    div class="card flush" {
      div class="tscroll" { (table_html) }
    }
  );

  Ok(base(&state, vec![], html))
}

#[cfg(test)]
mod tests {
  use std::time::{SystemTime, UNIX_EPOCH};

  use super::*;
  use crate::helpers::GhsFilter;

  fn db_path(name: &str) -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    std::env::temp_dir()
      .join(format!("ghstats-html-{name}-{}-{nanos}.db", std::process::id()))
      .to_string_lossy()
      .into_owned()
  }

  #[tokio::test]
  async fn base_starts_with_doctype() {
    let db = DbClient::new(&db_path("base")).await.unwrap();
    let state = Arc::new(AppState::for_test(db, GhsFilter::new("")));
    let page = base(&state, vec![], html!()).into_string();

    assert!(page.starts_with("<!DOCTYPE html><html><head><meta charset=\"utf-8\">"));
    assert_eq!(page.matches("<!DOCTYPE html>").count(), 1);
    assert!(!page.to_lowercase().contains("pico"));
    assert!(!page.contains("https://cdn") && !page.contains("unpkg.com"));
  }

  #[test]
  fn change_needs_half_collected_and_base_of_ten() {
    let (p, today) = (Period(30), "2026-11-02".parse::<NaiveDate>().unwrap());
    let full = Change::new(110, 100, 30, p, today).unwrap();
    assert_eq!((full.pct, full.coverage), (10.0, 1.0));
    assert!(Change::new(110, 100, 14, p, today).is_none());
    assert!(Change::new(110, 9, 30, p, today).is_none());
    assert!(Change::new(110, 100, 30, Period(-1), today).is_none());

    // both compared ranges and values in the title, always
    let html = full.tile(p).into_string();
    assert!(html.contains("class=\"delta up\""));
    assert!(html.contains("▲ 10% vs prev 30d"));
    assert!(html.contains(
      "title=\"110 (Oct 2–31) vs 100 (Sep 2–Oct 1) · last 2 days excluded, still incomplete\""
    ));

    let approx = Change::new(97, 100, 20, p, today).unwrap();
    assert!(approx.approx());
    let html = approx.tile(p).into_string();
    assert!(html.contains("class=\"delta down warn\""));
    assert!(html.contains("▼ 3.0% vs prev 30d"));
    assert!(html.contains("still incomplete · only 67% of days collected"));
    assert!(approx.short().into_string().contains(">−3%<"));

    // tiles: one decimal under 10%; index: whole percents; ±0% when it rounds to zero
    let fmt = |cur: i64, prev: i64| {
      let x = Change::new(cur, prev, 30, p, today).unwrap();
      let tile = x.tile(p).into_string();
      let tile = tile[tile.find('>').unwrap() + 1..tile.find(" vs prev").unwrap()].to_string();
      let short = x.short().into_string();
      let short = short[short.find('>').unwrap() + 1..short.rfind('<').unwrap()].to_string();
      (tile, short)
    };

    assert_eq!(fmt(1040, 1000), ("▲ 4.0%".into(), "+4%".into()));
    assert_eq!(fmt(1120, 1000), ("▲ 12%".into(), "+12%".into()));
    assert_eq!(fmt(3240, 1000), ("▲ 224%".into(), "+224%".into()));
    assert_eq!(fmt(916, 1000), ("▼ 8.4%".into(), "−8%".into()));
    assert_eq!(fmt(10996, 10000), ("▲ 10%".into(), "+10%".into()), "no 10.0% at the cutoff");
    assert_eq!(fmt(10096, 10000), ("▲ 1.0%".into(), "+1%".into()));
    assert_eq!(fmt(1004, 1000), ("▲ 0.4%".into(), "±0%".into()));
    assert_eq!(fmt(100004, 100000), ("±0%".into(), "±0%".into()), "never +0.0%");
    assert_eq!(fmt(100, 100), ("±0%".into(), "±0%".into()));
    assert_eq!(Change::new(1004, 1000, 30, p, today).unwrap().class(), "delta");

    let year = Change::new(20, 10, 365, Period(365), today).unwrap();
    assert!(
      year.title.starts_with("20 (Nov 1, 2025–Oct 31, 2026) vs 10 (Nov 1, 2024–Oct 31, 2025)")
    );
  }

  #[test]
  fn signed_uses_plus_and_minus_signs() {
    assert_eq!(signed(1234), "+1,234");
    assert_eq!(signed(-5), "−5");
    assert_eq!(signed(0), "0");
  }

  #[test]
  fn star_gaps_merge_outages_and_runs_without_stars() {
    let day = |x: &str| x.parse::<NaiveDate>().unwrap();
    let first = Some(day("2026-01-01"));

    // sparse stargazer history before daily rows start (Jan 14) is normal, later runs are gaps
    let stars =
      ["2025-12-01", "2026-01-10", "2026-01-14", "2026-01-15", "2026-01-17", "2026-01-21"];
    let stars = stars.map(day);
    assert_eq!(star_gaps(first, &[], &stars), vec![(day("2026-01-17"), day("2026-01-21"))]);
    assert!(star_gaps(None, &[], &stars).is_empty());

    // rows stop after Apr 1 and resume May 17 with backfilled traffic but no stars until Jun 1
    let stars = ["2026-03-31", "2026-04-01", "2026-06-01", "2026-06-02"].map(day);
    let outage = [(day("2026-04-01"), day("2026-05-17"))];
    assert_eq!(star_gaps(first, &outage, &stars), vec![(day("2026-04-01"), day("2026-06-01"))]);
  }

  #[test]
  fn stars_gain_is_signed_and_approximate_in_gaps() {
    let day = |x: &str| x.parse::<NaiveDate>().unwrap();
    let today = day("2026-11-02");
    let rp = RepoPeriod { stars: 15, stars_start: 20, ..Default::default() };
    let gaps = [(day("2026-10-20"), day("2026-10-30"))];
    let last = Some(today);

    // 7d starts Oct 27, the day before is inside the gap
    assert_eq!(stars_gain(&rp, Period(7), today, &[], last), Some(("−5 in 7d".into(), false)));
    assert_eq!(stars_gain(&rp, Period(7), today, &gaps, last), Some(("−5 in 7d".into(), true)));
    assert_eq!(stars_gain(&rp, Period(30), today, &gaps, last).unwrap().1, false);
    assert_eq!(stars_gain(&rp, Period(30), today, &[], Some(day("2026-10-28"))).unwrap().1, true);
    assert!(stars_gain(&rp, Period(-1), today, &gaps, last).is_none());
  }

  #[test]
  fn popular_rows_clamp_uniques_and_fill_by_sorted_column() {
    let item = |name: &str, count, uniques| RepoPopularItem { name: name.into(), count, uniques };
    let kind = PopularKind::Path;
    let rows = || {
      [item("/o/r", 10, 14), item("/o/r/issues", 40, 2), item("/o/r/pulls", 0, 3)]
        .into_iter()
        .map(|x| PopularRow::new("o/r", &kind, x))
        .collect::<Vec<_>>()
    };

    let qs = PopularFilter { sort: PopularSort::Uniques, ..Default::default() };
    let html = popular_html("o/r", &kind, rows(), &qs).into_string();
    assert!(html.contains("<td>10</td><td>10</td>"), "unique clamped to views: {html}");
    assert!(!html.contains("/pulls"), "rows without views are dropped");
    assert!(html.find("title=\"/\"").unwrap() < html.find("title=\"/issues\"").unwrap());
    assert!(html.contains("style=\"--w:100.0%\"><td class=\"name\" title=\"/\">"));
    assert!(html.contains("style=\"--w:20.0%\"><td class=\"name\" title=\"/issues\">"));
    assert!(html.contains("href=\"https://github.com/o/r/issues\""));
    assert!(html.contains("aria-sort=\"descending\"><button"));

    let qs =
      PopularFilter { sort: PopularSort::Name, direction: Direction::Asc, ..Default::default() };
    let html = popular_html("o/r", &kind, rows(), &qs).into_string();
    assert!(html.contains("style=\"--w:25.0%\"><td class=\"name\" title=\"/\">"), "{html}");
    assert!(html.contains("sort=name&amp;direction=desc&amp;period=30d"));
    assert!(html.contains("sort=count&amp;direction=desc&amp;period=30d"));

    let empty = vec![PopularRow::new("o/r", &kind, item("/o/r", 0, 0))];
    assert!(popular_html("o/r", &kind, empty, &qs).into_string().is_empty());
  }

  #[test]
  fn outages_are_runs_of_three_missing_days() {
    let some = Some([1, 1, 1, 1]);
    let dat = RepoDaily {
      start: Some("2026-01-01".into()),
      days: vec![some, None, None, some, None, None, None, some, None],
    };

    let day = |x: &str| x.parse::<NaiveDate>().unwrap();
    assert_eq!(outages(&dat), vec![(day("2026-01-04"), day("2026-01-08"))]);
    assert!(outages(&RepoDaily::default()).is_empty());
  }

  #[test]
  fn period_range_and_nav() {
    let today = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
    let first = NaiveDate::from_ymd_opt(2024, 7, 10);
    assert_eq!(period_range(Period(30), today, first), "Oct 4 – Nov 2, 2026");
    assert_eq!(period_range(Period(365), today, first), "Nov 3, 2025 – Nov 2, 2026");
    assert_eq!(period_range(Period(-1), today, first), "Jul 10, 2024 – Nov 2, 2026");

    let nav = period_nav(Period(90), false, |x| format!("/?period={x}")).into_string();
    assert!(
      nav.contains("<a href=\"/?period=90d\" title=\"Last 90 days\" aria-current=\"page\">90d</a>")
    );
    assert!(nav.contains("<a href=\"/?period=all\" title=\"All time\">All</a>"));
    assert!(!nav.contains("hx-swap-oob"));
  }

  #[tokio::test]
  async fn repo_page_renders_only_blocks_with_data() {
    use crate::gh_client::{Repo, RepoReferrer};

    let db = DbClient::new(&db_path("blocks")).await.unwrap();
    let repo = Repo {
      id: 1,
      full_name: "o/r".into(),
      description: None,
      stargazers_count: 5,
      forks_count: 0,
      watchers_count: 0,
      open_issues_count: 0,
      fork: false,
      archived: false,
      has_pull_requests: None,
    };
    db.insert_repo(&repo).await.unwrap();
    db.insert_stats(&repo, &format!("{}T00:00:00Z", today()), &[]).await.unwrap();
    db.insert_referrers(
      &repo,
      &format!("{}T00:00:00Z", today()),
      &[RepoReferrer { referrer: "google.com".into(), count: 3, uniques: 2 }],
    )
    .await
    .unwrap();
    db.update_deltas().await.unwrap();

    let state = Arc::new(AppState::for_test(db, GhsFilter::new("")));
    let get = |uri: &str, target: Option<&str>| {
      let mut headers = HeaderMap::new();
      if let Some(x) = target {
        headers.insert("hx-target", x.parse().unwrap());
      }

      repo_page(
        State(state.clone()),
        Path(("o".into(), "r".into())),
        OriginalUri(uri.parse().unwrap()),
        headers,
      )
    };

    let page = get("/o/r?period=14", None).await.unwrap().into_string();
    assert!(page.contains("<h2>Stars</h2>"));
    assert!(page.contains("+5 in 14d"));
    assert!(!page.contains("<h2>Views</h2>"), "no traffic, no views tile");
    assert!(!page.contains("id=\"chart_views\""));
    assert!(page.contains("id=\"refs_table\""));
    assert!(!page.contains("id=\"path_table\""), "no paths, no paths table");
    assert!(page.contains("kpis c1"));

    let frag = get("/o/r?sort=name&period=7d", Some("refs_table")).await.unwrap().into_string();
    assert!(frag.starts_with("<div id=\"refs_table\""));
    assert!(!frag.contains("<html"));
  }

  #[tokio::test]
  async fn index_fragment_keeps_filters_and_names_period() {
    let db = DbClient::new(&db_path("index")).await.unwrap();
    let state = Arc::new(AppState::for_test(db, GhsFilter::new("")));
    let mut headers = HeaderMap::new();
    headers.insert("hx-target", "repos_table".parse().unwrap());

    let uri = "/?sort=views_count&direction=desc&period=90d&q=x".parse().unwrap();
    let frag = index(State(state), OriginalUri(uri), headers).await.unwrap().into_string();
    assert!(frag.starts_with("<table id=\"repos_table\""));
    assert!(frag.contains("Views <span class=\"per\">90d</span>"));
    assert!(
      frag.contains("id=\"filter_period\" name=\"period\" value=\"90d\" hx-swap-oob=\"true\"")
    );
    assert!(frag.contains("id=\"period_nav\""));
    assert!(frag.contains("href=\"/?sort=views_count&amp;direction=desc&amp;period=7d&amp;q=x\""));
    assert!(frag.contains("No repos match"));
  }

  fn test_repo(stars: u32) -> crate::gh_client::Repo {
    crate::gh_client::Repo {
      id: 1,
      full_name: "o/r".into(),
      description: None,
      stargazers_count: stars,
      forks_count: 0,
      watchers_count: 0,
      open_issues_count: 0,
      fork: false,
      archived: false,
      has_pull_requests: None,
    }
  }

  fn views(days: &[(NaiveDate, u32)]) -> crate::gh_client::RepoViews {
    let views = days
      .iter()
      .map(|(d, c)| crate::gh_client::TrafficDaily {
        timestamp: format!("{d}T00:00:00Z"),
        count: *c,
        uniques: *c / 2,
      })
      .collect();

    crate::gh_client::RepoViews { count: 0, uniques: 0, views }
  }

  #[tokio::test]
  async fn change_skips_incomplete_last_days_and_star_gap_is_approximate() {
    let db = DbClient::new(&db_path("flat")).await.unwrap();
    let today = today();
    let ago = |x: u64| today - Days::new(x);
    db.insert_repo(&test_repo(20)).await.unwrap();

    // daily stats, except 10 days (ago 12..3) that only have backfilled traffic without stars
    for x in (0..40).filter(|x| !(3..=12).contains(x)) {
      let stars = if x > 12 { 20 } else { 15 };
      db.insert_stats(&test_repo(stars), &format!("{}T00:00:00Z", ago(x)), &[]).await.unwrap();
    }

    // flat 100 views a day, then GitHub's still incomplete yesterday and today
    let mut days: Vec<(NaiveDate, u32)> = (2..40).map(|x| (ago(x), 100)).collect();
    days.extend([(ago(1), 5), (today, 0)]);
    db.insert_views(&test_repo(20), &views(&days)).await.unwrap();

    let rp = db.get_periods(Some("o/r"), Period(7), today).await.unwrap().remove(0);
    assert_eq!((rp.views, rp.cmp_views, rp.prev_views), (505, 700, 700));
    assert_eq!((rp.cmp_days, rp.prev_days), (7, 7));
    let change = Change::new(rp.cmp_views, rp.prev_views, 7, Period(7), today).unwrap();
    assert!(change.pct.abs() < f64::EPSILON);

    let state = Arc::new(AppState::for_test(db, GhsFilter::new("")));
    let uri = "/o/r?period=7d".parse().unwrap();
    let page =
      repo_page(State(state), Path(("o".into(), "r".into())), OriginalUri(uri), HeaderMap::new())
        .await
        .unwrap()
        .into_string();

    assert!(page.contains("±0% vs prev 7d"), "flat traffic is flat");
    assert_eq!(page.matches(" vs prev ").count(), 1, "views only, no stars change");
    assert!(page.contains(&format!("\"gaps\":[[\"{}\",\"{}\"]]", ago(13), ago(2))));
    assert!(page.contains(">≈ −5 in 7d</span>"), "stars before the period are in the gap");
    assert!(page.contains("\"window_label\":\"≈ −5 in 7d\""));
    assert!(page.contains("id=\"chart_views\""));
    assert!(!page.contains("<h2>Clones</h2>"), "no clones ever, no clones tile");
    assert!(!page.contains("id=\"chart_clones\""));
    assert!(page.contains("kpis c2"));
  }

  #[tokio::test]
  async fn change_needs_both_compared_windows_collected() {
    let today = today();
    let ago = |x: u64| today - Days::new(x);

    // a year of complete history before, and an outage inside the current compared year
    for (outage, approx) in [(60, Some(true)), (200, None)] {
      let db = DbClient::new(&db_path("cmp")).await.unwrap();
      db.insert_repo(&test_repo(1)).await.unwrap();
      let days: Vec<(NaiveDate, u32)> =
        (2..800).filter(|x| !(41..41 + outage).contains(x)).map(|x| (ago(x), 100)).collect();
      db.insert_views(&test_repo(1), &views(&days)).await.unwrap();

      let rp = db.get_periods(Some("o/r"), Period(365), today).await.unwrap().remove(0);
      assert_eq!((rp.prev_days, rp.cmp_days), (365, 365 - outage as i64));
      let days = rp.prev_days.min(rp.cmp_days);
      let change = Change::new(rp.cmp_views, rp.prev_views, days, Period(365), today);
      assert_eq!(change.as_ref().map(|x| x.approx()), approx, "outage {outage}");
    }
  }

  #[tokio::test]
  async fn stars_after_outage_use_real_rows_only() {
    let db = DbClient::new(&db_path("outage")).await.unwrap();
    let day = |x: &str| x.parse::<NaiveDate>().unwrap();
    db.insert_repo(&test_repo(10)).await.unwrap();

    // a month of daily stats, past the backfill of the first traffic day
    for x in 0..32 {
      let d = day("2026-03-01") + Days::new(x);
      db.insert_stats(&test_repo(10), &format!("{d}T00:00:00Z"), &[]).await.unwrap();
    }

    // backfill after the outage: traffic rows with no stars, then daily stats resume
    let backfill: Vec<(NaiveDate, u32)> =
      (17..=31).map(|x| (day(&format!("2026-05-{x}")), 50)).collect();
    db.insert_views(&test_repo(10), &views(&[(day("2026-03-01"), 50)])).await.unwrap();
    db.insert_views(&test_repo(10), &views(&backfill)).await.unwrap();
    for d in ["2026-06-01", "2026-06-02"] {
      db.insert_stats(&test_repo(30), &format!("{d}T00:00:00Z"), &[]).await.unwrap();
    }

    let dat = db.get_daily("o/r", day("2026-06-02")).await.unwrap();
    let rows = db.get_star_rows("o/r").await.unwrap();
    let stars: Vec<NaiveDate> = rows.iter().map(|x| x.date[..10].parse().unwrap()).collect();
    assert_eq!(outages(&dat), vec![(day("2026-04-01"), day("2026-05-17"))]);
    let gaps = star_gaps(dat.start.as_deref().map(day), &outages(&dat), &stars);
    assert_eq!(gaps, vec![(day("2026-04-01"), day("2026-06-01"))]);
    assert_eq!(rows.len(), 34);

    // get_stars (badges) keeps its forward fill
    let filled = db.get_stars("o/r").await.unwrap();
    assert!(filled.iter().any(|x| x.date.starts_with("2026-05-20") && x.stars == 10));

    // 7d ending Jun 2: start May 27 is inside the gap, the last real value before it is Apr 1's
    let rp = db.get_periods(Some("o/r"), Period(7), day("2026-06-02")).await.unwrap().remove(0);
    assert_eq!((rp.stars, rp.stars_start, rp.stars_gain()), (30, 10, 20));
  }

  #[tokio::test]
  async fn sparse_first_days_are_zero_traffic_not_outages() {
    let db = DbClient::new(&db_path("backfill")).await.unwrap();
    let today = today();
    let ago = |x: u64| today - Days::new(x);
    db.insert_repo(&test_repo(5)).await.unwrap();

    // first sync: stargazer history from 2017 and the days of the 14-day backfill that had traffic
    db.insert_stars(
      1,
      &[("2017-05-01T00:00:00Z".into(), 3, 3), (format!("{}T00:00:00Z", ago(38)), 4, 1)],
    )
    .await
    .unwrap();
    db.insert_views(&test_repo(5), &views(&[(ago(40), 10), (ago(35), 10), (ago(30), 10)]))
      .await
      .unwrap();
    for x in 0..=25 {
      db.insert_stats(&test_repo(5), &format!("{}T00:00:00Z", ago(x)), &[]).await.unwrap();
    }

    let dat = db.get_daily("o/r", today).await.unwrap();
    assert_eq!(dat.start, Some(ago(40).to_string()));
    assert_eq!(dat.days[1], Some([0; 4]), "a missing backfill day is a day without traffic");
    assert!(dat.days.iter().all(|x| x.is_some()));
    assert!(outages(&dat).is_empty());

    let rows = db.get_star_rows("o/r").await.unwrap();
    let stars: Vec<NaiveDate> = rows.iter().map(|x| x.date[..10].parse().unwrap()).collect();
    assert!(star_gaps(Some(ago(40)), &outages(&dat), &stars).is_empty());

    // compared ago 31..2: 6 backfill days + 24 rows; previous ago 61..32: the 9 backfill days
    let rp = db.get_periods(Some("o/r"), Period(30), today).await.unwrap().remove(0);
    assert_eq!((rp.cmp_days, rp.prev_days), (30, 9));
  }
}
