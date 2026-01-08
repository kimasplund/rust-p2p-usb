//! Iroh network client
//!
//! Connects to remote servers and manages connections using Iroh P2P networking.

use anyhow::{Context, Result, anyhow};
use common::{ALPN_PROTOCOL, load_or_generate_secret_key};
use iroh::{Endpoint, EndpointAddr, PublicKey as EndpointId};
use protocol::{DeviceId, DeviceInfo};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock, broadcast};
use tokio::time::{Duration, sleep};
use tracing::{debug, error, info, warn};

use super::connection::{DeviceNotification, ServerConnection};
use super::device_proxy::DeviceProxy;

/// Type alias for reconciliation callback
///
/// Called after successful reconnection with server devices and server ID.
/// The callback should compare with local state and clean up stale devices.
pub type ReconciliationCallback = Arc<
    dyn Fn(
            EndpointId,
            Vec<DeviceInfo>,
        ) -> Pin<Box<dyn Future<Output = Result<ReconciliationResult>> + Send>>
        + Send
        + Sync,
>;

/// Result of a reconciliation operation after reconnection
#[derive(Debug, Clone, Default)]
pub struct ReconciliationResult {
    /// Number of stale devices that were detached
    pub detached_count: usize,
    /// Number of devices that were re-attached
    pub reattached_count: usize,
    /// Device IDs that failed to reconcile
    pub failed_device_ids: Vec<DeviceId>,
}

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

    /// Check if maximum retries exceeded
    fn max_retries_exceeded(&self, attempt: u32) -> bool {
        self.max_retries.is_some_and(|max| attempt > max)
    }
}

/// Iroh P2P client for connecting to USB servers
///
/// Manages connections to multiple servers with allowlist enforcement
/// and automatic reconnection logic.
#[derive(Clone)]
pub struct IrohClient {
    /// Iroh network endpoint
    endpoint: Endpoint,
    /// Allowed server EndpointIds (empty = allow all)
    allowed_servers: Arc<RwLock<HashSet<EndpointId>>>,
    /// Active server connections
    connections: Arc<Mutex<HashMap<EndpointId, ServerConnection>>>,
    /// Connection state updates
    state_updates: broadcast::Sender<(EndpointId, ConnectionState)>,
    /// Device notification updates (aggregated from all servers)
    notification_updates: broadcast::Sender<(EndpointId, DeviceNotification)>,
    /// Target servers we want to maintain connections to
    target_servers: Arc<RwLock<HashSet<EndpointId>>>,
    /// Optional callback for reconciliation after reconnection
    reconciliation_callback: Arc<RwLock<Option<ReconciliationCallback>>>,
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
        let (notification_updates, _) = broadcast::channel(128); // Larger buffer for notifications
        let target_servers = Arc::new(RwLock::new(HashSet::new()));
        let reconciliation_callback = Arc::new(RwLock::new(None));

        let client = Self {
            endpoint,
            allowed_servers: Arc::new(RwLock::new(config.allowed_servers)),
            connections: Arc::new(Mutex::new(HashMap::new())),
            state_updates,
            notification_updates,
            target_servers,
            reconciliation_callback,
        };

        // Start background connection monitor
        client.start_monitor();

