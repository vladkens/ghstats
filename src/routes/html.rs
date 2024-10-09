use std::sync::Arc;

use axum::extract::{Path, Query, Request, State};
use maud::{html, Markup, PreEscaped};
use thousands::Separable;

use crate::db_client::{
  DbClient, Direction, PopularFilter, PopularKind, PopularSort, RepoFilter, RepoSort, RepoTotals,
};
use crate::helpers::{get_filtered_repos, is_repo_included};
use crate::types::{AppError, HtmlRes};
use crate::AppState;

#[derive(Debug)]
struct TablePopularItem {
  item: (String, Option<String>), // title, url
  uniques: i64,
  count: i64,
}

fn get_hx_target(req: &Request) -> Option<&str> {
  crate::helpers::get_header(req, "hx-target")
}

fn maybe_url(item: &(String, Option<String>)) -> Markup {
  let (name, url) = item;

  match url {
    Some(url) => html!(a href=(url) { (name) }),
    None => html!(span { (name) }),
  }
}

fn get_custom_links() -> Vec<(String, String)> {
  let links = std::env::var("GHS_CUSTOM_LINKS").unwrap_or_default();
  let links: Vec<(String, String)> = links
    .split(",")
    .map(|x| {
      let parts: Vec<&str> = x.split("|").collect();
      if parts.len() != 2 {
        return None;
      }

      if parts[0].is_empty() || parts[1].is_empty() {
        return None;
      }

      Some((parts[0].to_string(), parts[1].to_string()))
    })
    .filter(|x| x.is_some())
    .map(|x| x.unwrap())
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

  let cols: Vec<(&str, Box<dyn Fn(&TablePopularItem) -> Markup>, PopularSort)> = vec![
    (name, Box::new(|x| maybe_url(&x.item)), PopularSort::Name),
    ("Views", Box::new(|x| html!((x.count.separate_with_commas()))), PopularSort::Count),
    ("Unique", Box::new(|x| html!((x.uniques.separate_with_commas()))), PopularSort::Uniques),
  ];

  fn filter_url(repo: &str, qs: &PopularFilter, col: &PopularSort) -> String {
    let dir = match qs.sort == *col && qs.direction == Direction::Desc {
      true => "asc",
      false => "desc",
    };

    format!("/{}?sort={}&direction={}", repo, col, dir)
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

  return Ok(html);
}

pub async fn repo_page(
  State(state): State<Arc<AppState>>,
  Path((owner, repo)): Path<(String, String)>,
  req: Request,
) -> HtmlRes {
  let repo = format!("{}/{}", owner, repo);
  if !is_repo_included(&repo) {
    return AppError::not_found();
  }

  let mut qs: Query<PopularFilter> = Query::try_from_uri(req.uri())?;
  let db = &state.db;

  let periods = vec![
    (7, "Last 7 days"),
    (14, "Last 14 days"),
    (30, "Last 30 days"),
    (90, "Last 90 days"),
    (-1, "All time"),
  ];

  qs.period = match periods.iter().all(|x| x.0 != qs.period) {
    true => 7,
    false => qs.period,
  };

  match get_hx_target(&req) {
    Some("refs_table") => return Ok(popular_table(db, &repo, &PopularKind::Refs, &qs).await?),
    Some("path_table") => return Ok(popular_table(db, &repo, &PopularKind::Path, &qs).await?),
    Some("popular_tables") => return Ok(repo_popular_tables(&db, &repo, &qs).await?),
    _ => {}
  }

  let totals = match db.get_repo_totals(&repo).await? {
    Some(x) => x,
    None => return AppError::not_found(),
  };

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
          article class="flex-col" {
            h6 class="mb-0" { "Total Clones" }
            h4 class="mb-0 grow flex-row items-center" {
              (totals.clones_uniques.separate_with_commas())
              " / "
              (totals.clones_count.separate_with_commas())
            }
          }
          article class="flex-col" {
            h6 class="mb-0" { "Total Views" }
            h4 class="mb-0 grow flex-row items-center" {
              (totals.views_uniques.separate_with_commas())
              " / "
              (totals.views_count.separate_with_commas())
            }
          }
        }
      }

      article class="flex-col" {
        h6 { "Stars" }
        div class="grow" { canvas id="chart_stars" {} }
      }
    }

    div class="grid" {
      @for (title, canvas_id) in vec![("Clones", "chart_clones"), ("Views", "chart_views")] {
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

    select name="period" hx-get=(format!("/{}", repo)) hx-target="#popular_tables" hx-swap="outerHTML" {
      @for (days, title) in &periods {
        option value=(days) selected[*days == qs.period] { (title) }
      }
    }

    (repo_popular_tables(db, &repo, &qs).await?)
  );

  Ok(base(&state, vec![(repo, None)], html))
}

// https://docs.rs/axum/latest/axum/extract/index.html#common-extractors
pub async fn index(State(state): State<Arc<AppState>>, req: Request) -> HtmlRes {
  // let qs: Query<HashMap<String, String>> = Query::try_from_uri(req.uri())?;
  let db = &state.db;
  let qs: Query<RepoFilter> = Query::try_from_uri(req.uri())?;
  let repos = get_filtered_repos(&db, &qs).await?;

  let cols: Vec<(&str, Box<dyn Fn(&RepoTotals) -> Markup>, RepoSort)> = vec![
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

    format!("/?sort={}&direction={}", col, dir)
  }

  let html = html!(
      table id="repos_table" {
        thead {
          tr {
            @for col in &cols {
              th scope="col" class="cursor-pointer select-none"
                hx-trigger="click"
                hx-get=(filter_url(&qs, &col.2))
                hx-target="#repos_table"
                hx-swap="outerHTML"
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
                td { ((col.1)(&repo)) }
              }
            }
          }
        }
      }
  );

  match get_hx_target(&req) {
    Some("repos_table") => return Ok(html),
    _ => {}
  }

  Ok(base(&state, vec![], html))
}
