//! Iroh network client
//!
//! Connects to remote servers and manages connections using Iroh P2P networking.

use anyhow::{Context, Result, anyhow};
use common::{ALPN_PROTOCOL, load_or_generate_secret_key};
use iroh::{Endpoint, EndpointAddr, PublicKey as EndpointId};
use protocol::{DeviceId, DeviceInfo};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock, broadcast};
use tokio::time::{Duration, sleep};
use tracing::{debug, error, info, warn};

use super::connection::{DeviceNotification, ServerConnection};
use super::device_proxy::DeviceProxy;

/// Connection state for a server
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    /// Connected to server
    Connected,
    /// Disconnected from server
    Disconnected,
    /// Attempting to reconnect (attempt #, next retry in)
    Reconnecting(u32, Duration),
}

/// Reconnection policy with exponential backoff
#[derive(Debug, Clone)]
struct ReconnectionPolicy {
    /// Initial retry delay
    initial_delay: Duration,
    /// Maximum retry delay
    max_delay: Duration,
    /// Backoff multiplier
    multiplier: f64,
    /// Maximum retries (None = infinite)
    max_retries: Option<u32>,
}

impl Default for ReconnectionPolicy {
    fn default() -> Self {
        Self {
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            multiplier: 1.5,
            max_retries: None, // Infinite retries by default
        }
    }
}

impl ReconnectionPolicy {
    /// Calculate delay for a specific attempt number (1-based)
    fn delay_for_attempt(&self, attempt: u32) -> Duration {
        if attempt == 0 {
            return Duration::ZERO;
        }

        let delay_secs =
            self.initial_delay.as_secs_f64() * self.multiplier.powi((attempt - 1) as i32);

        let capped_delay = delay_secs.min(self.max_delay.as_secs_f64());
        Duration::from_secs_f64(capped_delay)
    }
}

/// Iroh P2P client for connecting to USB servers
///
/// Manages connections to multiple servers with allowlist enforcement
/// and automatic reconnection logic.
pub struct IrohClient {
    /// Iroh network endpoint
    endpoint: Endpoint,
    /// Allowed server EndpointIds (empty = allow all)
    allowed_servers: Arc<RwLock<HashSet<EndpointId>>>,
    /// Active server connections
    /// Active server connections
    connections: Arc<Mutex<HashMap<EndpointId, ServerConnection>>>,
    /// Connection state updates
    state_updates: broadcast::Sender<(EndpointId, ConnectionState)>,
    /// Target servers we want to maintain connections to
    target_servers: Arc<RwLock<HashSet<EndpointId>>>,
}

/// Client configuration
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Allowed server EndpointIds (empty = allow all)
    pub allowed_servers: HashSet<EndpointId>,
    /// ALPN protocol identifier
    pub alpn: Vec<u8>,
    /// Path to the secret key file for stable EndpointId
    /// If None, uses default XDG path: ~/.config/p2p-usb/secret_key
    pub secret_key_path: Option<PathBuf>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            allowed_servers: HashSet::new(),
            alpn: ALPN_PROTOCOL.to_vec(),
            secret_key_path: None,
        }
    }
}

impl IrohClient {
    /// Create a new Iroh client
    ///
    /// # Example
    /// ```no_run
    /// use client::network::client::{IrohClient, ClientConfig};
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///     let client = IrohClient::new(ClientConfig::default()).await?;
    ///     Ok(())
    /// }
    /// ```
    pub async fn new(config: ClientConfig) -> Result<Self> {
        info!(
            "Creating Iroh client with ALPN: {:?}",
            String::from_utf8_lossy(&config.alpn)
        );

        // Load or generate persistent secret key for stable EndpointId
        let secret_key = load_or_generate_secret_key(config.secret_key_path.as_deref())
            .context("Failed to load or generate secret key")?;

        // Create Iroh endpoint with persistent key
        let endpoint = Endpoint::builder()
            .secret_key(secret_key)
            .alpns(vec![config.alpn.clone()])
            .bind()
            .await
            .context("Failed to create Iroh endpoint")?;

        // Wait for endpoint to discover its addresses before accepting connections
        let _ = endpoint.online().await;

        let endpoint_id = endpoint.id();
        info!(
            "Client EndpointId: {} (stable across restarts)",
            endpoint_id
        );

        let (state_updates, _) = broadcast::channel(32);
        let target_servers = Arc::new(RwLock::new(HashSet::new()));

        let client = Self {
            endpoint,
            allowed_servers: Arc::new(RwLock::new(config.allowed_servers)),
            connections: Arc::new(Mutex::new(HashMap::new())),
            state_updates,
            target_servers,
        };

        // Start background connection monitor
        client.start_monitor();

        Ok(client)
    }

