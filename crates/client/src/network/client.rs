//! Iroh network client
//!
//! Connects to remote servers and manages connections using Iroh P2P networking.

use anyhow::{Context, Result, anyhow};
use common::ALPN_PROTOCOL;
use iroh::{Endpoint, EndpointAddr, PublicKey as EndpointId};
use protocol::{DeviceId, DeviceInfo};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::{error, info, warn};

use super::connection::ServerConnection;
use super::device_proxy::DeviceProxy;

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
    connections: Arc<Mutex<HashMap<EndpointId, ServerConnection>>>,
}

/// Client configuration
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Allowed server EndpointIds (empty = allow all)
    pub allowed_servers: HashSet<EndpointId>,
    /// ALPN protocol identifier
    pub alpn: Vec<u8>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            allowed_servers: HashSet::new(),
            alpn: ALPN_PROTOCOL.to_vec(),
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

        // Create Iroh endpoint
        let endpoint = Endpoint::builder()
            .alpns(vec![config.alpn.clone()])
            .bind()
            .await
            .context("Failed to create Iroh endpoint")?;

        let endpoint_id = endpoint.id();
        info!("Client EndpointId: {}", endpoint_id);

        Ok(Self {
            endpoint,
            allowed_servers: Arc::new(RwLock::new(config.allowed_servers)),
            connections: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Get the client's EndpointId
    pub fn endpoint_id(&self) -> EndpointId {
        self.endpoint.id()
    }

    /// Add a server to the allowlist
    pub async fn add_allowed_server(&self, server_id: EndpointId) {
        let mut allowlist = self.allowed_servers.write().await;
        allowlist.insert(server_id);
        info!("Added server to allowlist: {}", server_id);
    }

    /// Remove a server from the allowlist
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

    /// Shutdown the client gracefully
    ///
    /// Closes all connections and shuts down the Iroh endpoint.
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
}
