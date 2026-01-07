//! Iroh P2P Network Server
//!
//! Manages the Iroh endpoint, accepts client connections, enforces allowlist,
//! and spawns per-client connection handlers.

use anyhow::{Context, Result, anyhow};
use common::{ALPN_PROTOCOL, UsbBridge, load_or_generate_secret_key};
use iroh::{Endpoint, PublicKey as EndpointId};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use super::connection::ClientConnection;
use crate::audit::SharedAuditLogger;
use crate::config::ServerConfig;

/// Iroh P2P server for USB device sharing
///
/// Manages the Iroh network endpoint, accepts incoming client connections,
/// enforces EndpointId allowlists, and spawns tasks to handle each connection.
pub struct IrohServer {
    /// Iroh network endpoint
    endpoint: Endpoint,
    /// Bridge to USB subsystem
    usb_bridge: UsbBridge,
    /// Allowed client EndpointIds (empty = allow all)
    allowed_clients: Arc<RwLock<HashSet<EndpointId>>>,
    /// Server configuration
    config: ServerConfig,
    /// Audit logger
    audit_logger: SharedAuditLogger,
}

impl IrohServer {
    /// Create a new Iroh server
    ///
    /// # Arguments
    /// * `config` - Server configuration including allowlist
    /// * `usb_bridge` - Communication bridge to USB subsystem
    /// * `audit_logger` - Optional audit logger for compliance logging
    ///
    /// # Returns
    /// Configured server ready to start accepting connections
    pub async fn new(
        config: ServerConfig,
        usb_bridge: UsbBridge,
        audit_logger: SharedAuditLogger,
    ) -> Result<Self> {
        info!("Initializing Iroh P2P server...");

        // Load or generate persistent secret key for stable EndpointId
        let secret_key = load_or_generate_secret_key(config.iroh.secret_key_path.as_deref())
            .context("Failed to load or generate secret key")?;

        // Create Iroh endpoint with ALPN protocol identifier and persistent key
        let endpoint = Endpoint::builder()
            .secret_key(secret_key)
            .alpns(vec![ALPN_PROTOCOL.to_vec()])
            .bind()
            .await
            .context("Failed to create Iroh endpoint")?;

        // Wait for endpoint to discover its addresses before accepting connections
        let _ = endpoint.online().await;

        // Parse allowed clients from config
        let allowed_clients = Self::parse_allowlist(&config.security.approved_clients)?;

        let endpoint_id = endpoint.id();
        info!("Server EndpointId: {}", endpoint_id);
        info!("Server ready to accept connections");

        if config.security.require_approval {
            info!(
                "Client allowlist enabled with {} entries",
                allowed_clients.len()
            );
        } else {
            warn!("Client allowlist disabled - accepting all connections");
        }

        Ok(Self {
            endpoint,
            usb_bridge,
            allowed_clients: Arc::new(RwLock::new(allowed_clients)),
            config,
            audit_logger,
        })
    }

    /// Get the server's EndpointId
    ///
    /// This EndpointId must be shared with clients for them to connect
    pub fn endpoint_id(&self) -> EndpointId {
        self.endpoint.id()
    }

    /// Get the server's listening addresses
    pub fn local_addrs(&self) -> Vec<std::net::SocketAddr> {
        self.endpoint.bound_sockets().iter().copied().collect()
    }

    /// Start accepting client connections
    ///
    /// This runs indefinitely, spawning a new task for each accepted connection.
    /// The function only returns on shutdown or fatal error.
    pub async fn run(self) -> Result<()> {
        info!("Server running, waiting for connections...");

        loop {
            // Accept incoming connection
            let incoming = match self.endpoint.accept().await {
                Some(conn) => conn,
                None => {
                    warn!("Endpoint closed, shutting down");
                    break;
                }
            };

            // Spawn task to handle connection
            let usb_bridge = self.usb_bridge.clone();
            let allowed_clients = self.allowed_clients.clone();
            let require_approval = self.config.security.require_approval;
            let audit_logger = self.audit_logger.clone();

            tokio::spawn(async move {
                if let Err(e) = Self::handle_connection(
                    incoming,
                    usb_bridge,
                    allowed_clients,
                    require_approval,
                    audit_logger,
                )
                .await
                {
                    error!("Connection error: {:#}", e);
                }
            });
        }

        Ok(())
    }