    /// Start the background connection monitor
    fn start_monitor(&self) {
        let connections = self.connections.clone();
        let target_servers = self.target_servers.clone();
        let endpoint = self.endpoint.clone();
        let state_updates = self.state_updates.clone();

        tokio::spawn(async move {
            info!("Connection monitor started");
            let mut reconnect_attempts = HashMap::new();
            let policy = ReconnectionPolicy::default();

            loop {
                // Check all target servers
                let targets = target_servers.read().await.clone();

                for server_id in &targets {
                    let is_connected = {
                        let conns = connections.lock().await;
                        conns.contains_key(server_id)
                    };

                    if !is_connected {
                        // We definitely need to reconnect
                        let attempts = reconnect_attempts.entry(*server_id).or_insert(0);
                        *attempts += 1;

                        let attempt_count = *attempts;
                        let delay = policy.delay_for_attempt(attempt_count);

                        debug!(
                            "Reconnecting to {} (attempt {}), waiting {:?}",
                            server_id, attempt_count, delay
                        );

                        // Broadcast state change
                        let _ = state_updates.send((
                            *server_id,
                            ConnectionState::Reconnecting(attempt_count, delay),
                        ));

                        sleep(delay).await;

                        info!("Attempting reconnection to {}...", server_id);

                        // Try to connect
                        match ServerConnection::new(endpoint.clone(), *server_id, None).await {
                            Ok(conn) => {
                                info!("Reconnected to {}!", server_id);
                                {
                                    let mut conns = connections.lock().await;
                                    conns.insert(*server_id, conn);
                                }
                                reconnect_attempts.remove(server_id);
                                let _ =
                                    state_updates.send((*server_id, ConnectionState::Connected));
                            }
                            Err(e) => {
                                warn!("Reconnection to {} failed: {}", server_id, e);
                                // Loop will retry next iteration
                            }
                        }
                    } else {
                        // We are connected, verify health (optional pings could go here)
                        // For now just clear attempts if we see it connected
                        reconnect_attempts.remove(server_id);
                        // We don't spam Connected state here, only on transitions ideally
                        // But since we don't track prev state in this loop easily,
                        // we assume `connect_to_server` or the successful reconnect above sent it.
                    }
                }

                // Clean up attempts for servers no longer in targets
                reconnect_attempts.retain(|k, _| targets.contains(k));

                sleep(Duration::from_secs(1)).await;
            }
        });
    }

    /// Subscribe to connection state updates
    pub fn subscribe(&self) -> broadcast::Receiver<(EndpointId, ConnectionState)> {
        self.state_updates.subscribe()
    }

    /// Get the client's EndpointId
    pub fn endpoint_id(&self) -> EndpointId {
        self.endpoint.id()
    }

    /// Add a server to the allowlist
    #[allow(dead_code)]
    pub async fn add_allowed_server(&self, server_id: EndpointId) {
        let mut allowlist = self.allowed_servers.write().await;
        allowlist.insert(server_id);
        info!("Added server to allowlist: {}", server_id);
    }

    /// Remove a server from the allowlist
    #[allow(dead_code)]
    pub async fn remove_allowed_server(&self, server_id: EndpointId) {
        let mut allowlist = self.allowed_servers.write().await;
        allowlist.remove(&server_id);
        info!("Removed server from allowlist: {}", server_id);
    }

    /// Check if a server is in the allowlist
    async fn is_server_allowed(&self, server_id: &EndpointId) -> bool {
        let allowlist = self.allowed_servers.read().await;
        allowlist.is_empty() || allowlist.contains(server_id)
    }

