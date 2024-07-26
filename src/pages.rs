use std::sync::Arc;

use axum::extract::{Path, State};
use maud::{html, Markup, PreEscaped};
use thousands::Separable;

use crate::db_client::RepoMetrics;
use crate::utils::HtmlRes;
use crate::AppState;

#[derive(Debug)]
struct TablePopularItem {
  item: (String, Option<String>), // title, url
  uniques: i64,
  count: i64,
}

fn base(navs: Vec<(String, Option<String>)>, inner: Markup) -> Markup {
  let brand = format!("{} v{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));

  html!(
    html {
      head {
        link rel="stylesheet" href="https://unpkg.com/@picocss/pico@2.0.6/css/pico.min.css" {}
        link rel="stylesheet" href="https://unpkg.com/simple-icons-font@v13/font/simple-icons.min.css" {}
        script src="https://unpkg.com/chart.js@4.4.3/dist/chart.umd.js" {}
        style { (PreEscaped(include_str!("app.css"))) }
      }
      body {
        main class="container pt-0" {
          div class="header" {
            nav aria-label="breadcrumb" {
              ul {
                li { a href="/" { "Repos" } }
                @for (name, url) in navs {
                  li {
                    @if let Some(url) = url {
                      a href=(url) { (name) }
                    } @else {
                      (name)
                    }
                  }
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

fn popular_table(items: &Vec<TablePopularItem>, name: &str) -> Markup {
  html!(
    article class="p-0 mb-0 table-popular" {
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
              td class="" {
                @if let Some(url) = &item.item.1 {
                  a href=(url) { (item.item.0) }
                } @else {
                  (item.item.0)
                }
              }
              td class="text-right" { (item.count.separate_with_commas()) }
              td class="text-right" { (item.uniques.separate_with_commas()) }
            }
          }
        }
      }
    }
  )
}

pub async fn repo_page(
  State(state): State<Arc<AppState>>,
  Path((user, name)): Path<(String, String)>,
) -> HtmlRes {
  let db = &state.db;
  let repo = format!("{}/{}", user, name);
  let metrics = db.get_metrics(&repo).await?;
  let stars = db.get_stars(&repo).await?;
  let totals = db.get_repo_totals(&repo).await?;

  let repo_popular_refs = db.get_popular_items("repo_referrers", &repo).await?;
  let repo_popular_refs: Vec<TablePopularItem> = repo_popular_refs
    .into_iter()
    .map(|x| TablePopularItem { item: (x.name, None), uniques: x.uniques, count: x.count })
    .collect();

  let repo_popular_path = db.get_popular_items("repo_popular_paths", &repo).await?;
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

    div class="grid" {
      (popular_table(&repo_popular_refs, "Referring sites"))
      (popular_table(&repo_popular_path, "Popular content"))
    }
  );

  Ok(base(vec![(repo, None)], html))
}

pub async fn index(State(state): State<Arc<AppState>>) -> HtmlRes {
  let db = &state.db;
  let repos = db.get_repos().await?;

  let cols: Vec<(&str, Box<dyn Fn(&RepoMetrics) -> Markup>)> = vec![
    ("Name", Box::new(|x| html!(a href=(format!("/{}", x.name)) { (x.name) }))),
    ("Issues", Box::new(|x| html!((x.issues.separate_with_commas())))),
    ("Forks", Box::new(|x| html!((x.forks.separate_with_commas())))),
    ("Stars", Box::new(|x| html!((x.stars.separate_with_commas())))),
    ("Clones", Box::new(|x| html!((x.clones_count.separate_with_commas())))),
    ("Views", Box::new(|x| html!((x.views_count.separate_with_commas())))),
  ];

  let html = html!(table {
    thead {
      tr {
        @for col in &cols {
          th scope="col" { (col.0) }
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
  });

  Ok(base(vec![], html))
}
