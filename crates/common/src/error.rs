//! Common error types

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("USB error: {0}")]
    Usb(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("Channel error: {0}")]
    Channel(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Other error: {0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;
