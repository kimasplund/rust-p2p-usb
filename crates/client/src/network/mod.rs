//! Network subsystem
//!
//! Handles Iroh P2P networking and communication with server.

pub mod client;
pub mod connection;
pub mod device_proxy;
pub mod health;
pub mod session;

// Re-export public types
pub use client::{
    ClientConfig, ConnectionState, IrohClient, ReconciliationCallback, ReconciliationResult,
};
pub use connection::DeviceNotification;
pub use health::{
    ConnectionQuality, HealthMetrics,
    HealthState,
};
