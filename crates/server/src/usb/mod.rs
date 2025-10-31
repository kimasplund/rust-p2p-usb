//! USB subsystem
//!
//! Manages USB device enumeration, hot-plug detection, and transfer handling.
//!
//! This module implements the USB subsystem for the server, handling:
//! - Device enumeration and discovery
//! - Hot-plug detection
//! - USB transfer execution (control, bulk, interrupt)
//! - Device lifecycle management
//!
//! The USB subsystem runs in a dedicated thread (worker) to avoid blocking
//! the Tokio async runtime, following the architecture design pattern for
//! hybrid sync-async USB operations.

// Allow dead_code and unused_imports for Phase 2 - modules not yet integrated with server main
#![allow(dead_code)]
#![allow(unused_imports)]

pub mod device;
pub mod manager;
pub mod transfers;
pub mod worker;

// Re-export public types
pub use device::UsbDevice;
pub use manager::DeviceManager;
pub use worker::{UsbWorkerThread, spawn_usb_worker};
