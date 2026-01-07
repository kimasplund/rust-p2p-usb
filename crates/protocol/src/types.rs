//! USB and protocol type definitions
//!
//! This module defines all the USB-related types used in the protocol,
//! including device descriptors, transfer types, and error conditions.

use serde::{Deserialize, Serialize};

/// Unique device identifier (server-assigned)
///
/// Used to identify USB devices on the server. This ID is stable for the
/// lifetime of the device connection and is used when attaching to devices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceId(pub u32);

/// Device handle (session-specific)
///
/// Returned when a client successfully attaches to a device. This handle
/// is used for all subsequent USB transfer requests. The handle is only
/// valid for the duration of the client session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceHandle(pub u32);

/// Request ID for matching responses
///
/// Each USB transfer request must have a unique request ID so that responses
/// can be matched to their requests. The client is responsible for generating
/// unique request IDs (typically using an atomic counter).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RequestId(pub u64);

/// Device information returned in discovery
///
/// Contains all relevant USB device descriptor information needed to
/// identify and select devices for attachment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// Unique server-assigned device identifier
    pub id: DeviceId,
    /// USB Vendor ID
    pub vendor_id: u16,
    /// USB Product ID
    pub product_id: u16,
    /// Bus number on the server
    pub bus_number: u8,
    /// Device address on the bus
    pub device_address: u8,
    /// Manufacturer string (if available)
    pub manufacturer: Option<String>,
    /// Product string (if available)
    pub product: Option<String>,
    /// Serial number string (if available)
    pub serial_number: Option<String>,
    /// USB device class
    pub class: u8,
    /// USB device subclass
    pub subclass: u8,
    /// USB device protocol
    pub protocol: u8,
    /// Device speed (USB 1.0, 2.0, 3.0, etc.)
    pub speed: DeviceSpeed,
    /// Number of configurations
    pub num_configurations: u8,
}

/// USB device speed
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DeviceSpeed {
    /// Low speed - 1.5 Mbps (USB 1.0)
    Low,
    /// Full speed - 12 Mbps (USB 1.1)
    Full,
    /// High speed - 480 Mbps (USB 2.0)
    High,
    /// SuperSpeed - 5 Gbps (USB 3.0)
    Super,
    /// SuperSpeed+ - 10 Gbps (USB 3.1)
    SuperPlus,
}

/// USB transfer request (client -> server)
///
/// Represents a USB transfer to be executed on the server.
/// The server will execute the transfer and return a UsbResponse.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsbRequest {
    /// Unique request ID for matching responses
    pub id: RequestId,
    /// Device handle to perform transfer on
    pub handle: DeviceHandle,
    /// Type of transfer to perform
    pub transfer: TransferType,
}

/// USB transfer types
///
/// Supports control, interrupt, bulk, and isochronous transfers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransferType {
    /// Control transfer (endpoint 0)
    ///
    /// Used for device configuration and descriptor requests.
    /// Always synchronous with 5 second timeout.
    Control {
        /// Request type byte (bmRequestType)
        request_type: u8,
        /// Request byte (bRequest)
        request: u8,
        /// Value parameter (wValue)
        value: u16,
        /// Index parameter (wIndex)
        index: u16,
        /// Data to send (OUT) or empty vec for IN transfers
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    /// Interrupt transfer
    ///
    /// Used for HID devices (keyboard, mouse) and other low-latency devices.
    Interrupt {
        /// Endpoint address (includes direction bit)
        endpoint: u8,
        /// Data to send (OUT) or empty vec for IN transfers
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
        /// Timeout in milliseconds
        timeout_ms: u32,
    },
    /// Bulk transfer
    ///
    /// Used for storage devices, network adapters, and high-throughput transfers.
    Bulk {
        /// Endpoint address (includes direction bit)
        endpoint: u8,
        /// Data to send (OUT) or empty vec for IN transfers
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
        /// Timeout in milliseconds
        timeout_ms: u32,
    },
    /*
    /// Isochronous transfer (Currently disabled due to rusb limitations)
    ///
    /// Used for audio/video streaming devices.
    Isochronous {
        /// Endpoint address (includes direction bit)
        endpoint: u8,
        /// Data to send (OUT) or empty vec for IN transfers.
        ///
        /// For IN transfers:
        /// The buffer size should be `num_packets * packet_len`.
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
        /// Length of each packet in the transfer
        packet_lengths: Vec<u32>,
        /// Timeout in milliseconds
        timeout_ms: u32,
    },
    */
}

/// USB transfer response (server -> client)
///
/// Contains the result of a USB transfer request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsbResponse {
    /// Request ID matching the original request
    pub id: RequestId,
    /// Transfer result (success or error)
    pub result: TransferResult,
}

/// USB transfer result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransferResult {
    /// Transfer succeeded
    Success {
        /// Data received (for IN transfers), empty for OUT transfers
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    /// Transfer failed with error
    Error {
        /// Error details
        error: UsbError,
    },
}

/// USB error types
///
/// Maps to libusb error codes. See rusb::Error for details.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum UsbError {
    /// Transfer timed out
    Timeout,
    /// Endpoint stalled (protocol error)
    Pipe,
    /// Device was disconnected
    NoDevice,
    /// Device or endpoint not found
    NotFound,
    /// Device is busy
    Busy,
    /// Buffer overflow
    Overflow,
    /// I/O error
    Io,
    /// Invalid parameter
    InvalidParam,
    /// Access denied (permissions)
    Access,
    /// Other error with message
    Other { message: String },
}

/// Device attach error
///
/// Returned when a client attempts to attach to a device.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AttachError {
    /// Device ID not found on server
    DeviceNotFound,
    /// Device already attached to another client
    AlreadyAttached,
    /// Client lacks permission to access device
    PermissionDenied,
    /// Other error with message
    Other { message: String },
}

/// Device detach error
///
/// Returned when a client attempts to detach from a device.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DetachError {
    /// Device handle not found (already detached or invalid)
    HandleNotFound,
    /// Other error with message
    Other { message: String },
}

/// Reason for device removal in hotplug notification
///
/// Indicates why a device was removed from the server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DeviceRemovalReason {
    /// Physical device was unplugged
    Unplugged,
    /// Server is shutting down
    ServerShutdown,
    /// Device error (USB reset failed, etc.)
    DeviceError { message: String },
    /// Administrative action (operator removed sharing)
    AdminAction,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_id_equality() {
        let id1 = DeviceId(42);
        let id2 = DeviceId(42);
        let id3 = DeviceId(43);

        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_device_speed_variants() {
        let speeds = [
            DeviceSpeed::Low,
            DeviceSpeed::Full,
            DeviceSpeed::High,
            DeviceSpeed::Super,
            DeviceSpeed::SuperPlus,
        ];

        assert_eq!(speeds.len(), 5);
    }

    #[test]
    fn test_usb_error_equality() {
        let err1 = UsbError::Timeout;
        let err2 = UsbError::Timeout;
        let err3 = UsbError::NoDevice;

        assert_eq!(err1, err2);
        assert_ne!(err1, err3);
    }

    #[test]
    fn test_device_removal_reason() {
        let reasons = [
            DeviceRemovalReason::Unplugged,
            DeviceRemovalReason::ServerShutdown,
            DeviceRemovalReason::AdminAction,
            DeviceRemovalReason::DeviceError { message: "test".to_string() },
        ];
        assert_eq!(reasons.len(), 4);
    }
}
