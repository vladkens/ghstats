use tokio::signal;
use tracing::{dispatcher, Dispatch, Level};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{EnvFilter, Registry};

pub fn init_logger() {
  let logfmt = tracing_logfmt::builder()
    .with_target(false)
    .with_span_name(false)
    .with_span_path(false)
    .with_ansi_color(true);

  let subscriber = Registry::default()
    .with(EnvFilter::builder().with_default_directive(Level::INFO.into()).from_env_lossy())
    .with(logfmt.layer());

  dispatcher::set_global_default(Dispatch::new(subscriber)).expect("failed to set global logger");
}

// https://github.com/tokio-rs/axum/discussions/1894
pub async fn shutdown_signal() {
  let ctrl_c = async {
    signal::ctrl_c().await.expect("failed to install Ctrl+C handler");
  };

  let terminate = async {
    signal::unix::signal(signal::unix::SignalKind::terminate())
      .expect("failed to install signal handler")
      .recv()
      .await;
  };

  tokio::select! {
      _ = ctrl_c => {},
      _ = terminate => {},
  }
}
