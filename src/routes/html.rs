use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::HeaderMap;
use chrono::{DateTime, Datelike, Days, NaiveDate, Utc};
use maud::{DOCTYPE, Markup, PreEscaped, html};
use serde::Serialize;
use thousands::Separable;
use url::form_urlencoded::Serializer;

use crate::AppState;
use crate::db_client::{
  BACKFILL_DAYS, CMP_LAG, DbClient, Direction, Period, PopularFilter, PopularKind, PopularSort,
  ReleaseDownloads, RepoDaily, RepoFilter, RepoMeta, RepoPeriod, RepoPopularItem, RepoSort,
  RepoTotals,
};
use crate::routes::assets::vendor_url;
use crate::state::SyncStatus;
use crate::types::{AppError, HtmlRes, Res};

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

/// JSON safe inside <script>: strings like "</script>" can't close the tag
fn script_json(x: &impl Serialize) -> Res<String> {
  let json = serde_json::to_string(x)?;
  let json = json.replace('<', "\\u003c").replace('>', "\\u003e").replace('&', "\\u0026");
  Ok(json.replace('\u{2028}', "\\u2028").replace('\u{2029}', "\\u2029"))
}

fn ago_time(x: DateTime<Utc>, now: DateTime<Utc>) -> String {
  match (now - x).num_minutes() {
    x if x < 1 => "just now".to_string(),
    x if x < 60 => format!("{x} min ago"),
    x if x < 60 * 24 => format!("{} h ago", x / 60),
    x => format!("{} d ago", x / 60 / 24),
  }
}

fn sync_label(status: &SyncStatus, now: DateTime<Utc>) -> Markup {
  let time = |x: DateTime<Utc>| x.format("%Y-%m-%d %H:%M UTC").to_string();
  let (class, text, title) = match (status.running, &status.last_error, status.last_success) {
    (true, _, _) => ("sync busy", "Syncing…".to_string(), "Syncing with GitHub".to_string()),
    (_, Some(err), _) => {
      let at = status.last_error_at.map(time).unwrap_or_default();
      ("sync bad", "Sync failed".to_string(), format!("{at}: {err}"))
    }
    (_, _, Some(x)) => ("sync ok", format!("Synced {}", ago_time(x, now)), time(x)),
    _ => ("sync", "Not synced yet".to_string(), "No sync since start".to_string()),
  };

  html!(span class=(class) title=(title) { i class="dot" {} span class="text" { (text) } })
}

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

              (sync_label(&state.sync_status(), Utc::now()))

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
  downloads: Option<&'a DownloadsSeries>,
}

fn day_title(x: NaiveDate) -> String {
  x.format("%b %-d, %Y").to_string()
}

fn ago(x: NaiveDate, today: NaiveDate) -> String {
  let (n, unit) = match (today - x).num_days() {
    x if x < 1 => return "today".to_string(),
    x if x < 30 => (x, "day"),
    x if x < 365 => ((x as f64 / 30.4).round() as i64, "month"),
    x => (x / 365, "year"),
  };

  format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

// https://github.com/github-linguist/linguist/blob/main/lib/linguist/languages.yml
fn lang_color(lang: &str) -> &'static str {
  match lang {
    "Rust" => "#dea584",
    "Python" => "#3572a5",
    "JavaScript" => "#f1e05a",
    "TypeScript" => "#3178c6",
    "Go" => "#00add8",
    "Java" => "#b07219",
    "Kotlin" => "#a97bff",
    "Swift" => "#f05138",
    "C" => "#555555",
    "C++" => "#f34b7d",
    "C#" => "#178600",
    "Ruby" => "#701516",
    "PHP" => "#4f5d95",
    "Shell" => "#89e051",
    "HTML" => "#e34c26",
    "CSS" => "#563d7c",
    "Vue" => "#41b883",
    "Svelte" => "#ff3e00",
    "Dart" => "#00b4ab",
    "Elixir" => "#6e4a7e",
    "Haskell" => "#5e5086",
    "Lua" => "#000080",
    "Zig" => "#ec915c",
    "Nix" => "#7e7eff",
    "Dockerfile" => "#384d54",
    "Jupyter Notebook" => "#da5b0b",
    "Objective-C" => "#438eff",
    _ => "#9aa4b2",
  }
}