    /// Handle a single client connection
    ///
    /// Validates the client against the allowlist and spawns a connection handler
    async fn handle_connection(
        incoming: iroh::endpoint::Incoming,
        usb_bridge: UsbBridge,
        allowed_clients: Arc<RwLock<HashSet<EndpointId>>>,
        require_approval: bool,
        audit_logger: SharedAuditLogger,
    ) -> Result<()> {
        // Wait for connection to establish
        let connection = incoming.await.context("Failed to establish connection")?;

        // Get remote EndpointId
        let remote_endpoint_id = connection.remote_id();
        let endpoint_id_str = remote_endpoint_id.to_string();

        debug!("Connection attempt from: {}", remote_endpoint_id);

        // Check allowlist if required
        if require_approval {
            let clients = allowed_clients.read().await;
            if !clients.is_empty() && !clients.contains(&remote_endpoint_id) {
                warn!(
                    "Rejected connection from unauthorized EndpointId: {}",
                    remote_endpoint_id
                );

                // Audit log: authentication failure
                if let Some(ref logger) = *audit_logger {
                    logger.log_auth_failure(&endpoint_id_str, "EndpointId not in allowlist");
                }

                return Ok(()); // Silent rejection
            }
        }

        info!("Accepted connection from: {}", remote_endpoint_id);

        // Audit log: client connected
        if let Some(ref logger) = *audit_logger {
            logger.log_client_connected(&endpoint_id_str, None);
        }

        // Create and run client connection handler
        let mut client_conn = ClientConnection::new(
            remote_endpoint_id,
            connection,
            usb_bridge,
            audit_logger.clone(),
        );

        client_conn.run().await?;

        // Audit log: client disconnected
        if let Some(ref logger) = *audit_logger {
            logger.log_client_disconnected(&endpoint_id_str, None);
        }

        info!("Connection closed: {}", remote_endpoint_id);
        Ok(())
    }

    /// Parse allowlist from config strings
    ///
    /// EndpointIds should be in hex format (64 characters) or base32 format
    fn parse_allowlist(approved_clients: &[String]) -> Result<HashSet<EndpointId>> {
        let mut allowlist = HashSet::new();

        for client_str in approved_clients {
            if client_str.is_empty() {
                continue;
            }

            // Try to parse EndpointId - Iroh EndpointIds are 32-byte Ed25519 public keys
            match client_str.parse::<EndpointId>() {
                Ok(endpoint_id) => {
                    allowlist.insert(endpoint_id);
                }
                Err(e) => {
                    warn!("Failed to parse EndpointId '{}': {}", client_str, e);
                }
            }
        }

        Ok(allowlist)
    }

    /// Add a client to the allowlist at runtime
    #[allow(dead_code)]
    pub async fn add_client(&self, endpoint_id: EndpointId) -> Result<()> {
        let mut clients = self.allowed_clients.write().await;
        clients.insert(endpoint_id);
        info!("Added client to allowlist: {}", endpoint_id);
        Ok(())
    }

    /// Remove a client from the allowlist
    #[allow(dead_code)]
    pub async fn remove_client(&self, endpoint_id: &EndpointId) -> Result<()> {
        let mut clients = self.allowed_clients.write().await;
        if clients.remove(endpoint_id) {
            info!("Removed client from allowlist: {}", endpoint_id);
            Ok(())
        } else {
            Err(anyhow!("Client not in allowlist: {}", endpoint_id))
        }
    }

    /// Gracefully shutdown the server
    #[allow(dead_code)]
    pub async fn shutdown(self) -> Result<()> {
        info!("Shutting down Iroh server...");

        // Close endpoint properly (closes all connections gracefully)
        self.endpoint.close().await;

        info!("Server shutdown complete");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::create_audit_logger;
    use crate::config::AuditConfig;
    use common::create_usb_bridge;

    #[tokio::test]
    async fn test_server_creation() {
        let config = ServerConfig::default();
        let (usb_bridge, _worker) = create_usb_bridge();
        let audit_logger = create_audit_logger(AuditConfig::default());

        let server = IrohServer::new(config, usb_bridge, audit_logger).await;
        assert!(server.is_ok());

        let server = server.unwrap();
        // Iroh 0.94+ uses 64-character hex representation for EndpointId (PublicKey)
        assert_eq!(server.endpoint_id().to_string().len(), 64);
    }

    #[tokio::test]
    async fn test_parse_allowlist() {
        let clients = vec![];
        let allowlist = IrohServer::parse_allowlist(&clients).unwrap();
        assert_eq!(allowlist.len(), 0);
    }

    #[tokio::test]
    async fn test_add_remove_client() {
        let config = ServerConfig::default();
        let (usb_bridge, _worker) = create_usb_bridge();
        let audit_logger = create_audit_logger(AuditConfig::default());
        let server = IrohServer::new(config, usb_bridge, audit_logger)
            .await
            .unwrap();

        // Generate a test EndpointId (in real usage, get from client)
        let test_endpoint_id = server.endpoint_id(); // Use server's own EndpointId for testing

        // Add client
        server.add_client(test_endpoint_id).await.unwrap();

        // Verify in allowlist
        let clients = server.allowed_clients.read().await;
        assert!(clients.contains(&test_endpoint_id));
        drop(clients);

        // Remove client
        server.remove_client(&test_endpoint_id).await.unwrap();

        // Verify removed
        let clients = server.allowed_clients.read().await;
        assert!(!clients.contains(&test_endpoint_id));
    }
}
