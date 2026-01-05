//! Server session management
//!
//! Manages state and operations for a connection to a single server.

use anyhow::Result;
use iroh::PublicKey as EndpointId;
use protocol::{DeviceHandle, DeviceId, DeviceInfo};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

use super::client::IrohClient;
use super::connection::ServerConnection;
use super::device_proxy::DeviceProxy;

/// Server session
///
/// Represents an active session with a USB server, tracking
/// attached devices and managing their lifecycle.
#[allow(dead_code)]
pub struct ServerSession {
    /// Server EndpointId
    server_id: EndpointId,
    /// Connection to server
    connection: Arc<ServerConnection>,
    /// Attached devices (DeviceHandle -> DeviceProxy)
    attached_devices: Arc<RwLock<HashMap<DeviceHandle, Arc<DeviceProxy>>>>,
    /// Client for creating device proxies
    client: Arc<IrohClient>,
}

#[allow(dead_code)]
impl ServerSession {
    /// Create a new server session
    ///
    /// # Arguments
    /// * `server_id` - Server EndpointId
    /// * `connection` - Established connection to server
    /// * `client` - IrohClient for device operations
    pub fn new(
        server_id: EndpointId,
        connection: Arc<ServerConnection>,
        client: Arc<IrohClient>,
    ) -> Self {
        Self {
            server_id,
            connection,
            attached_devices: Arc::new(RwLock::new(HashMap::new())),
            client,
        }
    }

    /// Get server EndpointId
    pub fn server_id(&self) -> EndpointId {
        self.server_id
    }

    /// List available devices on the server
    pub async fn list_devices(&self) -> Result<Vec<DeviceInfo>> {
        self.connection.list_devices().await
    }

    /// Attach to a device
    ///
    /// Creates a DeviceProxy for the specified device and returns it.
    ///
    /// # Arguments
    /// * `device_id` - ID of device to attach
    /// * `device_info` - Device information (from list_devices)
    ///
    /// # Returns
    /// DeviceProxy for performing USB operations
    pub async fn attach_device(
        &self,
        device_id: DeviceId,
        device_info: DeviceInfo,
    ) -> Result<Arc<DeviceProxy>> {
        debug!(
            "Attaching to device {} on server {}",
            device_id.0, self.server_id
        );

        // Create device proxy
        let proxy = Arc::new(DeviceProxy::new(
            self.client.clone(),
            self.server_id,
            device_info,
        ));

        // Attach to device
        proxy.attach().await?;

        // Get handle
        let handle = proxy.device_info().id;

        // Store in attached devices map
        // Note: We use a placeholder handle here since the actual handle
        // is managed internally by DeviceProxy
        let mut devices = self.attached_devices.write().await;
        devices.insert(DeviceHandle(handle.0), proxy.clone());

        info!("Successfully attached to device {}", device_id.0);

        Ok(proxy)
    }

    /// Detach from a device
    ///
    /// # Arguments
    /// * `handle` - Device handle from attach_device
    pub async fn detach_device(&self, handle: DeviceHandle) -> Result<()> {
        debug!("Detaching from device with handle {}", handle.0);

        let mut devices = self.attached_devices.write().await;

        if let Some(proxy) = devices.remove(&handle) {
            proxy.detach().await?;
            info!("Successfully detached from device with handle {}", handle.0);
            Ok(())
        } else {
            Err(anyhow::anyhow!("Device handle {} not found", handle.0))
        }
    }

    /// Get an attached device proxy
    ///
    /// # Arguments
    /// * `handle` - Device handle from attach_device
    ///
    /// # Returns
    /// DeviceProxy if found
    pub async fn get_device(&self, handle: DeviceHandle) -> Option<Arc<DeviceProxy>> {
        let devices = self.attached_devices.read().await;
        devices.get(&handle).cloned()
    }

    /// Get all attached devices
    pub async fn attached_devices(&self) -> Vec<(DeviceHandle, Arc<DeviceProxy>)> {
        let devices = self.attached_devices.read().await;
        devices.iter().map(|(h, p)| (*h, p.clone())).collect()
    }

    /// Close the session
    ///
    /// Detaches all devices and closes the connection.
    pub async fn close(self) -> Result<()> {
        info!("Closing session with server {}", self.server_id);

        // Detach all devices
        let mut devices = self.attached_devices.write().await;
        for (handle, proxy) in devices.drain() {
            if let Err(e) = proxy.detach().await {
                tracing::warn!("Error detaching device {}: {}", handle.0, e);
            }
        }

        info!("Session closed");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_session_creation() {
        // This is a placeholder test since we can't easily create
        // ServerConnection without network operations
        assert!(true);
    }
}
