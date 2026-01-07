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
use protocol::DeviceHandle;
use std::collections::HashSet;
use std::sync::Arc;

use crate::network::device_proxy::DeviceProxy;

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

// Re-export public types for internal use
#[allow(unused_imports)]
pub(crate) use device::VirtualDevice;

/// Virtual USB manager interface
///
/// Manages virtual USB device lifecycle across platforms.
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
    /// Device handle that can be used to detach the device later.
    pub async fn attach_device(&self, device_proxy: Arc<DeviceProxy>) -> Result<DeviceHandle> {
        self.inner.attach_device(device_proxy).await
    }

    /// Detach a virtual USB device
    ///
    /// Removes the virtual device from the system and cleans up resources.
    pub async fn detach_device(&self, handle: DeviceHandle) -> Result<()> {
        self.inner.detach_device(handle).await
    }

    /// List all attached virtual devices
    pub async fn list_devices(&self) -> Vec<DeviceHandle> {
        self.inner.list_devices().await
    }

    /// Handle device removal notification from server
    ///
    /// Automatically detaches virtual devices when the remote device is removed.
    /// Returns the list of handles that were successfully detached.
    pub async fn handle_device_removed(
        &self,
        device_id: protocol::DeviceId,
        invalidated_handles: Vec<protocol::DeviceHandle>,
    ) -> Result<Vec<protocol::DeviceHandle>> {
        self.inner
            .handle_device_removed(device_id, invalidated_handles)
            .await
    }

    /// Get the device IDs of all locally attached virtual devices
    ///
    /// Returns a set of DeviceIds for devices currently attached via USB/IP.
    /// Used for reconciliation after reconnection to compare with server state.
    #[cfg(target_os = "linux")]
    pub async fn get_attached_device_ids(&self) -> HashSet<protocol::DeviceId> {
        self.inner.get_attached_device_ids().await
    }

    /// Get the device IDs of all locally attached virtual devices
    ///
    /// Returns a set of DeviceIds for devices currently attached via USB/IP.
    /// Used for reconciliation after reconnection to compare with server state.
    #[cfg(not(target_os = "linux"))]
    pub async fn get_attached_device_ids(&self) -> HashSet<protocol::DeviceId> {
        HashSet::new()
    }

    /// Get detailed information about all attached virtual devices
    ///
    /// Returns a vector of (DeviceHandle, DeviceId) pairs for all attached devices.
    /// Used for reconciliation to identify which devices to detach.
    #[cfg(target_os = "linux")]
    pub async fn get_attached_device_info(&self) -> Vec<(DeviceHandle, protocol::DeviceId)> {
        self.inner.get_attached_device_info().await
    }

    /// Get detailed information about all attached virtual devices
    ///
    /// Returns a vector of (DeviceHandle, DeviceId) pairs for all attached devices.
    /// Used for reconciliation to identify which devices to detach.
    #[cfg(not(target_os = "linux"))]
    pub async fn get_attached_device_info(&self) -> Vec<(DeviceHandle, protocol::DeviceId)> {
        Vec::new()
    }
}
