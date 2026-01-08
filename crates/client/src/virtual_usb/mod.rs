//! Virtual USB device creation
//!
//! Platform-specific implementations for creating virtual USB devices.
//!
//! # Platform Support
//!
//! - **Linux**: Full support using USB/IP (vhci_hcd kernel module)
//! - **macOS**: Not yet implemented (future: IOKit/DriverKit)
//! - **Windows**: Not yet implemented (future: usbdk/libusb)
//!
//! # Architecture
//!
//! The virtual USB layer creates kernel-visible USB devices on the client
//! that proxy all USB operations to remote devices via the network layer.
//!
//! On Linux, we use the USB/IP kernel module (vhci_hcd) which provides
//! a virtual host controller interface. Devices are attached to this
//! virtual controller and appear in the system as if physically connected.

use anyhow::Result;
use iroh::PublicKey as EndpointId;
use protocol::DeviceHandle;
use std::collections::HashSet;
use std::sync::Arc;

use crate::network::device_proxy::DeviceProxy;

/// Unique device identifier across all connected servers
///
/// Since DeviceHandle is server-assigned and different servers may assign
/// the same handle value, we need a composite key to uniquely identify
/// devices when connected to multiple servers simultaneously.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlobalDeviceId {
    /// The server's EndpointId (iroh PublicKey)
    pub server_id: EndpointId,
    /// The server-assigned device handle
    pub device_handle: DeviceHandle,
}

impl GlobalDeviceId {
    /// Create a new GlobalDeviceId
    pub fn new(server_id: EndpointId, device_handle: DeviceHandle) -> Self {
        Self {
            server_id,
            device_handle,
        }
    }
}

impl std::fmt::Display for GlobalDeviceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}:{}",
            &self.server_id.to_string()[..8],
            self.device_handle.0
        )
    }
}

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "linux")]
pub mod usbip_protocol;

#[cfg(target_os = "linux")]
pub mod socket_bridge;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "windows")]
pub mod windows;

pub mod device;
pub mod interrupt_receive_buffer;

// Re-export public types for internal use
#[allow(unused_imports)]
pub(crate) use device::VirtualDevice;

/// Virtual USB manager interface
///
/// Manages virtual USB device lifecycle across platforms.
/// Supports multiple servers simultaneously by using GlobalDeviceId
/// to uniquely identify devices across all connected servers.
pub struct VirtualUsbManager {
    #[cfg(target_os = "linux")]
    inner: linux::LinuxVirtualUsbManager,

    #[cfg(target_os = "macos")]
    inner: macos::MacOsVirtualUsbManager,

    #[cfg(target_os = "windows")]
    inner: windows::WindowsVirtualUsbManager,
}

impl VirtualUsbManager {
    /// Create a new virtual USB manager
    ///
    /// # Platform Requirements
    ///
    /// - **Linux**: Requires vhci_hcd kernel module loaded (`modprobe vhci-hcd`)
    /// - **macOS**: Not implemented
    /// - **Windows**: Not implemented
    pub async fn new() -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            Ok(Self {
                inner: linux::LinuxVirtualUsbManager::new().await?,
            })
        }

        #[cfg(target_os = "macos")]
        {
            Ok(Self {
                inner: macos::MacOsVirtualUsbManager::new().await?,
            })
        }

        #[cfg(target_os = "windows")]
        {
            Ok(Self {
                inner: windows::WindowsVirtualUsbManager::new().await?,
            })
        }
    }

    /// Attach a remote device as a virtual USB device
    ///
    /// Creates a virtual USB device in the kernel that proxies all
    /// operations to the remote device via the provided DeviceProxy.
    ///
    /// # Returns
    ///
    /// GlobalDeviceId that uniquely identifies this device across all servers.
    pub async fn attach_device(&self, device_proxy: Arc<DeviceProxy>) -> Result<GlobalDeviceId> {
        self.inner.attach_device(device_proxy).await
    }

    /// Detach a virtual USB device
    ///
    /// Removes the virtual device from the system and cleans up resources.
    pub async fn detach_device(&self, global_id: GlobalDeviceId) -> Result<()> {
        self.inner.detach_device(global_id).await
    }

    /// Detach all devices from a specific server
    ///
    /// Used when a server connection is lost or intentionally closed.
    /// Returns the list of GlobalDeviceIds that were successfully detached.
    pub async fn detach_all_from_server(
        &self,
        server_id: EndpointId,
    ) -> Result<Vec<GlobalDeviceId>> {
        self.inner.detach_all_from_server(server_id).await
    }

    /// Handle device removal notification from server
    ///
    /// Automatically detaches virtual devices when the remote device is removed.
    /// Returns the list of GlobalDeviceIds that were successfully detached.
    pub async fn handle_device_removed(
        &self,
        server_id: EndpointId,
        device_id: protocol::DeviceId,
        invalidated_handles: Vec<protocol::DeviceHandle>,
    ) -> Result<Vec<GlobalDeviceId>> {
        self.inner
            .handle_device_removed(server_id, device_id, invalidated_handles)
            .await
    }

    /// Get the device IDs of all locally attached virtual devices for a specific server
    ///
    /// Returns a set of DeviceIds for devices currently attached via USB/IP from the given server.
    /// Used for reconciliation after reconnection to compare with server state.
    #[cfg(target_os = "linux")]
    pub async fn get_attached_device_ids(
        &self,
        server_id: EndpointId,
    ) -> HashSet<protocol::DeviceId> {
        self.inner.get_attached_device_ids(server_id).await
    }

    /// Get the device IDs of all locally attached virtual devices for a specific server
    #[cfg(not(target_os = "linux"))]
    pub async fn get_attached_device_ids(
        &self,
        _server_id: EndpointId,
    ) -> HashSet<protocol::DeviceId> {
        HashSet::new()
    }

    /// Get detailed information about all attached virtual devices for a specific server
    ///
    /// Returns a vector of (GlobalDeviceId, DeviceId) pairs for all attached devices from the server.
    /// Used for reconciliation to identify which devices to detach.
    #[cfg(target_os = "linux")]
    pub async fn get_attached_device_info(
        &self,
        server_id: EndpointId,
    ) -> Vec<(GlobalDeviceId, protocol::DeviceId)> {
        self.inner.get_attached_device_info(server_id).await
    }

    /// Get detailed information about all attached virtual devices for a specific server
    #[cfg(not(target_os = "linux"))]
    pub async fn get_attached_device_info(
        &self,
        _server_id: EndpointId,
    ) -> Vec<(GlobalDeviceId, protocol::DeviceId)> {
        Vec::new()
    }

    /// Get all attached devices across all servers
    #[cfg(target_os = "linux")]
    pub async fn get_all_attached_devices(&self) -> Vec<GlobalDeviceId> {
        self.inner.list_devices().await
    }

    /// Get all attached devices across all servers
    #[cfg(not(target_os = "linux"))]
    pub async fn get_all_attached_devices(&self) -> Vec<GlobalDeviceId> {
        Vec::new()
    }
}
