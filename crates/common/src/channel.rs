//! Async channel bridge between Tokio runtime and USB thread

use async_channel::{Receiver, Sender, bounded};

/// Commands from Tokio runtime to USB thread
#[derive(Debug)]
pub enum UsbCommand {
    /// List all connected USB devices
    ListDevices {
        /// Channel to send response back
        response: tokio::sync::oneshot::Sender<Vec<protocol::DeviceInfo>>,
    },

    /// Attach a device for client access
    AttachDevice {
        /// Device ID to attach
        device_id: protocol::DeviceId,
        /// Client NodeId (for permission tracking)
        client_id: String,
        /// Channel to send response back
        response:
            tokio::sync::oneshot::Sender<Result<protocol::DeviceHandle, protocol::AttachError>>,
    },

    /// Detach a device
    DetachDevice {
        /// Device handle to detach
        handle: protocol::DeviceHandle,
        /// Channel to send response back
        response: tokio::sync::oneshot::Sender<Result<(), protocol::DetachError>>,
    },

    /// Submit a USB transfer
    SubmitTransfer {
        /// Device handle for transfer
        handle: protocol::DeviceHandle,
        /// Transfer request
        request: protocol::UsbRequest,
        /// Channel to send response back
        response: tokio::sync::oneshot::Sender<protocol::UsbResponse>,
    },

    /// Reset a USB device
    ResetDevice {
        /// Device handle to reset
        handle: protocol::DeviceHandle,
        /// Channel to send response back
        response: tokio::sync::oneshot::Sender<Result<(), protocol::UsbError>>,
    },

    /// Get sharing status for a device
    GetSharingStatus {
        /// Device ID to query
        device_id: protocol::DeviceId,
        /// Optional handle for client-specific status
        handle: Option<protocol::DeviceHandle>,
        /// Channel to send response back
        response: tokio::sync::oneshot::Sender<
            Result<protocol::DeviceSharingStatus, protocol::AttachError>,
        >,
    },

    /// Acquire a lock on a device
    LockDevice {
        /// Device handle
        handle: protocol::DeviceHandle,
        /// Whether to request write access (for read-only mode)
        write_access: bool,
        /// Channel to send response back
        response: tokio::sync::oneshot::Sender<protocol::LockResult>,
    },

    /// Release a lock on a device
    UnlockDevice {
        /// Device handle
        handle: protocol::DeviceHandle,
        /// Channel to send response back
        response: tokio::sync::oneshot::Sender<protocol::UnlockResult>,
    },

    /// Shutdown the USB thread gracefully
    Shutdown,
}

/// USB events from the device manager
#[derive(Debug, Clone)]
pub enum UsbEvent {
    /// Device hot-plugged (connected)
    DeviceArrived {
        /// Full device information
        device: protocol::DeviceInfo,
    },

    /// Device removed with affected handles
    DeviceLeft {
        /// ID of the removed device
        device_id: protocol::DeviceId,
        /// Handles that were invalidated
        invalidated_handles: Vec<protocol::DeviceHandle>,
        /// Client IDs that need to be notified
        affected_clients: Vec<String>,
    },

    /// Device became available for a queued client
    DeviceAvailable {
        /// Device ID
        device_id: protocol::DeviceId,
        /// Handle that now has access
        handle: protocol::DeviceHandle,
        /// Client ID to notify
        client_id: String,
        /// Current sharing mode
        sharing_mode: protocol::SharingMode,
    },

    /// Queue position changed for a client
    QueuePositionChanged {
        /// Device ID
        device_id: protocol::DeviceId,
        /// Handle affected
        handle: protocol::DeviceHandle,
        /// Client ID to notify
        client_id: String,
        /// New queue position (0 = has access)
        new_position: u32,
    },

    /// Lock expired for a client
    LockExpired {
        /// Device ID
        device_id: protocol::DeviceId,
        /// Handle that lost the lock
        handle: protocol::DeviceHandle,
        /// Client ID to notify
        client_id: String,
    },
}

/// Handle for Tokio runtime (async)
#[derive(Clone)]
pub struct UsbBridge {
    cmd_tx: Sender<UsbCommand>,
    event_rx: Receiver<UsbEvent>,
}

impl UsbBridge {
    /// Send a command to the USB thread
    pub async fn send_command(&self, cmd: UsbCommand) -> crate::Result<()> {
        self.cmd_tx
            .send(cmd)
            .await
            .map_err(|e| crate::Error::Channel(e.to_string()))
    }

    /// Receive an event from the USB thread
    pub async fn recv_event(&self) -> crate::Result<UsbEvent> {
        self.event_rx
            .recv()
            .await
            .map_err(|e| crate::Error::Channel(e.to_string()))
    }
}

/// Handle for USB thread (blocking)
pub struct UsbWorker {
    pub(crate) cmd_rx: Receiver<UsbCommand>,
    /// Event sender (public for USB worker thread to access)
    pub event_tx: Sender<UsbEvent>,
}

impl UsbWorker {
    /// Receive a command from Tokio runtime (blocking)
    pub fn recv_command(&self) -> crate::Result<UsbCommand> {
        self.cmd_rx
            .recv_blocking()
            .map_err(|e| crate::Error::Channel(e.to_string()))
    }

    /// Try to receive a command without blocking
    pub fn try_recv_command(&self) -> Option<UsbCommand> {
        self.cmd_rx.try_recv().ok()
    }

    /// Send an event to Tokio runtime (blocking)
    pub fn send_event(&self, event: UsbEvent) -> crate::Result<()> {
        self.event_tx
            .send_blocking(event)
            .map_err(|e| crate::Error::Channel(e.to_string()))
    }
}

/// Create the channel bridge between Tokio and USB thread
///
/// Returns (UsbBridge for Tokio, UsbWorker for USB thread)
pub fn create_usb_bridge() -> (UsbBridge, UsbWorker) {
    let (cmd_tx, cmd_rx) = bounded(256);
    let (event_tx, event_rx) = bounded(256);

    (
        UsbBridge { cmd_tx, event_rx },
        UsbWorker { cmd_rx, event_tx },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_channel_bridge() {
        let (bridge, worker) = create_usb_bridge();

        // Spawn a thread to simulate USB worker
        let handle = std::thread::spawn(move || {
            let cmd = worker.recv_command().unwrap();
            matches!(cmd, UsbCommand::ListDevices { .. })
        });

        // Send command from async context
        let (tx, _rx) = tokio::sync::oneshot::channel();
        bridge
            .send_command(UsbCommand::ListDevices { response: tx })
            .await
            .unwrap();

        assert!(handle.join().unwrap());
    }
}
