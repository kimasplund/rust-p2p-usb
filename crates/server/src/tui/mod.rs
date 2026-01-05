//! Terminal User Interface
//!
//! Provides an interactive TUI for managing USB devices and connections.
//! Built with ratatui and crossterm for cross-platform terminal rendering.
//!
//! # Layout
//!
//! The TUI consists of three main panels:
//! - **Top Panel**: Status bar showing server EndpointId, connection count, uptime
//! - **Center Panel**: USB device list with columns for ID, VID:PID, Name, Status, Clients
//! - **Bottom Panel**: Help bar with available keybindings
//!
//! # Keybindings
//!
//! - `Up/k`: Move selection up
//! - `Down/j`: Move selection down
//! - `Space`: Toggle device sharing on/off
//! - `Enter`: View device details
//! - `c`: View connected clients
//! - `r`: Refresh device list
//! - `?`: Show help
//! - `q`: Quit (closes dialog first if one is open)
//! - `Esc`: Close dialog
//!
//! # Example
//!
//! ```ignore
//! use server::tui;
//! use common::UsbBridge;
//! use tokio::sync::mpsc;
//!
//! async fn run_tui_mode(
//!     endpoint_id: iroh::PublicKey,
//!     usb_bridge: UsbBridge,
//!     network_rx: mpsc::UnboundedReceiver<tui::NetworkEvent>,
//!     auto_share: bool,
//! ) -> anyhow::Result<()> {
//!     tui::run(endpoint_id, usb_bridge, network_rx, auto_share).await
//! }
//! ```

pub mod app;
pub mod events;
pub mod ui;

// Re-export main types for external use
pub use app::{App, DeviceState, Dialog, NetworkEvent, Tui, run};
pub use events::{Action, Event, EventHandler};