const TOPICS_SHOWN: usize = 6;

fn info_card(totals: &RepoTotals, meta: &RepoMeta, today: NaiveDate) -> Markup {
  let date = |x: &Option<String>| x.as_deref().and_then(|x| x.get(..10)?.parse::<NaiveDate>().ok());
  let (created, pushed) = (date(&meta.created_at), date(&meta.pushed_at));
  let homepage =
    meta.homepage.as_deref().filter(|x| x.starts_with("https://") || x.starts_with("http://"));
  let more = meta.topics.get(TOPICS_SHOWN..).unwrap_or_default();

  html!(
    section class="card info" {
      div {
        h1 {
          a href=(format!("https://github.com/{}", totals.name)) title="Open on GitHub" {
            (totals.name) span class="ext" aria-hidden="true" { "↗" }
          }
        }
        @if let Some(desc) = totals.description.as_deref().filter(|x| !x.is_empty()) {
          p class="desc" title=(desc) { (desc) }
        }
      }

      @if meta.language.is_some() || meta.license.is_some() || created.is_some() || pushed.is_some() {
        div class="facts" {
          @if let Some(x) = &meta.language {
            span class="lang" { i class="dot" style=(format!("background:{}", lang_color(x))) {} (x) }
          }
          @if let Some(x) = &meta.license { span { (x) } }
          @if let Some(x) = created { span title=(day_title(x)) { "created " (ago(x, today)) } }
          @if let Some(x) = pushed { span title=(day_title(x)) { "last push " (ago(x, today)) } }
        }
      }

      @if !meta.topics.is_empty() {
        div class="topics" {
          @for x in meta.topics.iter().take(TOPICS_SHOWN) { span class="topic" { (x) } }
          @if !more.is_empty() {
            span class="topic more" title=(more.join(", ")) { "+" (more.len()) }
          }
        }
      }

      @if let Some(x) = homepage {
        div class="facts" {
          a href=(x) { (x.trim_start_matches("https://").trim_start_matches("http://")) }
        }
      }

      div class="repo-stats" {
        @for (n, one, many) in [
          (totals.forks, "fork", "forks"),
          (totals.issues, "open issue", "open issues"),
          (totals.prs, "open PR", "open PRs"),
        ] {
          span { b { (num(n as i64)) } " " (if n == 1 { one } else { many }) }
        }
      }
    }
  )
}

fn stars_chart_card(gaps: bool) -> Markup {
  html!(
    section class="card stars-card" {
      div class="card-t" {
        h2 { "Stars" }
        @if gaps {
          div class="legend" { span { i class="sw hatch" {} "No data" } }
        }
      }
      div class="chart" { canvas id="chart_stars" role="img" aria-label="Stars over time" {} }
    }
  )
}

