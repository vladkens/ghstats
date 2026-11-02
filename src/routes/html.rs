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

fn period_mark(tip: &str) -> Markup {
  const ICON: &str = "<svg viewBox=\"0 0 12 12\" aria-hidden=\"true\"><rect x=\"1.5\" y=\"2.5\" \
    width=\"9\" height=\"8\" rx=\"1.5\" fill=\"none\" stroke=\"currentColor\"/><path d=\"M1.5 \
    5h9M4 1v3M8 1v3\" stroke=\"currentColor\"/></svg>";

  html!(span class="per" role="img" aria-label=(tip) title=(tip) { (PreEscaped(ICON)) })
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
                a href=(url) target="_blank" rel="noopener" { (name) }
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

// MARK: Change

const UNIQUE_TIP: &str =
  "Sum of GitHub's daily unique counts: someone who comes back on three days counts three times.";

fn short_range(from: NaiveDate, to: NaiveDate) -> String {
  match (from.year() == to.year(), from.month() == to.month()) {
    (true, true) => format!("{}–{}", from.format("%b %-d"), to.format("%-d")),
    (true, false) => format!("{}–{}", from.format("%b %-d"), to.format("%b %-d")),
    _ => format!("{}–{}", from.format("%b %-d, %Y"), to.format("%b %-d, %Y")),
  }
}

#[derive(Debug, PartialEq)]
struct Change {
  pct: f64,
  coverage: f64,
  title: String,
}

impl Change {
  fn new(
    cur: i64,
    cur_days: i64,
    prev: i64,
    prev_days: i64,
    period: Period,
    today: NaiveDate,
  ) -> Option<Self> {
    Self::compare(cur, cur_days, prev, prev_days, period, today).ok()
  }

  fn compare(
    cur: i64,
    cur_days: i64,
    prev: i64,
    prev_days: i64,
    period: Period,
    today: NaiveDate,
  ) -> Result<Self, &'static str> {
    let (Some(len), Some(win)) = (period.days(), period.windows(today)) else {
      return Err("No previous period for all time");
    };

    let coverage = cur_days.min(prev_days) as f64 / len as f64;
    if coverage < 0.5 {
      return Err("Not enough days collected to compare with the previous period");
    }

    if prev < 10 {
      return Err("Too little in the previous period to compare");
    }

    let day = Days::new(1);
    let mut title = format!(
      "{} over {cur_days} days ({}) vs {} over {prev_days} days ({}) · last {CMP_LAG} days \
       excluded, still incomplete",
      num(cur),
      short_range(win.cmp_start, win.cmp_end - day),
      num(prev),
      short_range(win.prev_start, win.cmp_start - day),
    );

    if coverage < 0.9 {
      title.push_str(&format!(" · only {:.0}% of days collected", coverage * 100.0));
    }

    let avg = |x: i64, days: i64| x as f64 / days as f64;
    let pct = (avg(cur, cur_days) / avg(prev, prev_days) - 1.0) * 100.0;
    Ok(Self { pct, coverage, title })
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

// MARK: Charts data

/// Runs of 3+ missing days as (last day before, first day after)
fn outages(dat: &RepoDaily) -> Vec<(NaiveDate, NaiveDate)> {
  let Some(start) = dat.start.as_deref().and_then(|x| x.parse::<NaiveDate>().ok()) else {
    return vec![];
  };

  let day = |idx: usize| start + Days::new(idx as u64);
  let have: Vec<usize> =
    dat.days.iter().enumerate().filter(|x| x.1.is_some()).map(|x| x.0).collect();
  have.windows(2).filter(|x| x[1] - x[0] > 3).map(|x| (day(x[0]), day(x[1]))).collect()
}

fn star_gaps(
  tracking: Option<NaiveDate>,
  outages: &[(NaiveDate, NaiveDate)],
  stars: &[NaiveDate],
  today: NaiveDate,
) -> Vec<(NaiveDate, NaiveDate)> {
  let mut all = outages.to_vec();
  if let Some(start) = tracking {
    // before this, star history only has the days with new stars
    let from = start + Days::new(BACKFILL_DAYS);
    let runs = stars.windows(2).filter(|x| x[1] > from && (x[1] - x[0]).num_days() > 3);
    all.extend(runs.map(|x| (x[0], x[1])));
  }

  if let Some(last) = stars.last().filter(|x| (today - **x).num_days() > 3) {
    all.push((*last, today));
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

fn stars_gain(
  rp: &RepoPeriod,
  p: Period,
  today: NaiveDate,
  gaps: &[(NaiveDate, NaiveDate)],
  last: Option<NaiveDate>,
) -> Option<StarsGain> {
  let before = p.windows(today)?.start - Days::new(1);
  let inside = gaps.iter().any(|(a, b)| *a < before && before < *b);
  let stale = last.is_some_and(|x| (today - x).num_days() > 3);
  let gain = rp.stars_gain()?;
  let text = match gain {
    0 => format!("no change in {}", p.label()),
    x => format!("{} in {}", signed(x), p.label()),
  };

  Some(StarsGain { text, approx: inside || stale, zero: gain == 0 })
}

#[derive(Debug, Clone, PartialEq)]
struct StarsGain {
  text: String,
  approx: bool,
  zero: bool,
}

impl StarsGain {
  fn label(&self) -> String {
    format!("{}{}", if self.approx { "≈ " } else { "" }, self.text)
  }
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
  window_label: Option<String>,
}

fn stars_card(stars: i64, gain: Option<StarsGain>, gaps: bool) -> Markup {
  html!(
    section class="card" {
      div class="card-t" {
        h2 { "Stars" }
        @if gaps {
          div class="legend" { span { i class="sw hatch" {} "No data" } }
        }
      }
      div class="card-v" {
        span class="big" { (num(stars)) }
        @if let Some(x) = gain {
          span class=(if x.zero { "sub muted" } else { "sub" }) title=[x.approx.then_some(STARS_APPROX)] {
            (x.label())
          }
        }
      }
      div class="chart tall" { canvas id="chart_stars" role="img" aria-label="Stars over time" {} }
    }
  )
}

fn traffic_card(
  title: &str,
  canvas_id: &str,
  total: i64,
  uniques: i64,
  change: Option<Change>,
  p: Period,
) -> Markup {
  html!(
    section class="card" {
      div class="card-t" {
        h2 { (title) }
        div class="legend" {
          span { i class="sw total" {} "Total" }
          span { i class="sw unique" {} "Unique" }
          span class="gap-key" hidden { i class="sw hatch" {} "No data" }
        }
      }
      div class="card-v" {
        span class="big" { (num(total)) }
        span class="sub" { abbr title=(UNIQUE_TIP) { (num(uniques.min(total))) " unique" } }
        @if let Some(x) = change { (x.tile(p)) }
      }
      div class="chart" {
        canvas id=(canvas_id) role="img" aria-label=(format!("{title}: total and unique")) {}
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
          @for (idx, x) in rows.iter().enumerate() {
            tr hidden[idx >= ROWS_SHOWN] style=(format!("--w:{:.1}%", val(x) as f64 / max as f64 * 100.0)) {
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

      (show_all(rows.len()))
    }
  )
}

const ROWS_SHOWN: usize = 10;

fn show_all(rows: usize) -> Markup {
  html!(
    @if rows > ROWS_SHOWN {
      button type="button" class="show-all" data-show-all { "Show all (" (rows) ")" }
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

pub async fn repo_page(
  State(state): State<Arc<AppState>>,
  Path((owner, repo)): Path<(String, String)>,
  OriginalUri(uri): OriginalUri,
  headers: HeaderMap,
) -> HtmlRes {
  let repo = format!("{}/{}", owner, repo);
  let qs: Query<PopularFilter> = Query::try_from_uri(&uri)?;
  let db = &state.db;

  let totals = match db.get_repo_totals(&repo).await? {
    Some(x) => x,
    None => return AppError::not_found(),
  };

  if !state.filter.is_included(&totals.name, totals.fork, totals.archived) {
    return AppError::not_found();
  }

  match get_hx_target(&headers) {
    Some("refs_table") => return popular_table(db, &repo, &PopularKind::Refs, &qs).await,
    Some("path_table") => return popular_table(db, &repo, &PopularKind::Path, &qs).await,
    _ => {}
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
  let gaps = star_gaps(first, &outages(&daily), &star_days, today);

  let gain = stars_gain(&rp, p, today, &gaps, star_days.last().copied());
  let views = Change::new(rp.cmp_views, rp.cmp_days, rp.prev_views, rp.prev_days, p, today);
  let clones = Change::new(rp.cmp_clones, rp.cmp_days, rp.prev_clones, rp.prev_days, p, today);

  // Downloads will be one more card like these
  let mut charts = vec![];
  if has_views {
    charts.push(traffic_card("Views", "chart_views", rp.views, rp.views_uniques, views, p));
  }

  if has_clones {
    charts.push(traffic_card("Clones", "chart_clones", rp.clones, rp.clones_uniques, clones, p));
  }

  let window_label = gain.as_ref().filter(|_| !stars.is_empty()).map(|x| x.label());

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

  let html = html!(
    div class="stack" {
      div class="repo-head" {
        h1 {
          a href=(format!("https://github.com/{}", repo)) title="Open on GitHub" {
            (totals.name) span class="ext" aria-hidden="true" { "↗" }
          }
        }
        @if let Some(desc) = totals.description.as_deref().filter(|x| !x.is_empty()) {
          p { (desc) }
        }
      }

      div class="periodbar" {
        span class="range" { (period_range(p, today, first)) }
        (period_nav(p, false, period_url))
      }

      @if !stars.is_empty() {
        (stars_card(rp.stars, gain.clone(), !gaps.is_empty()))
      }

      @if !charts.is_empty() {
        div class=[(charts.len() > 1).then_some("grid2")] {
          @for x in &charts { (x) }
        }
      }

      (repo_popular_tables(db, &repo, &qs).await?)
    }

    script { "window.renderRepo(" (PreEscaped(serde_json::to_string(&data)?)) ");" }
  );

  Ok(base(&state, vec![(repo, None)], html))
}

// MARK: Index page

struct IndexRow<'a> {
  repo: &'a RepoTotals,
  per: RepoPeriod,
  today: NaiveDate,
}

type IndexCell = fn(&IndexRow, Period) -> Markup;

struct IndexCol {
  name: &'static str,
  sort: RepoSort,
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
    let per = &x.per;
    let change =
      Change::compare(per.cmp_views, per.cmp_days, per.prev_views, per.prev_days, p, x.today);
    two_lines(per.views, p, change.map(|x| x.short()))
  };

  let clones: IndexCell = |x, p| {
    let per = &x.per;
    let change =
      Change::compare(per.cmp_clones, per.cmp_days, per.prev_clones, per.prev_days, p, x.today);
    two_lines(per.clones, p, change.map(|x| x.short()))
  };

  let stars: IndexCell = |x, p| {
    let gain = match x.per.stars_gain() {
      None => Err("No star history before this period"),
      Some(0) => Err("No change in this period"),
      Some(x) => Ok(html!(small class="muted" { (signed(x)) })),
    };

    two_lines(x.per.stars, p, gain)
  };

  vec![
    IndexCol { name: "Name", sort: RepoSort::Name, per: false, class: "c-name", cell: name },
    IndexCol { name: "Views", sort: RepoSort::Views, per: true, class: "c-views", cell: views },
    IndexCol {
      name: "Clones",
      sort: RepoSort::Clones,
      per: true,
      class: "c-clones opt2",
      cell: clones,
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

fn two_lines(value: i64, p: Period, second: Result<Markup, &str>) -> Markup {
  html!(
    span class="v" { (num(value)) }
    @if p.days().is_some() {
      @match second {
        Ok(x) => (x),
        Err(why) => small class="muted" title=(why) { "—" },
      }
    }
  )
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
  let first = state.db.get_first_traffic().await?;
  let period_tip = format!("{}: {}", p.title(), period_range(p, today, first));
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
                @if col.per { " " (period_mark(&period_tip)) }
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

    // no scroll box around it: that would break the sticky headers
    (table_html)
  );

  Ok(base(&state, vec![], html))
}

#[cfg(test)]
mod tests {
  use std::time::{SystemTime, UNIX_EPOCH};

  use super::*;
  use crate::gh_client::{Repo, RepoReferrer, RepoViews, TrafficDaily};
  use crate::helpers::GhsFilter;

  fn db_path(name: &str) -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    std::env::temp_dir()
      .join(format!("ghstats-html-{name}-{}-{nanos}.db", std::process::id()))
      .to_string_lossy()
      .into_owned()
  }

  fn day(x: &str) -> NaiveDate {
    x.parse().unwrap()
  }

  fn ago(x: u64) -> NaiveDate {
    today() - Days::new(x)
  }

  fn test_repo(stars: u32) -> Repo {
    Repo {
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

  fn views(days: &[(NaiveDate, u32)]) -> RepoViews {
    let views = days
      .iter()
      .map(|(d, c)| TrafficDaily { timestamp: format!("{d}T00:00:00Z"), count: *c, uniques: c / 2 })
      .collect();

    RepoViews { count: 0, uniques: 0, views }
  }

  async fn get(state: &Arc<AppState>, uri: &str, target: Option<&str>) -> HtmlRes {
    let mut headers = HeaderMap::new();
    if let Some(x) = target {
      headers.insert("hx-target", x.parse().unwrap());
    }

    let uri = OriginalUri(uri.parse().unwrap());
    match uri.path() {
      "/" => index(State(state.clone()), uri, headers).await,
      _ => repo_page(State(state.clone()), Path(("o".into(), "r".into())), uri, headers).await,
    }
  }

  #[tokio::test]
  async fn base_starts_with_doctype() {
    let db = DbClient::new(&db_path("base")).await.unwrap();
    let state = Arc::new(AppState::for_test(db, GhsFilter::new("")));
    let page = base(&state, vec![], html!()).into_string();

    assert!(page.starts_with("<!DOCTYPE html><html><head><meta charset=\"utf-8\">"));
    assert_eq!(page.matches("<!DOCTYPE html>").count(), 1);
  }

  #[test]
  fn change_rules() {
    let (p, today) = (Period(30), day("2026-11-02"));
    let text = |x: Markup| {
      let x = x.into_string();
      x[x.find('>').unwrap() + 1..x.rfind('<').unwrap()].replace(" vs prev 30d", "")
    };

    // (cur, cur_days, prev, prev_days) -> (tile, index, approx)
    let cases = [
      ((1040, 30, 1000, 30), Some(("▲ 4.0%", "+4%", false))),
      ((1120, 30, 1000, 30), Some(("▲ 12%", "+12%", false))),
      ((916, 30, 1000, 30), Some(("▼ 8.4%", "−8%", false))),
      ((10996, 30, 10000, 30), Some(("▲ 10%", "+10%", false))),
      ((1004, 30, 1000, 30), Some(("▲ 0.4%", "±0%", false))),
      ((100, 30, 100, 30), Some(("±0%", "±0%", false))),
      ((150, 15, 300, 30), Some(("±0%", "±0%", true))),
      ((110, 14, 100, 30), None),
      ((97, 20, 100, 20), Some(("▼ 3.0%", "−3%", true))),
      ((160, 20, 200, 30), Some(("▲ 20%", "+20%", true))),
      ((110, 30, 9, 30), None),
    ];
    for ((cur, cur_days, prev, prev_days), want) in cases {
      let got = Change::new(cur, cur_days, prev, prev_days, p, today)
        .map(|x| (text(x.tile(p)), text(x.short()), x.approx()));
      let want = want.map(|(a, b, c)| (a.to_string(), b.to_string(), c));
      assert_eq!(got, want, "{cur} over {cur_days} vs {prev} over {prev_days}");
    }

    assert!(Change::new(110, 30, 100, 30, Period(-1), today).is_none());
    let title = Change::new(110, 30, 100, 30, p, today).unwrap().title;
    assert_eq!(
      title,
      "110 over 30 days (Oct 2–31) vs 100 over 30 days (Sep 2–Oct 1) · last 2 days excluded, still \
       incomplete"
    );
    assert!(
      Change::new(97, 20, 100, 20, p, today).unwrap().title.ends_with("only 67% of days collected")
    );
  }

  #[test]
  fn popular_rows_clamp_fill_and_collapse() {
    let kind = PopularKind::Path;
    let item = |name: &str, count, uniques| RepoPopularItem { name: name.into(), count, uniques };
    let rows = |n| {
      let mut items =
        vec![item("/o/r", 10, 14), item("/o/r/issues", 40, 2), item("/o/r/pulls", 0, 3)];
      items.extend((0..n).map(|x| item(&format!("/o/r/x{x}"), 1, 1)));
      items.into_iter().map(|x| PopularRow::new("o/r", &kind, x)).collect::<Vec<_>>()
    };

    let qs = PopularFilter { sort: PopularSort::Uniques, ..Default::default() };
    let html = popular_html("o/r", &kind, rows(10), &qs).into_string();
    assert!(html.contains("<td>10</td><td>10</td>"), "uniques clamped to views");
    assert!(!html.contains("/pulls"), "rows without views are dropped");
    assert!(html.contains("<tr style=\"--w:100.0%\"><td class=\"name\" title=\"/\">"));
    assert!(html.contains("<tr style=\"--w:20.0%\"><td class=\"name\" title=\"/issues\">"));
    assert_eq!(html.matches("<tr hidden").count(), 2);
    assert!(html.contains(">Show all (12)</button>"));

    let qs =
      PopularFilter { sort: PopularSort::Name, direction: Direction::Asc, ..Default::default() };
    let html = popular_html("o/r", &kind, rows(0), &qs).into_string();
    assert!(
      html.contains("<tr style=\"--w:25.0%\"><td class=\"name\" title=\"/\">"),
      "fill by views"
    );
    assert!(!html.contains("Show all"));
  }

  #[test]
  fn outages_and_star_gaps() {
    let some = Some([1, 1, 1, 1]);
    let dat = RepoDaily {
      start: Some("2026-01-01".into()),
      days: vec![some, None, None, some, None, None, None, some, None],
    };
    assert_eq!(outages(&dat), vec![(day("2026-01-04"), day("2026-01-08"))]);

    // stargazer history is sparse until daily rows start (Jan 14), later runs of 4+ days are gaps
    let sparse =
      ["2025-12-01", "2026-01-10", "2026-01-14", "2026-01-15", "2026-01-17", "2026-01-21"];
    let outage = [(day("2026-04-01"), day("2026-05-17"))];
    let resumed = ["2026-03-31", "2026-04-01", "2026-06-01", "2026-06-02"];
    let cases: [(Option<&str>, &[_], &[&str], &str, &[(&str, &str)]); 4] = [
      (Some("2026-01-01"), &[], &sparse, "2026-01-23", &[("2026-01-17", "2026-01-21")]),
      (None, &[], &sparse, "2026-01-23", &[]),
      (None, &[], &sparse, "2026-02-10", &[("2026-01-21", "2026-02-10")]),
      (Some("2026-01-01"), &outage, &resumed, "2026-06-02", &[("2026-04-01", "2026-06-01")]),
    ];
    for (start, outages, stars, today, want) in cases {
      let stars: Vec<_> = stars.iter().map(|x| day(x)).collect();
      let want: Vec<_> = want.iter().map(|(a, b)| (day(a), day(b))).collect();
      assert_eq!(star_gaps(start.map(day), outages, &stars, day(today)), want);
    }
  }

  #[test]
  fn stars_gain_label() {
    let (today, last) = (day("2026-11-02"), Some(day("2026-11-02")));
    let rp = RepoPeriod { stars: 15, stars_start: 20, ..Default::default() };
    let gaps = [(day("2026-10-20"), day("2026-10-30"))];
    let gain = |rp: &RepoPeriod, p, gaps: &[_], last| {
      stars_gain(rp, p, today, gaps, last).map(|x| (x.label(), x.zero))
    };

    let label = |x: &str, zero| Some((x.to_string(), zero));
    assert_eq!(gain(&rp, Period(7), &[], last), label("−5 in 7d", false));
    assert_eq!(gain(&rp, Period(7), &gaps, last), label("≈ −5 in 7d", false), "start in a gap");
    assert_eq!(gain(&rp, Period(30), &[], Some(day("2026-10-28"))), label("≈ −5 in 30d", false));
    assert_eq!(gain(&rp, Period(-1), &[], last), None);

    let flat = RepoPeriod { stars: 20, stars_start: 20, ..Default::default() };
    assert_eq!(gain(&flat, Period(30), &[], last), label("no change in 30d", true));

    let unknown = RepoPeriod { stars: 20, ..Default::default() };
    assert_eq!(gain(&unknown, Period(30), &[], last), None, "no history before the period");
  }

  #[tokio::test]
  async fn backfill_counts_as_collected_until_the_last_row() {
    // the first sync's backfill only has days with traffic, then daily rows
    let db = DbClient::new(&db_path("backfill")).await.unwrap();
    db.insert_repo(&test_repo(5)).await.unwrap();
    db.insert_views(&test_repo(5), &views(&[(ago(40), 10), (ago(35), 10), (ago(30), 10)]))
      .await
      .unwrap();
    for x in 0..=25 {
      db.insert_stats(&test_repo(5), &format!("{}T00:00:00Z", ago(x)), &[]).await.unwrap();
    }

    let dat = db.get_daily("o/r", today()).await.unwrap();
    assert!(dat.days.iter().all(|x| x.is_some()));
    assert!(outages(&dat).is_empty());
    let rp = db.get_periods(Some("o/r"), Period(30), today()).await.unwrap().remove(0);
    assert_eq!((rp.cmp_days, rp.prev_days), (30, 9));

    // nothing collected after the first sync: no invented zeros, nothing to compare
    let db = DbClient::new(&db_path("dead")).await.unwrap();
    db.insert_repo(&test_repo(5)).await.unwrap();
    db.insert_views(&test_repo(5), &views(&[(ago(20), 10), (ago(15), 10), (ago(10), 10)]))
      .await
      .unwrap();

    let dat = db.get_daily("o/r", today()).await.unwrap();
    assert!(
      dat.days[..11].iter().all(|x| x.is_some()) && dat.days[11..].iter().all(|x| x.is_none())
    );
    let rp = db.get_periods(Some("o/r"), Period(7), today()).await.unwrap().remove(0);
    assert_eq!((rp.cmp_days, rp.prev_days), (0, 6));
  }

  #[tokio::test]
  async fn pages_render_only_blocks_with_data() {
    // stars dropped from 20 to 0, a referrer, no traffic, no paths
    let db = DbClient::new(&db_path("blocks")).await.unwrap();
    db.insert_repo(&test_repo(20)).await.unwrap();
    db.insert_stats(&test_repo(20), &format!("{}T00:00:00Z", ago(10)), &[]).await.unwrap();
    db.insert_stats(&test_repo(0), &format!("{}T00:00:00Z", ago(0)), &[]).await.unwrap();
    let refs = [RepoReferrer { referrer: "google.com".into(), count: 3, uniques: 2 }];
    db.insert_referrers(&test_repo(0), &format!("{}T00:00:00Z", ago(0)), &refs).await.unwrap();
    db.update_deltas().await.unwrap();
    let state = Arc::new(AppState::for_test(db, GhsFilter::new("")));

    let page = get(&state, "/o/r?period=7d", None).await.unwrap().into_string();
    assert!(page.contains("<h2>Stars</h2>"));
    assert!(page.contains("<span class=\"big\">0</span>"), "current stars from the latest row");
    assert!(page.contains("−20 in 7d"));
    assert!(!page.contains("<h2>Views</h2>") && !page.contains("<h2>Clones</h2>"));
    assert!(page.contains("id=\"refs_table\"") && !page.contains("id=\"path_table\""));

    let frag = get(&state, "/o/r?sort=name", Some("refs_table")).await.unwrap().into_string();
    assert!(frag.starts_with("<div id=\"refs_table\""));

    let frag = get(&state, "/?period=7d", Some("repos_table")).await.unwrap().into_string();
    assert!(frag.starts_with("<table id=\"repos_table\""));
    assert!(
      frag.contains("id=\"filter_period\" name=\"period\" value=\"7d\" hx-swap-oob=\"true\"")
    );
    assert!(frag.contains("<span class=\"v\">0</span><small class=\"muted\">−20</small>"));
    let dash = "title=\"Not enough days collected to compare with the previous period\">—</small>";
    assert_eq!(frag.matches(dash).count(), 2, "views and clones");

    let frag = get(&state, "/?period=all", Some("repos_table")).await.unwrap().into_string();
    assert!(!frag.contains("<small"), "no second line at all time");
  }

  #[tokio::test]
  async fn excluded_repo_fragments_are_not_found() {
    let db = DbClient::new(&db_path("excluded")).await.unwrap();
    db.insert_repo(&test_repo(5)).await.unwrap();
    db.insert_stats(&test_repo(5), &format!("{}T00:00:00Z", today()), &[]).await.unwrap();
    let state = Arc::new(AppState::for_test(db, GhsFilter::new("other/*")));

    for target in [None, Some("refs_table"), Some("path_table")] {
      let res = get(&state, "/o/r", target).await.unwrap_err();
      let res = axum::response::IntoResponse::into_response(res);
      assert_eq!(res.status(), axum::http::StatusCode::NOT_FOUND, "{target:?}");
    }
  }
}
