use std::sync::Arc;

use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::HeaderMap;
use maud::{Markup, PreEscaped, html};
use thousands::Separable;
use url::form_urlencoded::Serializer;

use crate::AppState;
use crate::db_client::{
  DbClient, Direction, PopularFilter, PopularKind, PopularSort, RepoFilter, RepoSort, RepoTotals,
};
use crate::helpers::truncate_middle;
use crate::types::{AppError, HtmlRes};

#[derive(Debug)]
struct TablePopularItem {
  item: (String, Option<String>), // title, url
  uniques: i64,
  count: i64,
}

type PopularColumn =
  (&'static str, Box<dyn Fn(&TablePopularItem) -> Markup + Send + Sync>, PopularSort);
type RepoColumn = (&'static str, Box<dyn Fn(&RepoTotals) -> Markup + Send + Sync>, RepoSort);

fn get_hx_target(headers: &HeaderMap) -> Option<&str> {
  headers.get("hx-target").and_then(|value| value.to_str().ok())
}

fn maybe_url(item: &(String, Option<String>)) -> Markup {
  let (name, url) = item;

  match url {
    Some(url) => html!(a href=(url) { (truncate_middle(name, 40)) }),
    None => html!(span { (name) }),
  }
}

fn compact_number(value: i32) -> String {
  let abs = value.abs() as f64;
  let (scaled, suffix) = if abs >= 1_000_000_000.0 {
    (value as f64 / 1_000_000_000.0, "b")
  } else if abs >= 1_000_000.0 {
    (value as f64 / 1_000_000.0, "m")
  } else if abs >= 1_000.0 {
    (value as f64 / 1_000.0, "k")
  } else {
    return value.separate_with_commas();
  };

  let rounded = (scaled * 10.0).round() / 10.0;
  if (rounded.fract() - 0.0).abs() < f64::EPSILON {
    format!("{rounded:.0}{suffix}")
  } else {
    format!("{rounded:.1}{suffix}")
  }
}

fn totals_card(title: &str, unique: i32, total: i32) -> Markup {
  let full_value = format!("{} / {}", unique.separate_with_commas(), total.separate_with_commas());
  html!(
    article class="flex-col" style="padding: 0.75rem;" {
      h6 class="mb-0" { (title) }
      h4 class="mb-0 grow flex-row items-center"
        title=(full_value)
        style="max-width: 100%; overflow: hidden; white-space: nowrap; line-height: 1; font-size: clamp(1.2rem, 1.25vw, 1.5rem);" {
        (compact_number(unique))
        " / "
        (compact_number(total))
      }
    }
  )
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
    html {
      head {
        meta charset="utf-8" {}
        meta name="viewport" content="width=device-width, initial-scale=1" {}
        title { (title) }

        link rel="icon" type="image/svg+xml" href=(PreEscaped(favicon)) {}
        link rel="stylesheet" href="https://unpkg.com/@picocss/pico@2.0" {}
        script src="https://unpkg.com/chart.js@4.4" {}
        script src="https://unpkg.com/luxon@3.5" {}
        script src="https://unpkg.com/chartjs-adapter-luxon@1.3" {}
        script src="https://unpkg.com/htmx.org@2.0" {}
        style { (PreEscaped(include_str!("../../assets/app.css"))) }
      }
      body {
        main class="container-fluid pt-0 main-box" {
          div class="flex-row items-center gap-2 justify-between" {
            nav aria-label="breadcrumb" {
              ul {
                li { a href="/" { "Repos" } }
                @for item in navs {
                  li { (maybe_url(&item)) }
                }
              }
            }

            div class="flex-row items-center gap-2" {
              div class="flex-row items-center gap-4 pr-4" style="font-size: 18px;" {
                @for (name, url) in &get_custom_links() {
                  a href=(url) target="_blank" { (name) }
                }
              }

              @if is_new_release {
                a href=(format!("https://github.com/vladkens/ghstats/releases/tag/v{last_release}"))
                  target="_blank" class="no-underline"
                  data-tooltip="New release available!" data-placement="bottom" { "🚨" }
              }

              a href="https://github.com/vladkens/ghstats"
                class="secondary flex-row items-center gap-2 no-underline font-mono"
                style="font-size: 18px;"
                target="_blank"
              {
                (format!("{} v{}", app_name, app_version))
              }
            }
          }

          (inner)
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
  let items: Vec<TablePopularItem> = match kind {
    PopularKind::Refs => items
      .into_iter()
      .map(|x| TablePopularItem { item: (x.name, None), uniques: x.uniques, count: x.count })
      .collect(),
    PopularKind::Path => items
      .into_iter()
      .map(|x| {
        let prefix = format!("/{}", repo);
        let mut name = x.name.replace(&prefix, "");
        if name.is_empty() {
          name = "/".to_string();
        }

        let item = (name, Some(format!("https://github.com{}", x.name)));
        TablePopularItem { item, uniques: x.uniques, count: x.count }
      })
      .collect(),
  };

  let name = match kind {
    PopularKind::Refs => "Referring sites",
    PopularKind::Path => "Popular paths",
  };

  let html_id = match kind {
    PopularKind::Refs => "refs_table",
    PopularKind::Path => "path_table",
  };

  let cols: Vec<PopularColumn> = vec![
    (name, Box::new(|x| maybe_url(&x.item)), PopularSort::Name),
    ("Views", Box::new(|x| html!((x.count.separate_with_commas()))), PopularSort::Count),
    ("Unique", Box::new(|x| html!((x.uniques.separate_with_commas()))), PopularSort::Uniques),
  ];

  fn filter_url(repo: &str, qs: &PopularFilter, col: &PopularSort) -> String {
    let dir = match qs.sort == *col && qs.direction == Direction::Desc {
      true => "asc",
      false => "desc",
    };

    format!("/{}?sort={}&direction={}&period={}", repo, col, dir, qs.period)
  }

  let html = html!(
    article id=(html_id) class="p-0 mb-0 table-popular" {
      table class="mb-0" {
        thead {
          tr {
            @for (idx, col) in cols.iter().enumerate() {
              th scope="col" .cursor-pointer .select-none .text-right[idx > 0]
                hx-trigger="click"
                hx-get=(filter_url(repo, qs, &col.2))
                hx-target=(format!("#{}", html_id))
                hx-swap="outerHTML"
                hx-replace-url="true"
              {
                (col.0)
                @if col.2 == qs.sort {
                  span class="ml-0.5" {
                    @if qs.direction == Direction::Asc { "↑" } @else { "↓" }
                  }
                }
              }
            }
          }
        }

        tbody {
          @if items.is_empty() {
            tr {
              td colspan=(cols.len()) .text-center { "No data for given period" }
            }
          }

          @for item in items {
            tr {
              @for (idx, col) in cols.iter().enumerate() {
                td .text-right[idx > 0] { ((col.1)(&item)) }
              }
            }
          }
        }
      }
    }
  );

  Ok(html)
}

async fn repo_popular_tables(db: &DbClient, repo: &str, filter: &PopularFilter) -> HtmlRes {
  let html = html!(
    div id="popular_tables" class="grid" {
      (popular_table(db, repo, &PopularKind::Refs, filter).await?)
      (popular_table(db, repo, &PopularKind::Path, filter).await?)
    }
  );

  Ok(html)
}

pub async fn repo_page(
  State(state): State<Arc<AppState>>,
  Path((owner, repo)): Path<(String, String)>,
  OriginalUri(uri): OriginalUri,
  headers: HeaderMap,
) -> HtmlRes {
  let repo = format!("{}/{}", owner, repo);
  let mut qs: Query<PopularFilter> = Query::try_from_uri(&uri)?;
  let db = &state.db;

  let periods = vec![
    (7, "Last 7 days"),
    (14, "Last 14 days"),
    (30, "Last 30 days"),
    (90, "Last 90 days"),
    (-1, "All time"),
  ];

  qs.period = match periods.iter().all(|x| x.0 != qs.period) {
    true => 30,
    false => qs.period,
  };

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

  let metrics = db.get_metrics(&repo).await?;
  let stars = db.get_stars(&repo).await?;

  let html = html!(
    div class="grid" style="grid-template-columns: 1fr 2fr;" {
      div class="grid" style="grid-template-rows: 2fr 1fr; grid-template-columns: 1fr;" {
        article class="mb-0" {
          hgroup class="flex-row flex-col gap-2" {
            h3 {
              a href=(format!("https://github.com/{}", repo)) class="contrast" { (totals.name) }
            }
            p { (totals.description.unwrap_or("".to_string())) }
          }
        }

        div class="grid" {
          (totals_card("Clones (U/T)", totals.clones_uniques, totals.clones_count))
          (totals_card("Views (U/T)", totals.views_uniques, totals.views_count))
        }
      }

      article class="flex-col" {
        h6 { "Stars" }
        div class="grow" { canvas id="chart_stars" {} }
      }
    }

    div class="grid" {
      @for (title, canvas_id) in [("Clones", "chart_clones"), ("Views", "chart_views")] {
        article {
          h6 { (title) }
          canvas id=(canvas_id) {}
        }
      }
    }

    script { (PreEscaped(include_str!("../../assets/app.js"))) }
    script {
      "const Metrics = "(PreEscaped(serde_json::to_string(&metrics)?))";"
      "const Stars = "(PreEscaped(serde_json::to_string(&stars)?))";"
      "renderMetrics('chart_clones', Metrics, 'clones_uniques', 'clones_count');"
      "renderMetrics('chart_views', Metrics, 'views_uniques', 'views_count');"
      "renderStars('chart_stars', Stars);"
    }

    select name="period" hx-get=(format!("/{}", repo)) hx-target="#popular_tables" hx-swap="outerHTML" hx-push-url="true" {
      @for (days, title) in &periods {
        option value=(days) selected[*days == qs.period] { (title) }
      }
    }

    (repo_popular_tables(db, &repo, &qs).await?)
  );

  Ok(base(&state, vec![(repo, None)], html))
}

// https://docs.rs/axum/latest/axum/extract/index.html#common-extractors
pub async fn index(
  State(state): State<Arc<AppState>>,
  OriginalUri(uri): OriginalUri,
  headers: HeaderMap,
) -> HtmlRes {
  // let qs: Query<HashMap<String, String>> = Query::try_from_uri(&uri)?;
  let qs: Query<RepoFilter> = Query::try_from_uri(&uri)?;
  let repos = state.get_repos_filtered(&qs).await?;
  let owners = state.get_owners().await?;

  let cols: Vec<RepoColumn> = vec![
    ("Name", Box::new(|x| html!(a href=(format!("/{}", x.name)) { (x.name) })), RepoSort::Name),
    ("Issues", Box::new(|x| html!((x.issues.separate_with_commas()))), RepoSort::Issues),
    ("PRs", Box::new(|x| html!((x.prs.separate_with_commas()))), RepoSort::Prs),
    ("Forks", Box::new(|x| html!((x.forks.separate_with_commas()))), RepoSort::Forks),
    ("Clones", Box::new(|x| html!((x.clones_count.separate_with_commas()))), RepoSort::Clones),
    ("Stars", Box::new(|x| html!((x.stars.separate_with_commas()))), RepoSort::Stars),
    ("Views", Box::new(|x| html!((x.views_count.separate_with_commas()))), RepoSort::Views),
  ];

  fn filter_url(qs: &RepoFilter, col: &RepoSort) -> String {
    let dir = match qs.sort == *col && qs.direction == Direction::Desc {
      true => "asc",
      false => "desc",
    };

    let mut query = Serializer::new(String::new());
    query.append_pair("sort", &col.to_string());
    query.append_pair("direction", dir);
    if let Some(q) = &qs.q
      && !q.is_empty()
    {
      query.append_pair("q", q);
    }
    if let Some(owner) = &qs.owner
      && !owner.is_empty()
    {
      query.append_pair("owner", owner);
    }
    format!("/?{}", query.finish())
  }

  let cur_q = qs.q.clone().unwrap_or_default();
  let cur_owner = qs.owner.clone().unwrap_or_default();

  let table_html = html!(
      table id="repos_table" {
        thead {
          tr {
            @for col in &cols {
              th scope="col" class="cursor-pointer select-none"
                hx-trigger="click"
                hx-get=(filter_url(&qs, &col.2))
                hx-target="#repos_table"
                hx-swap="outerHTML"
                hx-replace-url="true"
                {
                  (col.0)
                  @if col.2 == qs.sort {
                    span class="ml-0.5" {
                      @if qs.direction == Direction::Asc { "↑" } @else { "↓" }
                    }
                  }
                }
            }
          }
        }
        tbody {
          @for repo in &repos {
            tr {
              @for col in &cols {
                td { ((col.1)(repo)) }
              }
            }
          }
        }
      }
  );

  let sort_inputs = html!(
    input type="hidden" id="filter_sort" name="sort" value=(qs.sort) hx-swap-oob="true" {}
    input type="hidden" id="filter_direction" name="direction" value=(qs.direction) hx-swap-oob="true" {}
  );

  if let Some("repos_table") = get_hx_target(&headers) {
    return Ok(html!((table_html)(sort_inputs)));
  }

  let html = html!(
    div class="flex-row gap-4 mb-0" {
      input type="hidden" id="filter_sort" name="sort" value=(qs.sort) {}
      input type="hidden" id="filter_direction" name="direction" value=(qs.direction) {}

      @if owners.len() > 1 {
        select name="owner" class="mb-0"
          style="width: auto; height: calc(1.5em + 0.5rem + 2px); padding: 0.25rem 2rem 0.25rem 0.5rem; background-position: center right 0.25rem; background-size: 0.75rem auto;"
          hx-get="/"
          hx-target="#repos_table"
          hx-swap="outerHTML"
          hx-include="[name='q'], #filter_sort, #filter_direction"
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
        class="mb-0"
        style="padding: 0.25rem 0.5rem 0.25rem 2.75rem; height: auto;"
        hx-get="/"
        hx-trigger="keyup changed delay:300ms, search"
        hx-target="#repos_table"
        hx-swap="outerHTML"
        hx-include="[name='owner'], #filter_sort, #filter_direction"
        hx-replace-url="true"
      {}
    }

    (table_html)
  );

  Ok(base(&state, vec![], html))
}
