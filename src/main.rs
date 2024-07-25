use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{http::StatusCode, response::IntoResponse, Router};
use maud::{html, Markup, PreEscaped};
use sqlx::SqlitePool;
use thousands::Separable;

use db_client::{
  get_db, get_metrics, get_popular_items, get_repo_totals, get_repos, get_stars, update_deltas,
  update_metrics, RepoMetrics,
};
use gh_client::GhClient;
use utils::{HtmlRes, Res};

mod db_client;
mod gh_client;
mod utils;

struct AppState {
  db: SqlitePool,
  gh: GhClient,
}

impl AppState {
  async fn new() -> Res<Self> {
    let gh_token = std::env::var("GITHUB_TOKEN")?;
    let db_path = std::env::var("DB_PATH").unwrap_or("ghstats.db".to_string());

    let db = get_db(&db_path).await?;
    let gh = GhClient::new(gh_token)?;

    Ok(Self { db, gh })
  }
}

fn icon_link(name: &str, url: &str) -> Markup {
  // https://simpleicons.org/?q=github
  html!(
    a href=(url) class="secondary no-underline" target="_blank" {
      i class=(format!("si si-{}", name)) {}
    }
  )
}

fn base(navs: Vec<(String, Option<String>)>, inner: Markup) -> Markup {
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

            div {
              (icon_link("github", "https://github.com/vladkens/ghstats"))
            }
          }

          (inner)
        }
      }
    }
  )
}

#[derive(Debug)]
struct TablePopularItem {
  item: (String, Option<String>), // title, url
  uniques: i64,
  count: i64,
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

async fn repo_page(
  State(state): State<Arc<AppState>>,
  Path((user, name)): Path<(String, String)>,
) -> HtmlRes {
  let db = &state.db;
  let repo = format!("{}/{}", user, name);
  let metrics = get_metrics(db, &repo).await?;
  let stars = get_stars(db, &repo).await?;
  let totals = get_repo_totals(db, &repo).await?;

  let repo_popular_refs = get_popular_items(db, "repo_referrers", &repo).await?;
  let repo_popular_refs: Vec<TablePopularItem> = repo_popular_refs
    .into_iter()
    .map(|x| TablePopularItem { item: (x.name, None), uniques: x.uniques, count: x.count })
    .collect();

  let repo_popular_path = get_popular_items(db, "repo_popular_paths", &repo).await?;
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
              a href=(format!("https://github.com/{}", repo)) class="contrast no-underline" { (totals.name) }
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

async fn index(State(state): State<Arc<AppState>>) -> HtmlRes {
  let db = &state.db;

  let repos = db_client::get_repos(db).await?;

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

async fn health() -> impl IntoResponse {
  let msg = serde_json::json!({ "status": "ok" });
  (StatusCode::OK, axum::response::Json(msg))
}

async fn start_cron(state: Arc<AppState>) -> Res {
  use tokio_cron_scheduler::{Job, JobScheduler};

  update_deltas(&state.db).await?;

  // if new db, update metrics immediately
  let repos = get_repos(&state.db).await?;
  if repos.len() == 0 {
    match update_metrics(&state.db, &state.gh).await {
      Err(e) => tracing::error!("error updating metrics: {:?}", e),
      Ok(_) => {}
    }
  }

  // https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api?apiVersion=2022-11-28
  // >> All of these requests count towards your personal rate limit of 5,000 requests per hour.

  // https://docs.github.com/en/repositories/viewing-activity-and-data-for-your-repository/viewing-traffic-to-a-repository
  // >> Full clones and visitor information update hourly, while referring sites and popular content sections update daily.

  // last minute of every hour
  let job = Job::new_async("0 59 * * * *", move |_, _| {
    let state = state.clone();
    Box::pin(async move {
      match update_metrics(&state.db, &state.gh).await {
        Err(e) => tracing::error!("error updating metrics: {:?}", e),
        Ok(_) => {}
      }
    })
  })?;

  let runner = JobScheduler::new().await?;
  runner.start().await?;
  runner.add(job).await?;

  Ok(())
}

#[tokio::main]
async fn main() -> Res {
  use tower_http::trace::{self, TraceLayer};
  use tracing::Level;

  dotenv::dotenv().ok();
  tracing_subscriber::fmt().with_target(false).compact().init();

  let router = Router::new()
    .route("/health", get(health))
    .route("/", get(index))
    .route("/:user/:name", get(repo_page));

  let router = router.layer(
    TraceLayer::new_for_http()
      .make_span_with(trace::DefaultMakeSpan::new().level(Level::INFO))
      .on_response(trace::DefaultOnResponse::new().level(Level::INFO)),
  );

  let state = Arc::new(AppState::new().await?);
  let service = router.with_state(state.clone()).into_make_service();
  start_cron(state.clone()).await?;

  let addr = "127.0.0.1:8080";
  let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
  tracing::info!("listening on {}", addr);
  axum::serve(listener, service).await.unwrap();

  Ok(())
}