        Ok(client)
    }

    /// Start the background connection monitor
    fn start_monitor(&self) {
        let client = self.clone();
        let connections = self.connections.clone();
        let target_servers = self.target_servers.clone();
        let state_updates = self.state_updates.clone();
        let reconciliation_callback = self.reconciliation_callback.clone();

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

                        // Check if we've exceeded max retries
                        if policy.max_retries_exceeded(attempt_count) {
                            warn!(
                                "Max retries ({}) exceeded for {}, giving up reconnection",
                                policy.max_retries.unwrap_or(0), server_id
                            );
                            let _ = state_updates.send((*server_id, ConnectionState::Disconnected));
                            continue;
                        }

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

                        match client.setup_connection(*server_id, None).await {
                            Ok(conn) => {
                                info!("Reconnected to {}!", server_id);

                                // Perform reconciliation after successful reconnection
                                Self::perform_reconciliation(
                                    &conn,
                                    *server_id,
                                    &reconciliation_callback,
                                )
                                .await;

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

    /// Subscribe to all device notifications from all servers
    pub fn subscribe_all_notifications(
        &self,
    ) -> broadcast::Receiver<(EndpointId, DeviceNotification)> {
        self.notification_updates.subscribe()
    }

    /// Set a callback to be called after successful reconnection for reconciliation
    ///
    /// The callback receives the server EndpointId and the current list of devices
    /// on that server. It should compare with local state and clean up any stale
    /// devices that no longer exist on the server.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let virtual_usb = Arc::new(VirtualUsbManager::new().await?);
    /// let virtual_usb_clone = virtual_usb.clone();
    ///
    /// client.set_reconciliation_callback(Arc::new(move |server_id, server_devices| {
    ///     let virtual_usb = virtual_usb_clone.clone();
    ///     Box::pin(async move {
    ///         // Compare server_devices with locally attached devices
    ///         // and detach any that no longer exist on server
    ///         reconcile_devices(server_id, server_devices, &virtual_usb).await
    ///     })
    /// })).await;
    /// ```
    pub async fn set_reconciliation_callback(&self, callback: ReconciliationCallback) {
        let mut guard = self.reconciliation_callback.write().await;
        *guard = Some(callback);
        info!("Reconciliation callback registered");
    }

    /// Clear the reconciliation callback
    #[allow(dead_code)]
    pub async fn clear_reconciliation_callback(&self) {
        let mut guard = self.reconciliation_callback.write().await;
        *guard = None;
        debug!("Reconciliation callback cleared");
    }

    /// Perform reconciliation after successful reconnection
    ///
    /// Fetches the current device list from the server and invokes the
    /// reconciliation callback if one is registered.
    async fn perform_reconciliation(
        conn: &ServerConnection,
        server_id: EndpointId,
        callback: &Arc<RwLock<Option<ReconciliationCallback>>>,
    ) {
        // Check if callback is registered
        let callback_opt = {
            let guard = callback.read().await;
            guard.clone()
        };

        let Some(callback_fn) = callback_opt else {
            debug!(
                "No reconciliation callback registered, skipping reconciliation for {}",
                server_id
            );
            return;
        };

        info!("Starting reconciliation for server {}", server_id);

        // Fetch current device list from server
        match conn.list_devices().await {
            Ok(server_devices) => {
                debug!(
                    "Server {} has {} devices available",
                    server_id,
                    server_devices.len()
                );

                // Invoke the reconciliation callback
                match callback_fn(server_id, server_devices).await {
                    Ok(result) => {
                        info!(
                            "Reconciliation complete for {}: detached={}, reattached={}, failed={}",
                            server_id,
                            result.detached_count,
                            result.reattached_count,
                            result.failed_device_ids.len()
                        );

                        if !result.failed_device_ids.is_empty() {
                            warn!(
                                "Failed to reconcile devices: {:?}",
                                result.failed_device_ids
                            );
                        }
                    }
                    Err(e) => {
                        error!("Reconciliation callback failed for {}: {}", server_id, e);
                    }
                }
            }
            Err(e) => {
                warn!(
                    "Failed to fetch device list from {} for reconciliation: {}",
                    server_id, e
                );
            }
        }
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

        // Create connection and wire up notifications
        let connection = self.setup_connection(server_id, server_addr).await?;

        // Successfully connected
        info!("Successfully connected to server: {}", server_id);

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

    /// Get health metrics for a server connection
    ///
    /// Returns the current health metrics including RTT, packet loss,
    /// connection quality, and health state. Returns None if not connected.
    pub async fn get_health_metrics(
        &self,
        server_id: EndpointId,
    ) -> Option<super::health::HealthMetrics> {
        let connections = self.connections.lock().await;
        if let Some(conn) = connections.get(&server_id) {
            Some(conn.health_metrics().await)
        } else {
            None
        }
    }

    /// Get health metrics for all connected servers
    ///
    /// Returns a map of server EndpointId to health metrics.
    pub async fn get_all_health_metrics(
        &self,
    ) -> std::collections::HashMap<EndpointId, super::health::HealthMetrics> {
        let connections = self.connections.lock().await;
        let mut metrics = std::collections::HashMap::new();
        for (server_id, conn) in connections.iter() {
            metrics.insert(*server_id, conn.health_metrics().await);
        }
        metrics
    }

    /// Check if a server connection is healthy
    ///
    /// Returns true if connected and not in disconnected state.
    /// Returns false if not connected or connection has timed out.
    pub async fn is_server_healthy(&self, server_id: EndpointId) -> bool {
        let connections = self.connections.lock().await;
        if let Some(conn) = connections.get(&server_id) {
            conn.health_monitor().is_healthy().await
        } else {
            false
        }
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

    /// Internal helper to create a connection and wire up notifications
    async fn setup_connection(
        &self,
        server_id: EndpointId,
        server_addr: Option<EndpointAddr>,
    ) -> Result<ServerConnection> {
        // ServerConnection::new() includes connection warm-up which does
        // the capability exchange. No need to call send_client_capabilities() again.
        let connection =
            ServerConnection::new(self.endpoint.clone(), server_id, server_addr).await?;

        // Setup notification forwarding
        let notification_tx_agg = self.notification_updates.clone();
        let mut notification_rx = connection.subscribe_notifications();

        tokio::spawn(async move {
            while let Ok(notification) = notification_rx.recv().await {
                if notification_tx_agg.send((server_id, notification)).is_err() {
                    break;
                }
            }
            debug!("Notification forwarder for {} stopped", server_id);
        });

        Ok(connection)
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
    async fn test_allowlist_management() {
        use common::test_utils::generate_test_endpoint_id;

        let config = ClientConfig::default();
        let client = IrohClient::new(config).await.unwrap();

        let server_id = generate_test_endpoint_id();

        client.add_allowed_server(server_id).await;
        assert!(client.is_server_allowed(&server_id).await);

        client.remove_allowed_server(server_id).await;
        assert!(client.is_server_allowed(&server_id).await);
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
