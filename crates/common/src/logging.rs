//! Logging setup and configuration

use tracing_subscriber::{EnvFilter, fmt, prelude::*};

/// Setup tracing subscriber for the application
pub fn setup_logging(default_level: &str) -> crate::Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(default_level))
        .map_err(|e| crate::Error::Config(format!("Invalid log filter: {}", e)))?;

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer())
        .init();

    Ok(())
}
