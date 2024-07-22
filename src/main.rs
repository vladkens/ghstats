use axum::{routing::get, Router};
use maud::{html, Markup};
use utils::WithError;

mod db_client;
mod gh_client;
mod utils;

async fn index() -> Markup {
  html! {
    h1 { "Hello, World!" }
  }
}

async fn update_metrics() -> WithError {
  let gh = gh_client::GhClient::new().unwrap();
  let db = db_client::DbClient::new("test.db").await?;

  let repos = gh.get_repos("users/vladkens").await?;
  for repo in repos {
    if repo.fork || repo.archived {
      continue;
    }

    let id = db.insert_repo(&repo).await?;

    let rs = gh.traffic_clones(&repo.full_name).await?;
    db.insert_traffic_clones(id, &rs).await?;

    let rs = gh.traffic_views(&repo.full_name).await?;
    db.insert_traffic_views(id, &rs).await?;

    let rs = gh.traffic_paths(&repo.full_name).await?;
    db.insert_traffic_paths(id, &rs).await?;

    let rs = gh.traffic_refs(&repo.full_name).await?;
    db.insert_traffic_refs(id, &rs).await?;
  }

  Ok(())
}

#[tokio::main]
async fn main() -> WithError {
  dotenv::dotenv().ok();

  //   update_metrics().await?;

  let app = Router::new() //
    .route("/", get(index));

  let address = "127.0.0.1:8080";
  let listener = tokio::net::TcpListener::bind(address).await.unwrap();
  println!("Listening on http://{}", address);
  axum::serve(listener, app.into_make_service()).await.unwrap();

  Ok(())
}
