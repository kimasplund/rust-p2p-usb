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

impl DeviceSpeed {
    /// Check if this is a SuperSpeed device (USB 3.0+)
    pub fn is_superspeed(&self) -> bool {
        matches!(self, DeviceSpeed::Super | DeviceSpeed::SuperPlus)
    }

    /// Get maximum bulk transfer buffer size for this speed
    ///
    /// Returns the recommended maximum buffer size for bulk transfers
    /// based on the device speed class. These values match SuperSpeedConfig::for_speed().
    pub fn max_bulk_transfer_size(&self) -> usize {
        match self {
            DeviceSpeed::Low | DeviceSpeed::Full => 4 * 1024, // 4KB for USB 1.x
            DeviceSpeed::High => 64 * 1024, // 64KB for USB 2.0
            DeviceSpeed::Super | DeviceSpeed::SuperPlus => 1024 * 1024, // 1MB for USB 3.0+
        }
    }

    /// Get optimal chunk size (max packet size) for this speed
    ///
    /// Returns the maximum packet size for USB transfers at this speed.
    pub fn optimal_chunk_size(&self) -> usize {
        match self {
            DeviceSpeed::Low => 8,
            DeviceSpeed::Full => 64,
            DeviceSpeed::High => 512,
            DeviceSpeed::Super | DeviceSpeed::SuperPlus => 1024,
        }
    }
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
    /// Isochronous transfer
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
        /// Packet descriptors for the transfer
        iso_packet_descriptors: Vec<IsoPacketDescriptor>,
        /// Start frame for scheduling
        start_frame: u32,
        /// Interval between packets
        interval: u32,
        /// Timeout in milliseconds
        timeout_ms: u32,
    },
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
    /// Isochronous transfer succeeded
    IsochronousSuccess {
        /// Isochronous packet results
        iso_packet_descriptors: Vec<IsoPacketDescriptor>,
        /// Start frame number
        start_frame: u32,
        /// Number of packets with errors
        error_count: u32,
        /// Total data received/sent
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
}

/// Result of a single isochronous packet
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IsoPacketResult {
    /// Actual length of data transferred
    pub actual_length: u32,
    /// Status of this packet (0 = success)
    pub status: i32,
}

/// Isochronous packet descriptor for scheduling
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IsoPacketDescriptor {
    /// Offset in the data buffer
    pub offset: u32,
    /// Length of this packet
    pub length: u32,
    /// Actual length transferred (filled in result)
    pub actual_length: u32,
    /// Status of this packet (0 = success)
    pub status: i32,
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

impl From<AttachError> for UsbError {
    fn from(err: AttachError) -> Self {
        match err {
            AttachError::DeviceNotFound => UsbError::NotFound,
            AttachError::AlreadyAttached => UsbError::Busy,
            AttachError::PermissionDenied => UsbError::Access,
            AttachError::PolicyDenied { reason } => UsbError::Other { message: reason },
            AttachError::OutsideTimeWindow {
                current_time,
                allowed_windows,
            } => UsbError::Other {
                message: format!(
                    "Outside allowed time window ({}, allowed: {})",
                    current_time,
                    allowed_windows.join(", ")
                ),
            },
            AttachError::DeviceClassRestricted { device_class } => UsbError::Other {
                message: format!("Device class {:02x} restricted", device_class),
            },
            AttachError::Other { message } => UsbError::Other { message },
        }
    }
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
    /// Policy denied access with reason
    PolicyDenied {
        /// Human-readable reason for denial
        reason: String,
    },
    /// Access denied outside allowed time window
    OutsideTimeWindow {
        /// Current time formatted as HH:MM
        current_time: String,
        /// Allowed time windows (e.g., ["09:00-17:00"])
        allowed_windows: Vec<String>,
    },
    /// Device class not allowed for this client
    DeviceClassRestricted {
        /// USB device class code that was restricted
        device_class: u8,
    },
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

/// Device sharing mode
///
/// Determines how multiple clients can access a device simultaneously.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum SharingMode {
    /// Only one client can attach at a time (default, safest)
    #[default]
    Exclusive,
    /// Multiple clients can attach; access is arbitrated (round-robin or queued)
    Shared,
    /// Multiple clients can read, but only one can write
    ReadOnly,
}

impl std::fmt::Display for SharingMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SharingMode::Exclusive => write!(f, "exclusive"),
            SharingMode::Shared => write!(f, "shared"),
            SharingMode::ReadOnly => write!(f, "read-only"),
        }
    }
}

