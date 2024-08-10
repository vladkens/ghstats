use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, Request, State};
use maud::{html, Markup, PreEscaped};
use thousands::Separable;

use crate::db_client::{DbClient, Direction, RepoFilter, RepoMetrics, RepoSort};
use crate::utils::HtmlRes;
use crate::AppState;

#[derive(Debug)]
struct TablePopularItem {
  item: (String, Option<String>), // title, url
  uniques: i64,
  count: i64,
}

fn get_hx_target(req: &Request) -> Option<&str> {
  match req.headers().get("hx-target") {
    Some(x) => Some(x.to_str().unwrap_or_default()),
    None => None,
  }
}

fn maybe_url(item: &(String, Option<String>)) -> Markup {
  let (name, url) = item;

  match url {
    Some(url) => html!(a href=(url) { (name) }),
    None => html!(span { (name) }),
  }
}

fn base(navs: Vec<(String, Option<String>)>, inner: Markup) -> Markup {
  let brand = format!("{} v{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));

  html!(
    html {
      head {
        link rel="stylesheet" href="https://unpkg.com/@picocss/pico@2.0.6/css/pico.min.css" {}
        link rel="stylesheet" href="https://unpkg.com/simple-icons-font@v13/font/simple-icons.min.css" {}
        script src="https://unpkg.com/chart.js@4.4.3/dist/chart.umd.js" {}
        script src="https://unpkg.com/htmx.org@2.0.1" {}
        style { (PreEscaped(include_str!("app.css"))) }
      }
      body {
        main class="container pt-0" {
          div class="header" {
            nav aria-label="breadcrumb" {
              ul {
                li { a href="/" { "Repos" } }
                @for item in navs {
                  li { (maybe_url(&item)) }
                }
              }
            }

            div class="flex items-center gap-2" {
              a href="https://github.com/vladkens/ghstats"
                class="secondary flex items-center gap-2 no-underline font-mono"
                style="font-size: 18px;"
                target="_blank"
              {
                (brand)
                i class="si si-github" style="font-size: 22px;" {}
              }
            }
          }

          (inner)
        }
      }
    }
  )
}

fn popular_table(items: &Vec<TablePopularItem>, name: &str, html_id: &str) -> Markup {
  html!(
    article id=(html_id) class="p-0 mb-0 table-popular" {
      table class="mb-0" {
        thead {
          tr {
            th scope="col" class="" { (name) }
            th scope="col" class="text-right" { "Count" }
            th scope="col" class="text-right" { "Uniques" }
          }
        }

        tbody {
          @for item in items {
            tr {
              td class="" { (maybe_url(&item.item)) }
              td class="text-right" { (item.count.separate_with_commas()) }
              td class="text-right" { (item.uniques.separate_with_commas()) }
            }
          }
        }
      }
    }
  )
}

async fn repo_refs_table(db: &DbClient, repo: &str, granularity: i32) -> HtmlRes {
  let repo_popular_refs = db.get_popular_items("repo_referrers", &repo, granularity).await?;
  let repo_popular_refs: Vec<TablePopularItem> = repo_popular_refs
    .into_iter()
    .map(|x| TablePopularItem { item: (x.name, None), uniques: x.uniques, count: x.count })
    .collect();

  Ok(popular_table(&repo_popular_refs, "Referring sites", "refs_table"))
}

async fn repo_path_table(db: &DbClient, repo: &str, granularity: i32) -> HtmlRes {
  let repo_popular_path = db.get_popular_items("repo_popular_paths", &repo, granularity).await?;
  let repo_popular_path: Vec<TablePopularItem> = repo_popular_path
    .into_iter()
    .map(|x| {
      let prefix = format!("/{}", repo);
      let mut name = x.name.replace(&prefix, "");
      if name.is_empty() {
        name = "/".to_string();
      }

      TablePopularItem {
        item: (name, Some(format!("https://github.com{}", x.name))),
        uniques: x.uniques,
        count: x.count,
      }
    })
    .collect();

  Ok(popular_table(&repo_popular_path, "Popular paths", "path_table"))
}

async fn repo_popular_tables(db: &DbClient, repo: &str, granularity: i32) -> HtmlRes {
  let html = html!(
    div id="popular_tables" class="grid" {
      (repo_refs_table(&db, &repo, granularity).await?)
      (repo_path_table(&db, &repo, granularity).await?)
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
  let qs: Query<HashMap<String, String>> = Query::try_from_uri(req.uri())?;
  let db = &state.db;

  let granularities = vec![
    (7, "Last 7 days"),
    (14, "Last 14 days"),
    (30, "Last 30 days"),
    (90, "Last 90 days"),
    (-1, "All time"),
  ];

  let granularity = qs.get("granularity").unwrap_or(&"7".to_string()).parse::<i32>().unwrap();
  let granularity = match granularities.iter().all(|x| x.0 != granularity) {
    true => 7,
    false => granularity,
  };

  match get_hx_target(&req) {
    Some("refs_table") => return Ok(repo_refs_table(&db, &repo, granularity).await?),
    Some("path_table") => return Ok(repo_path_table(&db, &repo, granularity).await?),
    Some("popular_tables") => return Ok(repo_popular_tables(&db, &repo, granularity).await?),
    _ => {}
  }

  let totals = match db.get_repo_totals(&repo).await? {
    Some(x) => x,
    None => return Ok(base(vec![], html!(h1 { "Repo not found" }))),
  };

  let metrics = db.get_metrics(&repo).await?;
  let stars = db.get_stars(&repo).await?;

  let html = html!(
    div class="grid" style="grid-template-columns: 1fr 2fr;" {
      div class="grid" style="grid-template-rows: 2fr 1fr; grid-template-columns: 1fr;" {
        article class="mb-0" {
          hgroup class="flex flex-col gap-2" {
            h3 {
              a href=(format!("https://github.com/{}", repo)) class="contrast" { (totals.name) }
            }
            p { (totals.description.unwrap_or("".to_string())) }
          }
        }

        div class="grid" {
          article class="flex-col" {
            h6 class="mb-0" { "Total Clones" }
            h4 class="mb-0 grow flex items-center" {
              (totals.clones_uniques.separate_with_commas())
              " / "
              (totals.clones_count.separate_with_commas())
            }
          }
          article class="flex-col" {
            h6 class="mb-0" { "Total Views" }
            h4 class="mb-0 grow flex items-center" {
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

    script { (PreEscaped(include_str!("app.js"))) }
    script {
      "const Metrics = "(PreEscaped(serde_json::to_string(&metrics)?))";"
      "const Stars = "(PreEscaped(serde_json::to_string(&stars)?))";"
      "renderMetrics('chart_clones', Metrics, 'clones_uniques', 'clones_count');"
      "renderMetrics('chart_views', Metrics, 'views_uniques', 'views_count');"
      "renderStars('chart_stars', Stars);"
    }

    select name="granularity" hx-get=(format!("/{}", repo)) hx-target="#popular_tables" {
      @for (days, title) in &granularities {
        option value=(days) selected[*days == granularity] { (title) }
      }
    }

    (repo_popular_tables(db, &repo, granularity).await?)
  );

  Ok(base(vec![(repo, None)], html))
}

// https://docs.rs/axum/latest/axum/extract/index.html#common-extractors
pub async fn index(State(state): State<Arc<AppState>>, req: Request) -> HtmlRes {
  // let qs: Query<HashMap<String, String>> = Query::try_from_uri(req.uri())?;
  let qs: Query<RepoFilter> = Query::try_from_uri(req.uri())?;

  let db = &state.db;
  let repos = db.get_repos(&qs).await?;

  let cols: Vec<(&str, Box<dyn Fn(&RepoMetrics) -> Markup>, RepoSort)> = vec![
    ("Name", Box::new(|x| html!(a href=(format!("/{}", x.name)) { (x.name) })), RepoSort::Name),
    ("Issues", Box::new(|x| html!((x.issues.separate_with_commas()))), RepoSort::Issues),
    ("Forks", Box::new(|x| html!((x.forks.separate_with_commas()))), RepoSort::Forks),
    ("Stars", Box::new(|x| html!((x.stars.separate_with_commas()))), RepoSort::Stars),
    ("Clones", Box::new(|x| html!((x.clones_count.separate_with_commas()))), RepoSort::Clones),
    ("Views", Box::new(|x| html!((x.views_count.separate_with_commas()))), RepoSort::Views),
  ];

  fn filter_url(qs: &RepoFilter, col: &RepoSort) -> String {
    let dir = match qs.sort == *col && qs.direction == Direction::Asc {
      true => "desc",
      false => "asc",
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

  Ok(base(vec![], html))
}
