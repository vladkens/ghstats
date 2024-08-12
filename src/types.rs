// https://github.com/tokio-rs/axum/blob/main/examples/anyhow-error-response/src/main.rs

pub type Res<T = ()> = anyhow::Result<T>;
pub type HtmlRes = Result<maud::Markup, AppError>;

pub struct AppError(anyhow::Error);

impl axum::response::IntoResponse for AppError {
  fn into_response(self) -> axum::response::Response {
    let code = axum::http::StatusCode::INTERNAL_SERVER_ERROR;
    (code, format!("Something went wrong: {}", self.0)).into_response()
  }
}

impl<E: Into<anyhow::Error>> From<E> for AppError {
  fn from(err: E) -> Self {
    Self(err.into())
  }
}
