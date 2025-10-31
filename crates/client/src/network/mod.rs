//! Network subsystem
//!
//! Handles Iroh P2P networking and communication with server.

pub mod client;
pub mod connection;
pub mod device_proxy;
pub mod session;
pub mod streams;

// Re-export public types
pub use client::{ClientConfig, IrohClient};
