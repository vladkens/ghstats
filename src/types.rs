// https://github.com/tokio-rs/axum/blob/main/examples/anyhow-error-response/src/main.rs

pub type Res<T = ()> = anyhow::Result<T>;
pub type JsonRes<T> = Result<axum::Json<T>, AppError>;
pub type HtmlRes = Result<maud::Markup, AppError>;

pub struct AppError(anyhow::Error);

impl AppError {
  pub fn not_found() -> HtmlRes {
    Err(Self(anyhow::anyhow!(axum::http::StatusCode::NOT_FOUND)))
  }
}

impl axum::response::IntoResponse for AppError {
  fn into_response(self) -> axum::response::Response {
    match self.0.downcast_ref::<axum::http::StatusCode>() {
      Some(code) => (*code, self.0.to_string()).into_response(),
      None => {
        let code = axum::http::StatusCode::INTERNAL_SERVER_ERROR;
        (code, format!("Something went wrong: {}", self.0)).into_response()
      }
    }
  }
}

impl<E: Into<anyhow::Error>> From<E> for AppError {
  fn from(err: E) -> Self {
    Self(err.into())
  }
}
