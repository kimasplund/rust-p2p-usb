//! Protocol message definitions
//!
//! This module defines all message types used in the USB-over-QUIC protocol.
//! Messages are organized into logical groups:
//! - Discovery (list devices)
//! - Device management (attach/detach)
//! - USB transfers (submit/complete)
//! - Connection management (ping/pong, errors)

use crate::types::{
    AttachError, DetachError, DeviceHandle, DeviceId, DeviceInfo, UsbRequest, UsbResponse,
};
use crate::version::ProtocolVersion;
use serde::{Deserialize, Serialize};

/// Top-level message envelope
///
/// All protocol messages are wrapped in this envelope which includes
/// the protocol version for compatibility checking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Protocol version of this message
    pub version: ProtocolVersion,
    /// Message payload
    pub payload: MessagePayload,
}

/// All message types in the protocol
///
/// This enum defines every possible message that can be exchanged between
/// client and server. Messages are typically request-response pairs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessagePayload {
    // Discovery phase
    /// Request list of available USB devices from server
    ListDevicesRequest,

    /// Response containing list of available USB devices
    ListDevicesResponse {
        /// List of devices available on the server
        devices: Vec<DeviceInfo>,
    },

    // Device attachment
    /// Request to attach to a specific device
    AttachDeviceRequest {
        /// ID of the device to attach to
        device_id: DeviceId,
    },

    /// Response to attach request
    AttachDeviceResponse {
        /// Handle if successful, error if failed
        result: Result<DeviceHandle, AttachError>,
    },

    /// Request to detach from a device
    DetachDeviceRequest {
        /// Handle of the device to detach from
        handle: DeviceHandle,
    },

    /// Response to detach request
    DetachDeviceResponse {
        /// Success (unit) or error
        result: Result<(), DetachError>,
    },

    // USB transfers
    /// Submit a USB transfer request to the server
    SubmitTransfer {
        /// USB transfer request details
        request: UsbRequest,
    },

    /// USB transfer completion from server
    TransferComplete {
        /// USB transfer response with data or error
        response: UsbResponse,
    },

    // Connection management
    /// Ping message for keep-alive
    Ping,

    /// Pong response to ping
    Pong,

    /// Protocol-level error message
    Error {
        /// Human-readable error message
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::version::CURRENT_VERSION;

    #[test]
    fn test_message_construction() {
        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::Ping,
        };

        assert_eq!(msg.version, CURRENT_VERSION);
    }
}