fn tile(title: &str, value: i64, sub: Markup, change: Option<Markup>) -> Markup {
  html!(
    div class="card kpi" {
      h2 { (title) }
      div class="value" { (num(value)) }
      div class="sub" { (sub) }
      div class="change" { @if let Some(x) = change { (x) } }
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

fn stars_tile(stars: i64, gain: Option<StarsGain>) -> Markup {
  let sub = html!(@if let Some(x) = gain {
    span class=[x.zero.then_some("muted")] title=[x.approx.then_some(STARS_APPROX)] { (x.label()) }
  });
  tile("Stars", stars, sub, None)
}

fn traffic_card(title: &str, canvas_id: &str) -> Markup {
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
      div class="chart" {
        canvas id=(canvas_id) role="img" aria-label=(format!("{title}: total and unique")) {}
      }
    }
  )
}

// MARK: Downloads

/// Days with downloads by release: the top ones of the period, then the rest together
#[derive(Debug, Serialize)]
struct DownloadsSeries {
  start: String,
  days: Vec<Option<Vec<i64>>>,
  labels: Vec<String>,
}

struct Downloads {
  total: i64,
  /// in the period, when any of its days were collected
  period: Option<i64>,
  change: Option<Change>,
  releases: Vec<ReleaseDownloads>,
  series: Option<DownloadsSeries>,
}

const RELEASES_STACKED: usize = 3;

fn downloads(
  mut releases: Vec<ReleaseDownloads>,
  daily: &[(String, i64, i64)],
  p: Period,
  today: NaiveDate,
) -> Option<Downloads> {
  if releases.is_empty() {
    return None;
  }

  let total = releases.iter().map(|x| x.total).sum();
  let key =
    |x: &ReleaseDownloads| if p.days().is_some() { (x.period, x.total) } else { (x.total, 0) };
  releases.sort_by_key(|x| std::cmp::Reverse(key(x)));

  let mut per_day: Vec<(NaiveDate, i64, i64)> =
    daily.iter().filter_map(|(d, id, n)| Some((d.parse().ok()?, *id, *n))).collect();
  per_day.sort();

  let sum = |from: NaiveDate, to: NaiveDate| {
    let days: Vec<_> = per_day.iter().filter(|x| x.0 >= from && x.0 < to).collect();
    let mut dates: Vec<_> = days.iter().map(|x| x.0).collect();
    dates.dedup();
    (days.iter().map(|x| x.2).sum::<i64>(), dates.len() as i64)
  };

  let (period, change) = match p.windows(today) {
    Some(w) => {
      let (cur, cur_days) = sum(w.start, today + Days::new(1));
      let (cmp, cmp_days) = sum(w.cmp_start, w.cmp_end);
      let (prev, prev_days) = sum(w.prev_start, w.cmp_start);
      ((cur_days > 0).then_some(cur), Change::new(cmp, cmp_days, prev, prev_days, p, today))
    }
    None => (None, None),
  };

  let top: Vec<i64> =
    releases.iter().filter(|x| key(x).0 > 0).take(RELEASES_STACKED).map(|x| x.release_id).collect();
  let series = per_day.first().map(|first| {
    let start = first.0;
    let len = (today - start).num_days().max(0) as usize + 1;
    let mut days: Vec<Option<Vec<i64>>> = vec![None; len];
    for (d, id, n) in &per_day {
      let Some(x) = days.get_mut((*d - start).num_days() as usize) else { continue };
      let row = x.get_or_insert_with(|| vec![0; top.len() + 1]);
      let idx = top.iter().position(|x| x == id).unwrap_or(top.len());
      row[idx] += n;
    }

    let rest = releases.len() - top.len();
    let mut labels: Vec<String> = top
      .iter()
      .filter_map(|id| releases.iter().find(|x| x.release_id == *id).map(|x| x.tag.clone()))
      .collect();
    labels.push(match rest {
      1 => "1 other release".to_string(),
      x => format!("{x} other releases"),
    });

    DownloadsSeries { start: start.to_string(), days, labels }
  });

  Some(Downloads { total, period, change, releases, series })
}

fn downloads_tile(dl: &Downloads, p: Period) -> Markup {
  match dl.period {
    Some(x) => {
      let sub = html!((num(dl.total)) " all time");
      tile("Downloads", x, sub, dl.change.as_ref().map(|x| x.tile(p)))
    }
    None => tile("Downloads", dl.total, html!(span class="muted" { "all time" }), None),
  }
}

fn downloads_card(series: &DownloadsSeries) -> Markup {
  html!(
    section class="card" {
      div class="card-t" {
        h2 { "Downloads" }
        div class="legend" {
          @for (idx, x) in series.labels.iter().enumerate() {
            span { i class=(format!("sw s{}", idx + 1)) {} (x) }
          }
          span class="gap-key" hidden { i class="sw hatch" {} "No data" }
        }
      }
      div class="chart" { canvas id="chart_downloads" role="img" aria-label="Downloads by release" {} }
    }
  )
}

fn releases_table(dl: &Downloads, p: Period, today: NaiveDate, period_tip: &str) -> Markup {
  let has_period = p.days().is_some();
  let val = |x: &ReleaseDownloads| if has_period { x.period } else { x.total };
  let max = dl.releases.iter().map(val).max().unwrap_or(1).max(1);
  let published = |x: &ReleaseDownloads| {
    let day = x.published_at.as_deref()?.get(..10)?.parse::<NaiveDate>().ok()?;
    let fmt = if day.year() == today.year() { "%b %-d" } else { "%b %-d, %Y" };
    Some(day.format(fmt).to_string())
  };

  html!(
    div id="releases_table" class="card flush" {
      table class="data" {
        colgroup {
          col; col class="n2 opt2";
          @if has_period { col class="n2"; }
          col class="n2";
        }
        thead {
          tr {
            th scope="col" { "Releases" }
            th scope="col" class="opt2" { "Published" }
            @if has_period { th scope="col" { "Downloads " (period_mark(period_tip)) } }
            th scope="col" { "All time" }
          }
        }
        tbody {
          @for (idx, x) in dl.releases.iter().enumerate() {
            tr hidden[idx >= ROWS_SHOWN] style=(format!("--w:{:.1}%", val(x) as f64 / max as f64 * 100.0)) {
              td class="name mono" title=(x.tag) { (x.tag) }
              td class="opt2" { (published(x).unwrap_or_default()) }
              @if has_period { td { (num(x.period)) } }
              td { (num(x.total)) }
            }
          }
        }
      }

      (show_all(dl.releases.len()))
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
  // a synced star history has every day, so only traffic-less days after it are unknown
  let gaps = match rp.stars_synced {
    true => star_gaps(None, &[], &star_days, today),
    false => star_gaps(first, &outages(&daily), &star_days, today),
  };

  let gain = stars_gain(&rp, p, today, &gaps, star_days.last().copied());
  let views = Change::new(rp.cmp_views, rp.cmp_days, rp.prev_views, rp.prev_days, p, today);
  let clones = Change::new(rp.cmp_clones, rp.cmp_days, rp.prev_clones, rp.prev_days, p, today);

  let since = p.days().map(|x| x as i32 - 1).unwrap_or(0);
  let releases = db.get_release_downloads(&repo, since).await?;
  let dl_daily = db.get_release_downloads_daily(&repo).await?;
  let dl = downloads(releases, &dl_daily, p, today);
  let meta = db.get_repo_meta(&repo).await?;

  let mut tiles = vec![];
  if has_views {
    tiles.push(traffic_tile("Views", rp.views, rp.views_uniques, views, p));
  }

  if has_clones {
    tiles.push(traffic_tile("Clones", rp.clones, rp.clones_uniques, clones, p));
  }

  if !stars.is_empty() || rp.stars > 0 {
    tiles.push(stars_tile(rp.stars, gain.clone()));
  }

  if let Some(x) = &dl {
    tiles.push(downloads_tile(x, p));
  }

  let charts: Vec<Markup> =
    [(has_views, "Views", "chart_views"), (has_clones, "Clones", "chart_clones")]
      .into_iter()
      .filter(|x| x.0)
      .map(|x| traffic_card(x.1, x.2))
      .collect();

  let window_label = gain.as_ref().filter(|_| !stars.is_empty()).map(|x| x.label());
  let series = dl.as_ref().and_then(|x| x.series.as_ref());
  let data = ChartData {
    today: today.to_string(),
    period: p.0,
    daily: &daily,
    stars: &stars,
    gaps: &gaps,
    window_label,
    downloads: series,
  };

  let period_url =
    |x: Period| format!("/{}?sort={}&direction={}&period={}", repo, qs.sort, qs.direction, x);

  let html = html!(
    div class="stack" {
      div class="periodbar" {
        span class="range" { (period_range(p, today, first)) }
        (period_nav(p, false, period_url))
      }

      div class=[(!stars.is_empty()).then_some("row1")] {
        (info_card(&totals, &meta, today))
        @if !stars.is_empty() { (stars_chart_card(!gaps.is_empty())) }
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

      @if let Some(x) = series { (downloads_card(x)) }
      @if let Some(x) = &dl {
        (releases_table(x, p, today, &format!("{}: {}", p.title(), period_range(p, today, first))))
      }

      (repo_popular_tables(db, &repo, &qs).await?)
    }

    script { "window.renderRepo(" (PreEscaped(script_json(&data)?)) ");" }
  );

  Ok(base(&state, vec![(repo, None)], html))
}

// MARK: Index page

struct IndexRow<'a> {
  repo: &'a RepoTotals,
  per: RepoPeriod,
  today: NaiveDate,
  dl: Option<IndexDownloads>,
}

struct IndexDownloads {
  value: i64,
  change: Result<Change, &'static str>,
}

type IndexCell = fn(&IndexRow, Period) -> Markup;

struct IndexCol {
  name: &'static str,
  sort: Option<RepoSort>,
  per: bool,
  class: &'static str,
  cell: IndexCell,
}

fn index_cols(downloads: bool) -> Vec<IndexCol> {
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

  let dl: IndexCell = |x, p| match &x.dl {
    Some(dl) => two_lines(dl.value, p, dl.change.as_ref().map(|x| x.short()).map_err(|x| *x)),
    None => html!(
      span class="v muted" { "—" }
      @if p.days().is_some() { small class="muted" title="No release assets" { "—" } }
    ),
  };

  let stars: IndexCell = |x, p| {
    let gain = match x.per.stars_gain() {
      None => Err("No star history before this period"),
      Some(0) => Err("No change in this period"),
      Some(x) => Ok(html!(small class="muted" { (signed(x)) })),
    };

    two_lines(x.per.stars, p, gain)
  };

  let cols = vec![
    IndexCol { name: "Name", sort: Some(RepoSort::Name), per: false, class: "c-name", cell: name },
    IndexCol {
      name: "Views",
      sort: Some(RepoSort::Views),
      per: true,
      class: "c-views",
      cell: views,
    },
    IndexCol {
      name: "Clones",
      sort: Some(RepoSort::Clones),
      per: true,
      class: "c-clones opt2",
      cell: clones,
    },
    IndexCol { name: "Downloads", sort: None, per: true, class: "c-dl opt2", cell: dl },
    IndexCol {
      name: "Stars",
      sort: Some(RepoSort::Stars),
      per: false,
      class: "c-stars",
      cell: stars,
    },
    IndexCol {
      name: "Forks",
      sort: Some(RepoSort::Forks),
      per: false,
      class: "c-forks opt",
      cell: |x, _| html!((num(x.repo.forks as i64))),
    },
    IndexCol {
      name: "Issues",
      sort: Some(RepoSort::Issues),
      per: false,
      class: "c-issues opt",
      cell: |x, _| html!((num(x.repo.issues as i64))),
    },
    IndexCol {
      name: "PRs",
      sort: Some(RepoSort::Prs),
      per: false,
      class: "c-prs opt",
      cell: |x, _| html!((num(x.repo.prs as i64))),
    },
  ];

  cols.into_iter().filter(|x| downloads || !x.class.starts_with("c-dl")).collect()
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

/// Release downloads of the visible repos that have release assets
async fn index_downloads(
  db: &DbClient,
  repos: &[RepoTotals],
  p: Period,
  today: NaiveDate,
) -> Res<HashMap<i64, IndexDownloads>> {
  let ids: Vec<i64> = repos.iter().map(|x| x.id).collect();
  let totals: Vec<(i64, i64)> = db
    .get_repos_downloads(0)
    .await?
    .into_iter()
    .filter(|x| ids.contains(&x.id))
    .map(|x| (x.id, x.total))
    .collect();

  let Some(w) = p.windows(today) else {
    let none = || Err("No previous period for all time");
    return Ok(
      totals.into_iter().map(|(id, x)| (id, IndexDownloads { value: x, change: none() })).collect(),
    );
  };

  let daily = db.get_repos_downloads_daily(&w.prev_start.to_string()).await?;
  let out = totals
    .into_iter()
    .map(|(id, _)| {
      let days: Vec<(NaiveDate, i64)> =
        daily.iter().filter(|x| x.0 == id).filter_map(|x| Some((x.1.parse().ok()?, x.2))).collect();
      let sum = |from: NaiveDate, to: NaiveDate| {
        let xs: Vec<_> = days.iter().filter(|x| x.0 >= from && x.0 < to).collect();
        (xs.iter().map(|x| x.1).sum::<i64>(), xs.len() as i64)
      };

      let (cmp, cmp_days) = sum(w.cmp_start, w.cmp_end);
      let (prev, prev_days) = sum(w.prev_start, w.cmp_start);
      let value = sum(w.start, today + Days::new(1)).0;
      let change = Change::compare(cmp, cmp_days, prev, prev_days, p, today);
      (id, IndexDownloads { value, change })
    })
    .collect();

  Ok(out)
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
  let mut dls = index_downloads(&state.db, &repos, p, today).await?;
  let mut rows: Vec<IndexRow> = repos
    .iter()
    .map(|x| {
      let per = pers.remove(&x.name).unwrap_or_default();
      IndexRow { repo: x, per, today, dl: dls.remove(&x.id) }
    })
    .collect();
  sort_index(&mut rows, &qs.sort, &qs.direction);

  let cols = index_cols(rows.iter().any(|x| x.dl.is_some()));
  let first = state.db.get_first_traffic().await?;
  let period_tip = format!("{}: {}", p.title(), period_range(p, today, first));
  let cur_q = qs.q.clone().unwrap_or_default();
  let cur_owner = qs.owner.clone().unwrap_or_default();

  let sort_url = |col: &RepoSort| {
    let dir = next_dir(qs.sort == *col, &qs.direction, *col == RepoSort::Name);
    index_url(&qs, col, dir, p)
  };

  let head = |col: &IndexCol| {
    let name = html!((col.name) @if col.per { " " (period_mark(&period_tip)) });
    match &col.sort {
      Some(sort) => html!(
        button type="button"
          hx-get=(sort_url(sort))
          hx-target="#repos_table"
          hx-swap="outerHTML"
          hx-replace-url="true"
        { (name) }
      ),
      None => name,
    }
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
            th scope="col" class=(col.class) aria-sort=[aria_sort(col.sort.as_ref() == Some(&qs.sort), &qs.direction)] {
              (head(col))
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
      ..Default::default()
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
  fn sync_label_states() {
    let now = Utc::now();
    let ok =
      SyncStatus { last_success: Some(now - chrono::Duration::minutes(5)), ..Default::default() };
    let failed =
      SyncStatus { last_error: Some("401".into()), last_error_at: Some(now), ..ok.clone() };
    let cases = [
      (SyncStatus::default(), "sync", "Not synced yet"),
      (SyncStatus { running: true, ..failed.clone() }, "sync busy", "Syncing…"),
      (ok, "sync ok", "Synced 5 min ago"),
      (failed, "sync bad", "Sync failed"),
    ];
    for (status, class, text) in cases {
      let html = sync_label(&status, now).into_string();
      assert!(html.starts_with(&format!("<span class=\"{class}\"")), "{html}");
      assert!(html.contains(&format!("<span class=\"text\">{text}</span>")), "{html}");
    }
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
    assert!(page.contains("<h2>Stars</h2><div class=\"value\">0</div>"), "stars of the latest row");
    assert!(page.contains("−20 in 7d"));
    assert!(!page.contains("<h2>Views</h2>") && !page.contains("<h2>Clones</h2>"));
    assert!(!page.contains("<h2>Downloads</h2>") && !page.contains("id=\"releases_table\""));
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

  #[tokio::test]
  async fn traffic_flag_keeps_star_rows_out_of_traffic() {
    // traffic until 20 days ago, then an outage the star history fills with traffic-less rows
    let db = DbClient::new(&db_path("flag")).await.unwrap();
    db.insert_repo(&test_repo(5)).await.unwrap();
    for x in (0..60).filter(|x| !(10..=20).contains(x)) {
      db.insert_stats(&test_repo(5), &format!("{}T00:00:00Z", ago(x)), &[]).await.unwrap();
    }

    db.insert_views(&test_repo(5), &views(&[(ago(59), 10)])).await.unwrap();
    let history: Vec<_> =
      [30].into_iter().chain(10..=20).map(|x| (format!("{}T00:00:00Z", ago(x)), 5, 0)).collect();
    db.insert_stars(1, &history).await.unwrap();

    let dat = db.get_daily("o/r", today()).await.unwrap();
    assert_eq!(outages(&dat), vec![(ago(21), ago(9))], "star rows aren't traffic");
    assert!(dat.days[59 - 30].is_some(), "a star row doesn't unmark a traffic day");

    let rp = db.get_periods(Some("o/r"), Period(30), today()).await.unwrap().remove(0);
    assert_eq!(rp.cmp_days, 30 - 11);
  }

  #[tokio::test]
  async fn downloads_render_only_with_release_assets() {
    use crate::gh_client::{Release, ReleaseAsset};

    let path = db_path("downloads");
    let db = DbClient::new(&path).await.unwrap();
    let other = |stars| Repo { id: 2, full_name: "o/b".into(), ..test_repo(stars) };
    for repo in [test_repo(5), other(5)] {
      db.insert_repo(&repo).await.unwrap();
      db.insert_stats(&repo, &format!("{}T00:00:00Z", ago(0)), &[]).await.unwrap();
    }

    let release = |count| Release {
      id: 7,
      tag_name: "</script><b>x</b>".into(),
      draft: false,
      published_at: Some(format!("{}T10:00:00Z", ago(40))),
      assets: vec![ReleaseAsset {
        id: 70,
        name: "app.tar.gz".into(),
        download_count: count,
        created_at: None,
      }],
    };
    for (x, count) in [(3, 100), (2, 120), (1, 150)] {
      db.insert_release_assets(&test_repo(5), &format!("{}T00:00:00Z", ago(x)), &[release(count)])
        .await
        .unwrap();
    }

    let state = Arc::new(AppState::for_test(db, GhsFilter::new("")));
    let page = get(&state, "/o/r?period=7d", None).await.unwrap().into_string();
    assert!(page.contains("<h2>Downloads</h2><div class=\"value\">50</div>"));
    assert!(page.contains("id=\"chart_downloads\"") && page.contains("id=\"releases_table\""));
    assert!(
      !page.contains("</script><b>") && page.contains("\\u003c/script\\u003e"),
      "tag stays a string"
    );

    let frag = get(&state, "/?period=7d", Some("repos_table")).await.unwrap().into_string();
    assert!(frag.contains("<span class=\"v\">50</span>"));
    assert!(frag.contains("title=\"No release assets\""), "o/b has none");

    // the column follows the visible repos only
    let db = DbClient::new(&path).await.unwrap();
    let state = Arc::new(AppState::for_test(db, GhsFilter::new("o/b")));
    let frag = get(&state, "/?period=7d", Some("repos_table")).await.unwrap().into_string();
    assert!(!frag.contains("Downloads"));
  }
}
