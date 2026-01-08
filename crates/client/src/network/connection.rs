//! Per-server connection handling
//!
//! Manages connection to a single server with protocol handshake,
//! request/response correlation, heartbeat, and automatic reconnection.

use anyhow::{Context, Result, anyhow};
use common::ALPN_PROTOCOL;
use iroh::{Endpoint, EndpointAddr, PublicKey as EndpointId};
use protocol::{
    CURRENT_VERSION, DeviceHandle, DeviceId, DeviceInfo, DeviceRemovalReason, Message,
    MessagePayload, RequestId, UsbRequest, UsbResponse, decode_framed, encode_framed,
    validate_version,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, RwLock, broadcast};
use tokio::task::JoinHandle;
use tokio::time::{Instant, interval_at, sleep};
use tracing::{debug, error, info, warn};

use super::health::{
    HEARTBEAT_INTERVAL, HEARTBEAT_TIMEOUT, HealthMetrics, HealthMonitor, create_health_monitor,
};

/// Device notification received from server via push
#[derive(Debug, Clone)]
pub enum DeviceNotification {
    /// Device was connected on server
    DeviceArrived { device: DeviceInfo },
    /// Device was removed from server
    DeviceRemoved {
        device_id: DeviceId,
        invalidated_handles: Vec<DeviceHandle>,
        reason: DeviceRemovalReason,
    },
    /// Device status/capability changed
    DeviceStatusChanged {
        device_id: DeviceId,
        device_info: Option<DeviceInfo>,
        reason: protocol::DeviceStatusChangeReason,
    },
    /// Interrupt data received (for proactive HID streaming)
    InterruptData {
        handle: DeviceHandle,
        endpoint: u8,
        sequence: u64,
        data: Vec<u8>,
        timestamp_us: u64,
        /// CRC32C checksum for integrity verification
        checksum: u32,
    },
}

/// Connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ConnectionState {
    /// Disconnected
    Disconnected,
    /// Connecting to server
    Connecting,
    /// Connected and operational
    Connected,
    /// Connection lost, attempting to reconnect
    Reconnecting,
    /// Permanently closed
    Closed,
}

/// Per-server connection
pub struct ServerConnection {
    /// Server EndpointId
    server_id: EndpointId,
    /// Server address (for reconnection)
    server_addr: Option<EndpointAddr>,
    /// Iroh endpoint
    endpoint: Endpoint,
    /// Current connection state
    state: Arc<RwLock<ConnectionState>>,
    /// QUIC connection (when connected)
    connection: Arc<Mutex<Option<iroh::endpoint::Connection>>>,
    /// Request ID counter
    next_request_id: Arc<AtomicU64>,
    /// Shutdown flag
    shutdown: Arc<AtomicBool>,
    /// Broadcast channel for device notifications
    notification_tx: broadcast::Sender<DeviceNotification>,
    /// Connection health monitor
    health_monitor: Arc<HealthMonitor>,
}

impl ServerConnection {
    /// Create a new server connection
    ///
    /// Establishes connection to the server and performs protocol handshake.
    pub async fn new(
        endpoint: Endpoint,
        server_id: EndpointId,
        server_addr: Option<EndpointAddr>,
    ) -> Result<Self> {
        let state = Arc::new(RwLock::new(ConnectionState::Connecting));
        let connection = Arc::new(Mutex::new(None));
        let next_request_id = Arc::new(AtomicU64::new(1));
        let shutdown = Arc::new(AtomicBool::new(false));
        let (notification_tx, _) = broadcast::channel(64);
        let health_monitor = create_health_monitor();

        let conn = Self {
            server_id,
            server_addr,
            endpoint,
            state: state.clone(),
            connection: connection.clone(),
            next_request_id,
            shutdown: shutdown.clone(),
            notification_tx: notification_tx.clone(),
            health_monitor: health_monitor.clone(),
        };

        // Establish initial connection
        conn.connect().await?;

        // Spawn heartbeat task with health monitoring
        let conn_clone = Self {
            server_id: conn.server_id,
            server_addr: conn.server_addr.clone(),
            endpoint: conn.endpoint.clone(),
            state: state.clone(),
            connection: connection.clone(),
            next_request_id: conn.next_request_id.clone(),
            shutdown: shutdown.clone(),
            notification_tx: notification_tx.clone(),
            health_monitor: health_monitor.clone(),
        };
        tokio::spawn(async move {
            conn_clone.heartbeat_loop().await;
        });

        // Spawn notification listener task
        Self::spawn_notification_listener(connection.clone(), notification_tx, shutdown);

        Ok(conn)
    }

