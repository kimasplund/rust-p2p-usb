//! Protocol message definitions
//!
//! This module defines all message types used in the USB-over-QUIC protocol.
//! Messages are organized into logical groups:
//! - Discovery (list devices)
//! - Device management (attach/detach)
//! - USB transfers (submit/complete)
//! - Connection management (ping/pong, errors)

use crate::types::{
    AggregatedNotification, AttachError, DetachError, DeviceHandle, DeviceId, DeviceInfo,
    DeviceRemovalReason, DeviceSharingStatus, DeviceStatusChangeReason, ForceDetachReason,
    LockResult, ProtocolMetrics, QueuePositionUpdate, ServerMetricsSummary, SharingMode,
    UnlockResult, UsbRequest, UsbResponse,
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
    /// Ping message for keep-alive (legacy, use Heartbeat for health monitoring)
    Ping,

    /// Pong response to ping (legacy, use HeartbeatAck for health monitoring)
    Pong,

    /// Heartbeat message for connection health monitoring
    /// Includes timestamp and sequence for RTT measurement
    Heartbeat {
        /// Sequence number for matching responses
        sequence: u64,
        /// Client timestamp in milliseconds since epoch (for RTT calculation)
        timestamp_ms: u64,
    },

    /// Heartbeat acknowledgment from server
    HeartbeatAck {
        /// Echoed sequence number
        sequence: u64,
        /// Echoed client timestamp for RTT calculation
        client_timestamp_ms: u64,
        /// Server timestamp in milliseconds since epoch
        server_timestamp_ms: u64,
    },

    /// Protocol-level error message
    Error {
        /// Human-readable error message
        message: String,
    },

    // Hotplug notifications (server -> client, via unidirectional stream)
    /// Device was hot-plugged (connected) on server
    DeviceArrivedNotification {
        /// Full device information
        device: DeviceInfo,
    },

    /// Device was hot-unplugged (disconnected) on server
    DeviceRemovedNotification {
        /// ID of the removed device
        device_id: DeviceId,
        /// Handles that are now invalid
        invalidated_handles: Vec<DeviceHandle>,
        /// Reason for removal
        reason: DeviceRemovalReason,
    },

    // Capability negotiation (bidirectional, on connection)
    /// Client announces supported features
    ClientCapabilities {
        /// Client supports push notifications
        supports_push_notifications: bool,
    },

    /// Server announces supported features
    ServerCapabilities {
        /// Server will send push notifications
        will_send_notifications: bool,
    },

    // Policy enforcement notifications (server -> client)
    /// Notification that a device session is about to be forcibly ended (server -> client)
    ///
    /// Sent before the server forces a detach due to policy constraints.
    /// Gives the client time to clean up gracefully before forced disconnect.
    ForceDetachWarning {
        /// Device handle being detached
        handle: DeviceHandle,
        /// Device ID
        device_id: DeviceId,
        /// Reason for forced detach
        reason: ForceDetachReason,
        /// Seconds until forced detach (0 = immediate)
        seconds_until_detach: u32,
    },

    /// Notification that a device session was forcibly ended (server -> client)
    ///
    /// Sent after the server has forcibly detached a client from a device.
    ForcedDetachNotification {
        /// Device handle that was detached
        handle: DeviceHandle,
        /// Device ID
        device_id: DeviceId,
        /// Reason for forced detach
        reason: ForceDetachReason,
    },

    /// Device capability/status changed notification (server -> client)
    DeviceStatusChangedNotification {
        /// Device ID that changed
        device_id: DeviceId,
        /// Updated device info (if still available)
        device_info: Option<DeviceInfo>,
        /// Reason for the status change
        reason: DeviceStatusChangeReason,
    },

    /// Aggregated notification batch (server -> client)
    AggregatedNotifications {
        /// List of notifications in this batch
        notifications: Vec<AggregatedNotification>,
    },

    // Metrics exchange
    /// Request server metrics (client -> server)
    GetMetricsRequest,

    /// Response with server metrics (server -> client)
    GetMetricsResponse {
        /// Server metrics summary
        metrics: ServerMetricsSummary,
    },

    /// Client metrics update (client -> server)
    ClientMetricsUpdate {
        /// Client's current metrics
        metrics: ProtocolMetrics,
    },

    // Device sharing and lock management
    /// Request sharing status for a device
    GetSharingStatusRequest {
        /// Device ID to query
        device_id: DeviceId,
    },

    /// Response with device sharing status
    GetSharingStatusResponse {
        /// Sharing status if device exists
        result: Result<DeviceSharingStatus, AttachError>,
    },

    /// Request to acquire lock on a device (for Shared/ReadOnly modes)
    LockDeviceRequest {
        /// Device handle (must be attached first)
        handle: DeviceHandle,
        /// Whether to request write access (only for ReadOnly mode)
        write_access: bool,
        /// Timeout in seconds (0 = no timeout)
        timeout_secs: u32,
    },

    /// Response to lock request
    LockDeviceResponse {
        /// Lock result
        result: LockResult,
    },

    /// Request to release lock on a device
    UnlockDeviceRequest {
        /// Device handle
        handle: DeviceHandle,
    },

    /// Response to unlock request
    UnlockDeviceResponse {
        /// Unlock result
        result: UnlockResult,
    },

    /// Notification when queue position changes (server -> client)
    QueuePositionNotification {
        /// Queue position update details
        update: QueuePositionUpdate,
    },

    /// Notification when device becomes available (server -> client)
    DeviceAvailableNotification {
        /// Device ID that became available
        device_id: DeviceId,
        /// Device handle (for attached clients)
        handle: DeviceHandle,
        /// Current sharing mode
        sharing_mode: SharingMode,
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
