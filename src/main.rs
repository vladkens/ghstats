use std::sync::Arc;

use axum::{
  extract::{Path, State},
  routing::get,
  Router,
};
use db_client::{get_db, get_metrics, RepoDto};
use maud::{html, Markup, PreEscaped};
use sqlx::SqlitePool;
use utils::{HtmlRes, Res};

mod db_client;
mod gh_client;
mod utils;

struct AppState {
  db: SqlitePool,
}

fn base(inner: Markup) -> Markup {
  html!(
    html {
      head {
        link rel="stylesheet" href="https://unpkg.com/@picocss/pico@2.0.6/css/pico.min.css" {}
        script src="https://unpkg.com/chart.js@4.4.3/dist/chart.umd.js" {}
        style { (PreEscaped(include_str!("app.css"))) }
      }
      body {
        main class="container" {
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

  Ok(base(html!(
    nav aria-label="breadcrumb" {
      ul {
        li { a href="/" { "Repos" } }
        li { (repo) }
      }
    }

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
  )))
}

async fn index(State(state): State<Arc<AppState>>) -> HtmlRes {
  let db = &state.db;
  let repos = db_client::get_repos(db).await?;

  Ok(base(html!(
    nav aria-label="breadcrumb" {
      ul {
        li { a href="/" { "Repos" } }
      }
    }

    ul {
      @for repo in repos {
        li { a href=(format!("/{}", repo.name)) { (repo.name) } }
      }
    }
  )))
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