    /// Establish connection to server
    async fn connect(&self) -> Result<()> {
        info!("Establishing connection to server: {}", self.server_id);

        *self.state.write().await = ConnectionState::Connecting;

        // Connect to server
        let conn = if let Some(ref addr) = self.server_addr {
            self.endpoint
                .connect(addr.clone(), ALPN_PROTOCOL)
                .await
                .context("Failed to connect to server")?
        } else {
            self.endpoint
                .connect(self.server_id, ALPN_PROTOCOL)
                .await
                .context("Failed to connect to server")?
        };

        info!("Connected to server: {}", self.server_id);

        // Store connection
        *self.connection.lock().await = Some(conn);
        *self.state.write().await = ConnectionState::Connected;

        // Warm up the QUIC connection by opening a stream and completing a round-trip
        // This ensures the connection is fully established before USB operations begin
        // Without this, first USB transfer may timeout waiting for QUIC stream establishment
        self.warm_up_connection().await?;

        Ok(())
    }

    /// Warm up the QUIC connection by completing a round-trip request
    ///
    /// This is critical for USB/IP: the kernel has a 5-second timeout for USB transfers,
    /// but QUIC connection establishment can take 10+ seconds on first stream open.
    /// By warming up the connection with ClientCapabilities exchange, we ensure
    /// subsequent USB transfers complete within the kernel's timeout.
    ///
    /// Note: The server expects ClientCapabilities as the first message, so we use
    /// that for warm-up rather than Ping.
    async fn warm_up_connection(&self) -> Result<()> {
        info!("Warming up QUIC connection...");
        let start = Instant::now();

        // Use ClientCapabilities for warm-up since server expects it first
        let message = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::ClientCapabilities {
                supports_push_notifications: true,
            },
        };

        // Allow generous timeout for warm-up (30 seconds) since this is a one-time cost
        let response = tokio::time::timeout(Duration::from_secs(30), self.send_message(message))
            .await
            .context("Connection warm-up timed out (30s)")?
            .context("Failed to warm up connection")?;

