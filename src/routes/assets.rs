use axum::extract::Path;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

// Third-party frontend libs, embedded into the binary so pages don't depend on a CDN.
// Downloaded with `make vendor`; links carry `?v=<app version>` to bust the cache on upgrade.
const VENDOR: &[(&str, &str, &[u8])] = &[
  ("chart.umd.js", "text/javascript", include_bytes!("../../assets/vendor/chart.umd.js")),
  ("luxon.min.js", "text/javascript", include_bytes!("../../assets/vendor/luxon.min.js")),
  (
    "chartjs-adapter-luxon.umd.min.js",
    "text/javascript",
    include_bytes!("../../assets/vendor/chartjs-adapter-luxon.umd.min.js"),
  ),
  ("htmx.min.js", "text/javascript", include_bytes!("../../assets/vendor/htmx.min.js")),
];

pub fn vendor_url(file: &str) -> String {
  format!("/assets/vendor/{}?v={}", file, env!("CARGO_PKG_VERSION"))
}

pub async fn vendor_file(Path(file): Path<String>) -> Response {
  match VENDOR.iter().find(|(name, _, _)| *name == file) {
    Some((_, mime, body)) => (
      [(header::CONTENT_TYPE, *mime), (header::CACHE_CONTROL, "public,max-age=31536000,immutable")],
      *body,
    )
      .into_response(),
    None => StatusCode::NOT_FOUND.into_response(),
  }
}

#[cfg(test)]
mod tests {
  use axum::body::to_bytes;

  use super::*;

  #[tokio::test]
  async fn serves_embedded_vendor_files() {
    for (name, mime, body) in VENDOR {
      let response = vendor_file(Path(name.to_string())).await;
      assert_eq!(response.status(), StatusCode::OK);
      assert_eq!(response.headers()[header::CONTENT_TYPE], *mime);
      assert_eq!(response.headers()[header::CACHE_CONTROL], "public,max-age=31536000,immutable");
      assert_eq!(to_bytes(response.into_body(), usize::MAX).await.unwrap(), *body);
    }
  }

  #[tokio::test]
  async fn returns_404_for_unknown_vendor_file() {
    let response = vendor_file(Path("../Cargo.toml".to_string())).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
  }
}