/// Reason for forced device detachment
///
/// Indicates why a device session was forcibly ended by the server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ForceDetachReason {
    /// Session duration limit reached (policy enforcement)
    SessionDurationLimitReached {
        /// How long the session lasted
        duration_secs: u64,
        /// Maximum allowed duration
        max_duration_secs: u64,
    },
    /// Access time window expired (policy enforcement)
    TimeWindowExpired {
        /// Current time when access ended
        current_time: String,
        /// Next allowed time window (if any)
        next_window: Option<String>,
    },
    /// Administrative action (operator forced detach)
    AdminAction {
        /// Optional reason provided by admin
        reason: Option<String>,
    },
    /// Server is shutting down
    ServerShutdown,
    /// Device was physically disconnected
    DeviceDisconnected,
}

/// Serializable latency statistics for protocol exchange
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProtocolLatencyStats {
    pub min_us: u64,
    pub max_us: u64,
    pub avg_us: u64,
    pub sample_count: usize,
}

impl ProtocolLatencyStats {
    pub fn format_min(&self) -> String {
        format!("{:.2} ms", self.min_us as f64 / 1000.0)
    }
    pub fn format_max(&self) -> String {
        format!("{:.2} ms", self.max_us as f64 / 1000.0)
    }
    pub fn format_avg(&self) -> String {
        format!("{:.2} ms", self.avg_us as f64 / 1000.0)
    }
}

/// Serializable metrics snapshot for protocol exchange
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProtocolMetrics {
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub transfers_completed: u64,
    pub transfers_failed: u64,
    pub retries: u64,
    pub active_transfers: u64,
    pub latency: ProtocolLatencyStats,
    pub throughput_tx_bps: f64,
    pub throughput_rx_bps: f64,
    pub loss_rate: f64,
    pub retry_rate: f64,
    pub uptime_secs: Option<u64>,
}

impl ProtocolMetrics {
    pub fn format_throughput_tx(&self) -> String {
        format_bps(self.throughput_tx_bps)
    }
    pub fn format_throughput_rx(&self) -> String {
        format_bps(self.throughput_rx_bps)
    }
    pub fn format_bytes_sent(&self) -> String {
        format_size(self.bytes_sent)
    }
    pub fn format_bytes_received(&self) -> String {
        format_size(self.bytes_received)
    }
    pub fn format_loss_rate(&self) -> String {
        format!("{:.1}%", self.loss_rate * 100.0)
    }
    pub fn format_uptime(&self) -> String {
        match self.uptime_secs {
            Some(s) => {
                let h = s / 3600;
                let m = (s % 3600) / 60;
                let sec = s % 60;
                if h > 0 { format!("{}h {}m {}s", h, m, sec) }
                else if m > 0 { format!("{}m {}s", m, sec) }
                else { format!("{}s", sec) }
            }
            None => "N/A".to_string(),
        }
    }
    pub fn connection_quality(&self) -> u8 {
        let mut q: f64 = 100.0;
        let lat_ms = self.latency.avg_us as f64 / 1000.0;
        if lat_ms > 100.0 { q -= 30.0; }
        else if lat_ms > 50.0 { q -= 20.0; }
        else if lat_ms > 20.0 { q -= 10.0; }
        if self.loss_rate > 0.1 { q -= 30.0; }
        else if self.loss_rate > 0.05 { q -= 20.0; }
        else if self.loss_rate > 0.01 { q -= 10.0; }
        if self.retry_rate > 0.3 { q -= 20.0; }
        else if self.retry_rate > 0.1 { q -= 10.0; }
        q.max(0.0) as u8
    }
    pub fn connection_quality_label(&self) -> &'static str {
        match self.connection_quality() {
            90..=100 => "Excellent",
            70..=89 => "Good",
            50..=69 => "Fair",
            30..=49 => "Poor",
            _ => "Critical",
        }
    }
}

/// Per-device metrics from server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceMetrics {
    pub device_id: DeviceId,
    pub metrics: ProtocolMetrics,
}

/// Per-client metrics from server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientMetrics {
    pub client_id: String,
    pub metrics: ProtocolMetrics,
}

/// Server metrics summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerMetricsSummary {
    pub total: ProtocolMetrics,
    pub devices: Vec<DeviceMetrics>,
    pub clients: Vec<ClientMetrics>,
}

fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if bytes >= GB { format!("{:.2} GB", bytes as f64 / GB as f64) }
    else if bytes >= MB { format!("{:.2} MB", bytes as f64 / MB as f64) }
    else if bytes >= KB { format!("{:.2} KB", bytes as f64 / KB as f64) }
    else { format!("{} B", bytes) }
}