        match response.payload {
            MessagePayload::ServerCapabilities {
                will_send_notifications,
            } => {
                let elapsed = start.elapsed();
                info!(
                    "Connection warm-up complete in {:?} - server will push notifications: {}",
                    elapsed, will_send_notifications
                );
                // Record this as the first RTT measurement
                if elapsed.as_millis() > 1000 {
                    warn!(
                        "Slow connection detected: {:?} warm-up time. USB transfers may be affected.",
                        elapsed
                    );
                }
                Ok(())
            }
            MessagePayload::Error { message } => {
                Err(anyhow!("Server error during warm-up: {}", message))
            }
            _ => Err(anyhow!("Unexpected response during warm-up")),
        }
    }

    /// Reconnect with exponential backoff
    async fn reconnect(&self) -> Result<()> {
        let mut backoff_ms = 1000; // Start at 1 second
        let max_backoff_ms = 30_000; // Max 30 seconds

        *self.state.write().await = ConnectionState::Reconnecting;

        loop {
            if self.shutdown.load(Ordering::Relaxed) {
                return Err(anyhow!("Connection closed during reconnect"));
            }

            info!(
                "Attempting reconnection to {} (backoff: {}ms)",
                self.server_id, backoff_ms
            );

            match self.connect().await {
                Ok(()) => {
                    info!("Reconnected to server: {}", self.server_id);
                    return Ok(());
                }
                Err(e) => {
                    warn!("Reconnection failed: {}. Retrying in {}ms", e, backoff_ms);
                    sleep(Duration::from_millis(backoff_ms)).await;

                    // Exponential backoff
                    backoff_ms = (backoff_ms * 2).min(max_backoff_ms);
                }
            }
        }
    }

    /// Get current connection state
    #[allow(dead_code)]
    pub async fn state(&self) -> ConnectionState {
        *self.state.read().await
    }

    /// Check if connected
    pub async fn is_connected(&self) -> bool {
        *self.state.read().await == ConnectionState::Connected
    }

    /// Subscribe to device notifications from this server
    pub fn subscribe_notifications(&self) -> broadcast::Receiver<DeviceNotification> {
        self.notification_tx.subscribe()
    }

    /// Spawn a task to listen for push notifications via unidirectional streams
    #[allow(dead_code)]
    fn spawn_notification_listener(
        connection: Arc<Mutex<Option<iroh::endpoint::Connection>>>,
        notification_tx: broadcast::Sender<DeviceNotification>,
        shutdown: Arc<AtomicBool>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                if shutdown.load(Ordering::Relaxed) {
                    debug!("Notification listener shutting down");
                    break;
                }

                // Get connection if available
                let conn = {
                    let guard = connection.lock().await;
                    guard.clone()
                };

                let Some(conn) = conn else {
                    // Not connected, wait and retry
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                };

                // Accept unidirectional stream with timeout
                match tokio::time::timeout(Duration::from_secs(1), conn.accept_uni()).await {
                    Ok(Ok(mut recv)) => {
                        // Read notification message
                        match protocol::read_framed_async(&mut recv).await {
                            Ok(bytes) => match decode_framed(&bytes) {
                                Ok(message) => {
                                    Self::handle_notification(message.payload, &notification_tx);
                                }
                                Err(e) => {
                                    warn!("Failed to decode notification: {}", e);
                                }
                            },
                            Err(e) => {
                                debug!("Failed to read notification: {}", e);
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        // Connection error - may be closing
                        debug!("Uni stream accept error: {}", e);
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    Err(_) => {
                        // Timeout - normal, just continue
                    }
                }
            }
        })
    }

    fn handle_notification(payload: MessagePayload, tx: &broadcast::Sender<DeviceNotification>) {
        match payload {
            MessagePayload::DeviceArrivedNotification { device } => {
                info!("Received device arrived notification: {:?}", device.id);
                let _ = tx.send(DeviceNotification::DeviceArrived { device });
            }
            MessagePayload::DeviceRemovedNotification {
                device_id,
                invalidated_handles,
                reason,
            } => {
                info!("Received device removed notification: {:?}", device_id);
                let _ = tx.send(DeviceNotification::DeviceRemoved {
                    device_id,
                    invalidated_handles,
                    reason,
                });
            }
            MessagePayload::DeviceStatusChangedNotification {
                device_id,
                device_info,
                reason,
            } => {
                info!(
                    "Received device status changed notification: {:?} ({:?})",
                    device_id, reason
                );
                let _ = tx.send(DeviceNotification::DeviceStatusChanged {
                    device_id,
                    device_info,
                    reason,
                });
            }
            MessagePayload::AggregatedNotifications { notifications } => {
                info!(
                    "Received aggregated notification batch with {} items",
                    notifications.len()
                );
                for notification in notifications {
                    match notification {
                        protocol::AggregatedNotification::Arrived(device) => {
                            let _ = tx.send(DeviceNotification::DeviceArrived { device });
                        }
                        protocol::AggregatedNotification::Removed {
                            device_id,
                            invalidated_handles,
                            reason,
                        } => {
                            let _ = tx.send(DeviceNotification::DeviceRemoved {
                                device_id,
                                invalidated_handles,
                                reason,
                            });
                        }
                        protocol::AggregatedNotification::StatusChanged {
                            device_id,
                            device_info,
                            reason,
                        } => {
                            let _ = tx.send(DeviceNotification::DeviceStatusChanged {
                                device_id,
                                device_info,
                                reason,
                            });
                        }
                    }
                }
            }
            MessagePayload::InterruptData {
                handle,
                endpoint,
                sequence,
                data,
                timestamp_us,
                checksum,
            } => {
                // High-frequency HID data - use trace level to avoid log spam
                tracing::trace!(
                    "Received interrupt data: handle={}, ep=0x{:02x}, seq={}, len={}, crc=0x{:08x}",
                    handle.0, endpoint, sequence, data.len(), checksum
                );
                let _ = tx.send(DeviceNotification::InterruptData {
                    handle,
                    endpoint,
                    sequence,
                    data,
                    timestamp_us,
                    checksum,
                });
            }
            _ => {
                warn!("Unexpected notification payload: {:?}", payload);
            }
        }
    }

    /// Send client capabilities and receive server capabilities
    ///
    /// This should be called after establishing the connection to negotiate
    /// push notification support with the server.
    pub async fn send_client_capabilities(&self) -> Result<bool> {
        let message = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::ClientCapabilities {
                supports_push_notifications: true,
            },
        };

        let response = self.send_message(message).await?;

        match response.payload {
            MessagePayload::ServerCapabilities {
                will_send_notifications,
            } => {
                debug!(
                    "Server capabilities received: will_send_notifications={}",
                    will_send_notifications
                );
                Ok(will_send_notifications)
            }
            MessagePayload::Error { message } => Err(anyhow!("Server error: {}", message)),
            _ => Err(anyhow!("Unexpected response to ClientCapabilities")),
        }
    }

    /// Heartbeat loop - sends Heartbeat every 5 seconds for health monitoring
    async fn heartbeat_loop(&self) {
        // Delay first tick to allow capability exchange to complete first
        let mut ticker = interval_at(Instant::now() + HEARTBEAT_INTERVAL, HEARTBEAT_INTERVAL);

        loop {
            ticker.tick().await;

            if self.shutdown.load(Ordering::Relaxed) {
                debug!("Heartbeat loop shutting down");
                break;
            }

            // Only ping if connected
            if !self.is_connected().await {
                continue;
            }

            // Send heartbeat with health monitoring
            match self.send_heartbeat().await {
                Ok(rtt_ms) => {
                    if let Some(rtt) = rtt_ms {
                        debug!("Heartbeat successful, RTT: {}ms", rtt);
                    }
                }
                Err(e) => {
                    warn!("Heartbeat failed: {}. Recording failure.", e);
                    self.health_monitor.record_failure().await;

                    // Check if we've timed out
                    if self.health_monitor.is_timed_out().await {
                        warn!("Connection timed out, triggering reconnect");
                        // Reset health monitor for reconnection attempt
                        self.health_monitor.reset().await;

                        // Trigger reconnection
                        if let Err(e) = self.reconnect().await {
                            error!("Reconnection failed: {}", e);
                            break;
                        }
                    }
                }
            }
        }
    }

    /// Send a heartbeat message with RTT measurement
    async fn send_heartbeat(&self) -> Result<Option<u64>> {
        // Prepare heartbeat with sequence and timestamp
        let (sequence, timestamp_ms) = self.health_monitor.prepare_heartbeat().await;

        let message = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::Heartbeat {
                sequence,
                timestamp_ms,
            },
        };

        // Send with timeout
        let response = tokio::time::timeout(HEARTBEAT_TIMEOUT, self.send_message(message))
            .await
            .context("Heartbeat timeout")??;

        match response.payload {
            MessagePayload::HeartbeatAck {
                sequence: ack_seq,
                client_timestamp_ms,
                server_timestamp_ms,
            } => {
                if ack_seq == sequence {
                    let rtt = self
                        .health_monitor
                        .process_heartbeat_ack(ack_seq, client_timestamp_ms, server_timestamp_ms)
                        .await;
                    Ok(rtt)
                } else {
                    warn!(
                        "Heartbeat sequence mismatch: expected {}, got {}",
                        sequence, ack_seq
                    );
                    Ok(None)
                }
            }
            // Fallback for servers that don't support Heartbeat yet
            MessagePayload::Pong => {
                debug!("Server responded with Pong (legacy), health monitoring limited");
                Ok(None)
            }
            MessagePayload::Error { message } => Err(anyhow!("Server error: {}", message)),
            _ => Err(anyhow!("Unexpected response to Heartbeat")),
        }
    }

    /// Send a legacy ping message (for compatibility)
    #[allow(dead_code)]
    async fn send_ping(&self) -> Result<()> {
        let message = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::Ping,
        };

        let response = self.send_message(message).await?;

        match response.payload {
            MessagePayload::Pong => Ok(()),
            MessagePayload::Error { message } => Err(anyhow!("Server error: {}", message)),
            _ => Err(anyhow!("Unexpected response to Ping")),
        }
    }

    /// Get current health metrics for this connection
    pub async fn health_metrics(&self) -> HealthMetrics {
        self.health_monitor.get_metrics().await
    }

    /// Get the health monitor for this connection
    pub fn health_monitor(&self) -> Arc<HealthMonitor> {
        self.health_monitor.clone()
    }

    /// Send a message and wait for response
    async fn send_message(&self, message: Message) -> Result<Message> {
        let connection = self.connection.lock().await;
        let conn = connection
            .as_ref()
            .ok_or_else(|| anyhow!("Not connected"))?;

        // Open bidirectional stream
        let (mut send, mut recv) = conn.open_bi().await.context("Failed to open QUIC stream")?;

        // Encode and send message
        let encoded = encode_framed(&message).context("Failed to encode message")?;

        protocol::write_framed_async(&mut send, &encoded)
            .await
            .context("Failed to write message")?;
        send.finish().context("Failed to finish stream")?;

        // Read response
        let response_bytes = protocol::read_framed_async(&mut recv)
            .await
            .context("Failed to read response")?;

        // Decode response
        let response = decode_framed(&response_bytes).context("Failed to decode response")?;

        // Validate version
        validate_version(&response.version).context("Incompatible protocol version")?;

        Ok(response)
    }

    /// List devices on the server
    pub async fn list_devices(&self) -> Result<Vec<DeviceInfo>> {
        let message = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::ListDevicesRequest,
        };

        let response = self.send_message(message).await?;

        match response.payload {
            MessagePayload::ListDevicesResponse { devices } => Ok(devices),
            MessagePayload::Error { message } => Err(anyhow!("Server error: {}", message)),
            _ => Err(anyhow!("Unexpected response to ListDevicesRequest")),
        }
    }

    /// Attach to a device
    pub async fn attach_device(&self, device_id: DeviceId) -> Result<DeviceHandle> {
        let message = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::AttachDeviceRequest { device_id },
        };

        let response = self.send_message(message).await?;

        match response.payload {
            MessagePayload::AttachDeviceResponse { result } => {
                result.map_err(|e| anyhow!("Attach failed: {:?}", e))
            }
            MessagePayload::Error { message } => Err(anyhow!("Server error: {}", message)),
            _ => Err(anyhow!("Unexpected response to AttachDeviceRequest")),
        }
    }

    /// Detach from a device
    pub async fn detach_device(&self, handle: DeviceHandle) -> Result<()> {
        let message = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::DetachDeviceRequest { handle },
        };

        let response = self.send_message(message).await?;

        match response.payload {
            MessagePayload::DetachDeviceResponse { result } => {
                result.map_err(|e| anyhow!("Detach failed: {:?}", e))
            }
            MessagePayload::Error { message } => Err(anyhow!("Server error: {}", message)),
            _ => Err(anyhow!("Unexpected response to DetachDeviceRequest")),
        }
    }

    /// Submit a USB transfer
    pub async fn submit_transfer(&self, request: UsbRequest) -> Result<UsbResponse> {
        let message = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::SubmitTransfer { request },
        };

        let response = self.send_message(message).await?;

        match response.payload {
            MessagePayload::TransferComplete { response } => Ok(response),
            MessagePayload::Error { message } => Err(anyhow!("Server error: {}", message)),
            _ => Err(anyhow!("Unexpected response to SubmitTransfer")),
        }
    }

    /// Generate next request ID
    #[allow(dead_code)]
    pub fn next_request_id(&self) -> RequestId {
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        RequestId(id)
    }

    /// Close the connection
    pub async fn close(self) -> Result<()> {
        info!("Closing connection to server: {}", self.server_id);

        // Set shutdown flag
        self.shutdown.store(true, Ordering::Relaxed);

        // Update state
        *self.state.write().await = ConnectionState::Closed;

        // Close QUIC connection
        let mut connection = self.connection.lock().await;
        if let Some(conn) = connection.take() {
            conn.close(0u32.into(), b"client shutdown");
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_state() {
        let state = ConnectionState::Disconnected;
        assert_eq!(state, ConnectionState::Disconnected);

        let connected = ConnectionState::Connected;
        assert_eq!(connected, ConnectionState::Connected);
        assert_ne!(connected, state);
    }

    #[test]
    fn test_request_id_generation() {
        let counter = Arc::new(AtomicU64::new(1));
        let id1 = counter.fetch_add(1, Ordering::Relaxed);
        let id2 = counter.fetch_add(1, Ordering::Relaxed);

        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_ne!(id1, id2);
    }
}
