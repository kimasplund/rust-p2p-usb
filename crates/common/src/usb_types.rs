//! USB type abstractions and utilities

// TODO: Implement USB type abstractions
// This module will contain shared USB-related types and utilities
// used by both server and client.

/// Placeholder for USB device abstraction
#[derive(Debug, Clone)]
pub struct UsbDevice {
    pub vendor_id: u16,
    pub product_id: u16,
    pub description: String,
}
