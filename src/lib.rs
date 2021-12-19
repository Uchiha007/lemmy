#![recursion_limit = "512"]
pub mod api_routes;
pub mod code_migrations;
pub mod root_span_builder;
pub mod scheduled_tasks;

use console_subscriber::ConsoleLayer;
use lemmy_utils::LemmyError;
use tracing::subscriber::set_global_default;
use tracing_error::ErrorLayer;
use tracing_log::LogTracer;
use tracing_subscriber::{filter::Targets, layer::SubscriberExt, Layer, Registry};

pub fn init_tracing() -> Result<(), LemmyError> {
  LogTracer::init()?;

  let log_description = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into());

  let targets = log_description
    .trim()
    .trim_matches('"')
    .parse::<Targets>()?;

  let format_layer = tracing_subscriber::fmt::layer().with_filter(targets);

  let console_layer = ConsoleLayer::builder()
    .with_default_env()
    .server_addr(([0, 0, 0, 0], 6669))
    .event_buffer_capacity(1024 * 1024)
    .spawn();

  let subscriber = Registry::default()
    .with(format_layer)
    .with(ErrorLayer::default())
    .with(console_layer);

  set_global_default(subscriber)?;

  Ok(())
}