fn format_bps(bps: f64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    if bps >= GB { format!("{:.2} GB/s", bps / GB) }
    else if bps >= MB { format!("{:.2} MB/s", bps / MB) }
    else if bps >= KB { format!("{:.2} KB/s", bps / KB) }
    else { format!("{:.0} B/s", bps) }
}

/// SuperSpeed configuration for USB 3.0+ devices
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuperSpeedConfig {
    /// Maximum burst size (1-16 packets)
    pub max_burst: u8,
    /// Maximum streams for bulk endpoints (0 = not supported)
    pub max_streams: u16,
    /// Bytes per interval for isochronous
    pub bytes_per_interval: u32,
    /// Maximum bulk transfer size
    pub max_bulk_size: usize,
    /// URB buffer size for transfers
    pub urb_buffer_size: usize,
    /// Whether burst mode is enabled
    pub enable_burst: bool,
}

impl Default for SuperSpeedConfig {
    fn default() -> Self {
        Self {
            max_burst: 1,
            max_streams: 0,
            bytes_per_interval: 0,
            max_bulk_size: 4 * 1024, // 4KB default (USB 1.x)
            urb_buffer_size: 4 * 1024,
            enable_burst: false,
        }
    }
}

impl SuperSpeedConfig {
    /// Create configuration for a device speed
    pub fn for_speed(speed: DeviceSpeed) -> Self {
        match speed {
            DeviceSpeed::Super | DeviceSpeed::SuperPlus => Self {
                max_burst: 16,
                max_streams: 32,
                bytes_per_interval: 196608,
                max_bulk_size: 1024 * 1024,  // 1MB for USB 3.0+
                urb_buffer_size: 256 * 1024, // 256KB
                enable_burst: true,
            },
            DeviceSpeed::High => Self {
                max_burst: 1,
                max_streams: 0,
                bytes_per_interval: 3072,
                max_bulk_size: 64 * 1024,  // 64KB for USB 2.0
                urb_buffer_size: 64 * 1024,
                enable_burst: false,
            },
            DeviceSpeed::Low | DeviceSpeed::Full => Self {
                max_burst: 1,
                max_streams: 0,
                bytes_per_interval: 0,
                max_bulk_size: 4 * 1024,  // 4KB for USB 1.x
                urb_buffer_size: 4 * 1024,
                enable_burst: false,
            },
        }
    }
}

/// Reason for device status change
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DeviceStatusChangeReason {
    /// Device was reset
    DeviceReset,
    /// Sharing status changed
    SharingStatusChanged { shared: bool },
    /// Device configuration changed
    ConfigurationChanged,
    /// Device capabilities updated
    CapabilitiesUpdated,
    /// Interface claimed/released
    InterfaceChange,
    /// Power state changed
    PowerStateChange,
    /// Other reason
    Other { description: String },
}

/// Single notification within an aggregated batch
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AggregatedNotification {
    /// Device arrived
    Arrived(DeviceInfo),
    /// Device removed
    Removed {
        device_id: DeviceId,
        invalidated_handles: Vec<DeviceHandle>,
        reason: DeviceRemovalReason,
    },
    /// Device status changed
    StatusChanged {
        device_id: DeviceId,
        device_info: Option<DeviceInfo>,
        reason: DeviceStatusChangeReason,
    },
}

/// Queue position update notification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuePositionUpdate {
    /// Device ID
    pub device_id: DeviceId,
    /// New queue position (0 = has lock)
    pub position: u32,
    /// Total queue length
    pub queue_length: u32,
}

/// Device sharing status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceSharingStatus {
    /// Device ID
    pub device_id: DeviceId,
    /// Current sharing mode
    pub sharing_mode: SharingMode,
    /// Number of attached clients
    pub attached_clients: u32,
    /// Whether current client has write lock
    pub has_write_lock: bool,
    /// Queue position (0 = has access)
    pub queue_position: u32,
    /// Total queue length
    pub queue_length: u32,
}

/// Result of a lock request
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum LockResult {
    /// Lock acquired
    Acquired,
    /// Lock already held by this client
    AlreadyHeld,
    /// Queued at position
    Queued { position: u32 },
    /// Lock not available
    NotAvailable { reason: String },
}

/// Result of an unlock request
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum UnlockResult {
    /// Unlock succeeded
    Released,
    /// Was not holding lock
    NotHeld,
    /// Error during unlock
    Error { message: String },
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