    /// Connect to a server
    ///
    /// Creates a connection to the specified server and stores it in the connection pool.
    /// If already connected, returns the existing connection.
    ///
    /// # Arguments
    /// * `server_id` - EndpointId of the server to connect to
    /// * `server_addr` - Optional EndpointAddr with relay information
    ///
    /// # Example
    /// ```no_run
    /// use client::network::client::{IrohClient, ClientConfig};
    /// use iroh::PublicKey as EndpointId;
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///     let client = IrohClient::new(ClientConfig::default()).await?;
    ///     let server_id = "your-server-endpoint-id".parse()?;
    ///     client.connect_to_server(server_id, None).await?;
    ///     Ok(())
    /// }
    /// ```
    pub async fn connect_to_server(
        &self,
        server_id: EndpointId,
        server_addr: Option<EndpointAddr>,
    ) -> Result<()> {
        // Check allowlist
        if !self.is_server_allowed(&server_id).await {
            warn!("Attempted connection to non-allowed server: {}", server_id);
            return Err(anyhow!("Server {} not in allowlist", server_id));
        }

        // Check if already connected
        {
            let connections = self.connections.lock().await;
            if connections.contains_key(&server_id) {
                info!("Already connected to server: {}", server_id);
                // Ensure it's in target servers
                self.target_servers.write().await.insert(server_id);
                let _ = self
                    .state_updates
                    .send((server_id, ConnectionState::Connected));
                return Ok(());
            }
        }

        info!("Connecting to server: {}", server_id);

        // Create connection
        let connection =
            ServerConnection::new(self.endpoint.clone(), server_id, server_addr).await?;

        // Store connection
        {
            let mut connections = self.connections.lock().await;
            connections.insert(server_id, connection);
        }

        // Add to target servers for auto-reconnection
        self.target_servers.write().await.insert(server_id);

        // Notify
        let _ = self
            .state_updates
            .send((server_id, ConnectionState::Connected));

        info!("Successfully connected to server: {}", server_id);
        Ok(())
    }

    /// Disconnect from a server
    ///
    /// Closes the connection to the specified server and removes it from the pool.
    pub async fn disconnect_from_server(&self, server_id: EndpointId) -> Result<()> {
        info!("Disconnecting from server: {}", server_id);

        let mut connections = self.connections.lock().await;
        if let Some(connection) = connections.remove(&server_id) {
            connection.close().await?;
            info!("Disconnected from server: {}", server_id);
            // Remove from targets so we don't auto-reconnect
            self.target_servers.write().await.remove(&server_id);
            let _ = self
                .state_updates
                .send((server_id, ConnectionState::Disconnected));
            Ok(())
        } else {
            Err(anyhow!("Not connected to server: {}", server_id))
        }
    }

    /// List remote devices from a server
    ///
    /// # Arguments
    /// * `server_id` - EndpointId of the server to query
    ///
    /// # Returns
    /// List of available USB devices on the server
    pub async fn list_remote_devices(&self, server_id: EndpointId) -> Result<Vec<DeviceInfo>> {
        let connections = self.connections.lock().await;
        let connection = connections
            .get(&server_id)
            .ok_or_else(|| anyhow!("Not connected to server: {}", server_id))?;

        connection.list_devices().await
    }

    /// Create a device proxy for a remote USB device
    ///
    /// Note: This method must be called on an Arc<IrohClient>
    ///
    /// # Arguments
    /// * `client` - Arc reference to this client (for DeviceProxy)
    /// * `server_id` - Server hosting the device
    /// * `device_info` - Device information from list_remote_devices
    ///
    /// # Returns
    /// DeviceProxy for performing USB operations
    pub async fn create_device_proxy(
        client: Arc<Self>,
        server_id: EndpointId,
        device_info: DeviceInfo,
    ) -> Result<Arc<DeviceProxy>> {
        // Verify we're connected to the server
        let connections = client.connections.lock().await;
        if !connections.contains_key(&server_id) {
            return Err(anyhow!("Not connected to server: {}", server_id));
        }
        drop(connections); // Release lock

        // Create proxy (doesn't attach yet - that's done by the caller)
        Ok(Arc::new(DeviceProxy::new(client, server_id, device_info)))
    }

    /// Attach to a remote device
    ///
    /// # Arguments
    /// * `server_id` - Server hosting the device
    /// * `device_id` - ID of the device to attach
    ///
    /// # Returns
    /// Device handle for subsequent operations
    pub async fn attach_device(
        &self,
        server_id: EndpointId,
        device_id: DeviceId,
    ) -> Result<protocol::DeviceHandle> {
        let connections = self.connections.lock().await;
        let connection = connections
            .get(&server_id)
            .ok_or_else(|| anyhow!("Not connected to server: {}", server_id))?;

        connection.attach_device(device_id).await
    }

