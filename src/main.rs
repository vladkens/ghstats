use axum::{routing::get, Router};
use maud::{html, Markup};

pub type WithError<T> = Result<T, Box<dyn std::error::Error + Sync + Send>>;

mod collect;

async fn index() -> Markup {
  html! {
      h1 { "Hello, World!" }
  }
}

#[tokio::main]
async fn main() -> WithError<()> {
  dotenv::dotenv().ok();
  let client = collect::ApiClient::new().unwrap();

  let items = client.get_repos("users/vladkens").await?;
  for item in items {
    println!("{:?}", item);
  }

  let app = Router::new() //
    .route("/", get(index));

  let address = "127.0.0.1:8080";
  let listener = tokio::net::TcpListener::bind(address).await.unwrap();
  println!("Listening on http://{}", address);
  axum::serve(listener, app.into_make_service()).await.unwrap();

  Ok(())
}
