//! Per-server connection handling
//!
//! Manages connection to a single server with protocol handshake,
//! request/response correlation, heartbeat, and automatic reconnection.

use anyhow::{Context, Result, anyhow};
use common::ALPN_PROTOCOL;
use iroh::{Endpoint, EndpointAddr, PublicKey as EndpointId};
use protocol::{
    CURRENT_VERSION, DeviceHandle, DeviceId, DeviceInfo, Message, MessagePayload, RequestId,
    UsbRequest, UsbResponse, decode_framed, encode_framed, validate_version,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};
use tokio::time::{interval, sleep};
use tracing::{debug, error, info, warn};

/// Connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

        let conn = Self {
            server_id,
            server_addr,
            endpoint,
            state: state.clone(),
            connection: connection.clone(),
            next_request_id,
            shutdown: shutdown.clone(),
        };

        // Establish initial connection
        conn.connect().await?;

        // Spawn heartbeat task
        let conn_clone = Self {
            server_id: conn.server_id,
            server_addr: conn.server_addr.clone(),
            endpoint: conn.endpoint.clone(),
            state: state.clone(),
            connection: connection.clone(),
            next_request_id: conn.next_request_id.clone(),
            shutdown: shutdown.clone(),
        };
        tokio::spawn(async move {
            conn_clone.heartbeat_loop().await;
        });

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

        Ok(())
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
    pub async fn state(&self) -> ConnectionState {
        *self.state.read().await
    }

    /// Check if connected
    pub async fn is_connected(&self) -> bool {
        *self.state.read().await == ConnectionState::Connected
    }

    /// Heartbeat loop - sends Ping every 30 seconds
    async fn heartbeat_loop(&self) {
        let mut ticker = interval(Duration::from_secs(30));

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

            // Send ping
            match self.send_ping().await {
                Ok(()) => {
                    debug!("Heartbeat ping successful");
                }
                Err(e) => {
                    warn!("Heartbeat ping failed: {}. Triggering reconnect", e);

                    // Trigger reconnection
                    if let Err(e) = self.reconnect().await {
                        error!("Reconnection failed: {}", e);
                        break;
                    }
                }
            }
        }
    }

    /// Send a ping message
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