    /// Detach from a remote device
    ///
    /// # Arguments
    /// * `server_id` - Server hosting the device
    /// * `handle` - Device handle from attach_device
    pub async fn detach_device(
        &self,
        server_id: EndpointId,
        handle: protocol::DeviceHandle,
    ) -> Result<()> {
        let connections = self.connections.lock().await;
        let connection = connections
            .get(&server_id)
            .ok_or_else(|| anyhow!("Not connected to server: {}", server_id))?;

        connection.detach_device(handle).await
    }

    /// Submit a USB transfer request
    ///
    /// # Arguments
    /// * `server_id` - Server hosting the device
    /// * `request` - USB transfer request
    ///
    /// # Returns
    /// USB transfer response
    pub async fn submit_transfer(
        &self,
        server_id: EndpointId,
        request: protocol::UsbRequest,
    ) -> Result<protocol::UsbResponse> {
        let connections = self.connections.lock().await;
        let connection = connections
            .get(&server_id)
            .ok_or_else(|| anyhow!("Not connected to server: {}", server_id))?;

        connection.submit_transfer(request).await
    }

    /// Get list of connected servers
    pub async fn connected_servers(&self) -> Vec<EndpointId> {
        let connections = self.connections.lock().await;
        connections.keys().copied().collect()
    }

    /// Subscribe to device notifications from a server
    ///
    /// Returns a broadcast receiver for device arrival/removal notifications.
    /// Returns None if not connected to the specified server.
    pub async fn subscribe_notifications(
        &self,
        server_id: EndpointId,
    ) -> Option<broadcast::Receiver<DeviceNotification>> {
        let connections = self.connections.lock().await;
        connections
            .get(&server_id)
            .map(|conn| conn.subscribe_notifications())
    }

    /// Shutdown the client gracefully
    ///
    /// Closes all connections and shuts down the Iroh endpoint.
    #[allow(dead_code)]
    pub async fn shutdown(self) -> Result<()> {
        info!("Shutting down client");

        // Close all connections
        let mut connections = self.connections.lock().await;
        for (server_id, connection) in connections.drain() {
            info!("Closing connection to {}", server_id);
            if let Err(e) = connection.close().await {
                error!("Error closing connection to {}: {}", server_id, e);
            }
        }

        // Shutdown endpoint
        self.endpoint.close().await;

        info!("Client shutdown complete");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_client_creation() {
        let config = ClientConfig::default();
        let client = IrohClient::new(config).await.unwrap();

        // Verify endpoint_id is accessible
        let _endpoint_id = client.endpoint_id();
    }

    #[tokio::test]
    #[ignore] // Requires valid NodeId creation which needs SecretKey
    async fn test_allowlist_management() {
        // TODO: Implement this test with proper NodeId generation
        // For now, allowlist functionality is tested indirectly
        // via integration tests
    }

    #[tokio::test]
    async fn test_connected_servers_empty() {
        let config = ClientConfig::default();
        let client = IrohClient::new(config).await.unwrap();

        let servers = client.connected_servers().await;
        assert!(servers.is_empty());
    }
    #[test]
    fn test_reconnection_policy_backoff() {
        use std::time::Duration;
        let policy = ReconnectionPolicy {
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(10),
            multiplier: 2.0,
            max_retries: None,
        };

        // Attempt 0: initial attempt (0s)
        assert_eq!(policy.delay_for_attempt(0), Duration::ZERO);

        // Attempt 1: 1s * 2.0^0 = 1s
        assert_eq!(policy.delay_for_attempt(1), Duration::from_secs(1));

        // Attempt 2: 1s * 2.0^1 = 2s
        assert_eq!(policy.delay_for_attempt(2), Duration::from_secs(2));

        // Attempt 3: 1s * 2.0^2 = 4s
        assert_eq!(policy.delay_for_attempt(3), Duration::from_secs(4));

        // Attempt 4: 1s * 2.0^3 = 8s
        assert_eq!(policy.delay_for_attempt(4), Duration::from_secs(8));

        // Attempt 5: 1s * 2.0^4 = 16s -> capped at max_delay (10s)
        assert_eq!(policy.delay_for_attempt(5), Duration::from_secs(10));
    }
}
