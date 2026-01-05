//! Common utilities for rust-p2p-usb
//!
//! This crate provides shared functionality between the server and client,
//! including Iroh networking extensions, USB type abstractions, error handling,
//! secret key persistence, and the async channel bridge for USB thread communication.

pub mod alpn;
pub mod channel;
pub mod error;
pub mod iroh_ext;
pub mod keys;
pub mod logging;
pub mod test_utils;
pub mod usb_types;

pub use alpn::ALPN_PROTOCOL;
pub use channel::{UsbBridge, UsbCommand, UsbEvent, UsbWorker, create_usb_bridge};
pub use error::{Error, Result};
pub use keys::{default_secret_key_path, load_or_generate_secret_key};
pub use logging::setup_logging;
