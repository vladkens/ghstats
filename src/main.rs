use std::sync::Arc;

use axum::{
  extract::{Path, State},
  routing::get,
  Router,
};
use db_client::{get_db, get_metrics, RepoMetrics};
use maud::{html, Markup, PreEscaped};
use sqlx::SqlitePool;
use thousands::Separable;
use utils::{HtmlRes, Res};

mod db_client;
mod gh_client;
mod utils;

struct AppState {
  db: SqlitePool,
}

fn icon_link(name: &str, url: &str) -> Markup {
  // https://simpleicons.org/?q=github
  html!(
    a href=(url) class="secondary" style="text-decoration: none" target="_blank" {
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
        main class="container" {
          div class="header" {
            nav aria-label="breadcrumb" {
              ul {
                li { a href="/" { "Repos" } }
                @for (name, url) in navs {
                  li {
                    @if let Some(url) = url {
                      a href=(url) { (name) }
                    } else {
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

async fn repo_page(
  State(state): State<Arc<AppState>>,
  Path((user, name)): Path<(String, String)>,
) -> HtmlRes {
  let db = &state.db;
  let repo = format!("{}/{}", user, name);
  let metrics = get_metrics(db, &repo).await?;

  let html = html!(
    div class="grid" {
      @for (title, canvas_id) in vec![("Clones", "chart_clones"), ("Views", "chart_views")] {
        article {
          h5 { (title) }
          canvas id=(canvas_id) {}
        }
      }
    }

    script { "const metrics = "(PreEscaped(serde_json::to_string(&metrics)?)); }
    script { (PreEscaped(include_str!("app.js"))) }
  );

  Ok(base(vec![(repo, None)], html))
}

async fn index(State(state): State<Arc<AppState>>) -> HtmlRes {
  let db = &state.db;

  let mut repos = db_client::get_repos(db).await?;
  repos.sort_by(|a, b| b.stars.cmp(&a.stars));

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

#[tokio::main]
async fn main() -> Res {
  dotenv::dotenv().ok();

  let db = get_db("demo.db").await?;
  // db_client::update_metrics(&db).await?;

  let state = Arc::new(AppState { db });
  let app =
    Router::new().route("/", get(index)).route("/:user/:name", get(repo_page)).with_state(state);

  let address = "127.0.0.1:8080";
  let listener = tokio::net::TcpListener::bind(address).await.unwrap();
  println!("Listening on http://{}", address);
  axum::serve(listener, app.into_make_service()).await.unwrap();

  Ok(())
}
