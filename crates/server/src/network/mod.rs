//! Network subsystem
//!
//! Handles Iroh P2P networking, client connections, and QUIC stream multiplexing.
//!
//! This module implements Phase 3 of the rust-p2p-usb project:
//! - Iroh P2P server with NAT traversal
//! - NodeId-based authentication and allowlists
//! - Per-client connection handling
//! - Protocol message routing to USB subsystem
//! - Keep-alive (ping/pong) for connection health
//!
//! # Architecture
//!
//! ```text
//! IrohServer
//!   ├─> accept connections
//!   ├─> validate allowlist
//!   └─> spawn ClientConnection per client
//!         ├─> handle QUIC streams (request/response)
//!         ├─> route to USB subsystem via UsbBridge
//!         ├─> track device attachments
//!         └─> cleanup on disconnect
//! ```

pub mod connection;
pub mod notification_aggregator;
pub mod server;

// Re-export public types
pub use notification_aggregator::{NotificationAggregator, PendingNotification};
pub use server::IrohServer;
