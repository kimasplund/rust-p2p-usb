//! Linux virtual USB implementation using USB/IP
//!
//! This module implements virtual USB device creation on Linux using the
//! vhci_hcd (Virtual Host Controller Interface) kernel module, which is part
//! of the USB/IP subsystem.
//!
//! # How USB/IP Works
//!
//! USB/IP is a kernel-level USB device sharing system included in the Linux
//! kernel since 2.6.28. It provides a virtual USB host controller (vhci_hcd)
//! that applications can attach devices to via sysfs.
//!
//! ## Sysfs Interface
//!
//! - `/sys/devices/platform/vhci_hcd.X/attach` - Attach a device
//! - `/sys/devices/platform/vhci_hcd.X/detach` - Detach a device
//! - `/sys/devices/platform/vhci_hcd.X/status` - Query attached devices
//!
//! ## Attach Format
//!
//! Write to `attach`: `<port> <speed> <devid> <sockfd>`
//!
//! - `port`: VHCI port number (0-7 for vhci_hcd.0)
//! - `speed`: Device speed (1=low, 2=full, 3=high, 4=super, 5=super+)
//! - `devid`: Unique device ID (busnum << 16 | devnum)
//! - `sockfd`: Socket file descriptor (-1 for userspace implementation)
//!
//! # Limitations
//!
//! - Requires vhci_hcd kernel module loaded
//! - Maximum 8 devices per vhci_hcd instance (can use multiple instances)
//! - Requires appropriate permissions (root or udev rules)

use anyhow::{Context, Result, anyhow};
use protocol::{DeviceHandle, DeviceSpeed};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

use super::device::VirtualDevice;
use super::socket_bridge::SocketBridge;
use crate::network::device_proxy::DeviceProxy;

/// Linux-specific virtual USB manager using USB/IP
pub struct LinuxVirtualUsbManager {
    /// Attached virtual devices
    attached_devices: Arc<RwLock<HashMap<DeviceHandle, VirtualDevice>>>,
    /// Socket bridges for USB/IP protocol
    socket_bridges: Arc<RwLock<HashMap<DeviceHandle, Arc<SocketBridge>>>>,
    /// VHCI device path (e.g., /sys/devices/platform/vhci_hcd.0)
    vhci_path: PathBuf,
    /// Next port to allocate (0-7)
    next_port: Arc<RwLock<u8>>,
}

impl LinuxVirtualUsbManager {
    /// Create a new Linux virtual USB manager
    ///
    /// # Errors
    ///
    /// Returns error if vhci_hcd module is not loaded or accessible.
    pub async fn new() -> Result<Self> {
        // Check if vhci_hcd is available
        let vhci_path = Self::find_vhci_device()?;

        info!("Found vhci_hcd at: {}", vhci_path.display());

        Ok(Self {
            attached_devices: Arc::new(RwLock::new(HashMap::new())),
            socket_bridges: Arc::new(RwLock::new(HashMap::new())),
            vhci_path,
            next_port: Arc::new(RwLock::new(0)),
        })
    }

    /// Find the vhci_hcd device path
    fn find_vhci_device() -> Result<PathBuf> {
        // Try common paths
        for i in 0..4 {
            let path = PathBuf::from(format!("/sys/devices/platform/vhci_hcd.{}", i));
            if path.exists() {
                return Ok(path);
            }
        }

        // Also check for vhci_hcd without number suffix
        let path = PathBuf::from("/sys/devices/platform/vhci_hcd");
        if path.exists() {
            return Ok(path);
        }

        Err(anyhow!(
            "vhci_hcd not found. Please load the kernel module: sudo modprobe vhci-hcd"
        ))
    }

    /// Attach a device to the virtual USB controller
    pub async fn attach_device(&self, device_proxy: Arc<DeviceProxy>) -> Result<DeviceHandle> {
        let device_info = device_proxy.device_info();
        let handle = DeviceHandle(device_info.id.0);

        debug!(
            "Attaching virtual device: {} (VID: {:04x}, PID: {:04x})",
            device_proxy.description(),
            device_info.vendor_id,
            device_info.product_id
        );

        // Ensure device proxy is attached to remote
        if !device_proxy.is_attached().await {
            device_proxy
                .attach()
                .await
                .context("Failed to attach to remote device")?;
        }

        // Allocate a VHCI port
        let port = self.allocate_port().await?;

        // Map device speed to USB/IP speed code
        let speed = map_device_speed(device_info.speed);

        // Generate unique device ID (using device handle as ID)
        let devid = handle.0;

        // Create socket bridge for USB/IP protocol
        let (socket_bridge, vhci_fd) = SocketBridge::new(device_proxy.clone(), devid, port)
            .await
            .context("Failed to create socket bridge")?;

        let socket_bridge = Arc::new(socket_bridge);

        // Attach to VHCI via sysfs (pass real socket FD)
        self.attach_to_vhci(port, speed, devid, vhci_fd)
            .await
            .context("Failed to attach device to vhci_hcd")?;

        // Start the socket bridge
        socket_bridge.clone().start();

        // Create virtual device
        let virtual_device =
            VirtualDevice::new(handle, device_proxy.clone(), device_info.clone(), port);

        // Store device and bridge
        self.attached_devices
            .write()
            .await
            .insert(handle, virtual_device);

        self.socket_bridges
            .write()
            .await
            .insert(handle, socket_bridge);

        info!(
            "Virtual device attached successfully: handle={}, port={}",
            handle.0, port
        );

        Ok(handle)
    }

    /// Detach a virtual USB device
    pub async fn detach_device(&self, handle: DeviceHandle) -> Result<()> {
        let mut devices = self.attached_devices.write().await;

        let device = devices
            .get(&handle)
            .ok_or_else(|| anyhow!("Device handle {} not found", handle.0))?;

        let port = device.vhci_port();
        let device_proxy = device.device_proxy().clone();

        debug!(
            "Detaching virtual device: handle={}, port={}",
            handle.0, port
        );

        // Stop the socket bridge first
        let mut bridges = self.socket_bridges.write().await;
        if let Some(bridge) = bridges.remove(&handle) {
            bridge.stop();
        }
        drop(bridges);

        // Detach from VHCI
        self.detach_from_vhci(port)
            .await
            .context("Failed to detach from vhci_hcd")?;

        // Free the port
        self.free_port(port).await;

        // Remove from map (now safe - no more references to device)
        devices.remove(&handle);

        // Detach from remote device
        let device_proxy = &device_proxy;
        if device_proxy.is_attached().await {
            device_proxy
                .detach()
                .await
                .context("Failed to detach from remote device")?;
        }

        info!("Virtual device detached successfully: handle={}", handle.0);

        Ok(())
    }

    /// List all attached virtual devices
    pub async fn list_devices(&self) -> Vec<DeviceHandle> {
        self.attached_devices.read().await.keys().copied().collect()
    }

    /// Allocate a VHCI port
    async fn allocate_port(&self) -> Result<u8> {
        let mut next_port = self.next_port.write().await;

        if *next_port >= 8 {
            return Err(anyhow!(
                "No available VHCI ports (maximum 8 devices supported)"
            ));
        }

        let port = *next_port;
        *next_port += 1;

        Ok(port)
    }

    /// Free a VHCI port
    async fn free_port(&self, _port: u8) {
        // Note: In a production implementation, we would track which ports
        // are in use and allow reuse. For Phase 5, we use a simple incrementing
        // counter that doesn't reuse ports.
        //
        // Future enhancement: Implement a proper port allocation bitmap.
    }

    /// Attach a device to the VHCI controller via sysfs
    async fn attach_to_vhci(&self, port: u8, speed: u8, devid: u32, sockfd: std::os::unix::io::RawFd) -> Result<()> {
        let attach_path = self.vhci_path.join("attach");

        // Format: <port> <speed> <devid> <sockfd>
        // sockfd = real socket FD from socket bridge
        let attach_string = format!("{} {} {} {}\n", port, speed, devid, sockfd);

        debug!(
            "Writing to {}: {}",
            attach_path.display(),
            attach_string.trim()
        );

        // Write to sysfs (requires elevated privileges)
        let mut file = OpenOptions::new()
            .write(true)
            .open(&attach_path)
            .context(format!(
                "Failed to open {} (requires root or appropriate udev rules)",
                attach_path.display()
            ))?;

        file.write_all(attach_string.as_bytes())
            .context("Failed to write to attach file")?;

        file.flush()?;

        Ok(())
    }

    /// Detach a device from the VHCI controller via sysfs
    async fn detach_from_vhci(&self, port: u8) -> Result<()> {
        let detach_path = self.vhci_path.join("detach");

        // Format: <port>
        let detach_string = format!("{}\n", port);

        debug!(
            "Writing to {}: {}",
            detach_path.display(),
            detach_string.trim()
        );

        let mut file = OpenOptions::new()
            .write(true)
            .open(&detach_path)
            .context(format!(
                "Failed to open {} (requires root or appropriate udev rules)",
                detach_path.display()
            ))?;

        file.write_all(detach_string.as_bytes())
            .context("Failed to write to detach file")?;

        file.flush()?;

        Ok(())
    }

    /// Read VHCI status (for debugging/monitoring)
    #[allow(dead_code)]
    async fn read_vhci_status(&self) -> Result<String> {
        let status_path = self.vhci_path.join("status");

        let file = File::open(&status_path)
            .context(format!("Failed to open {}", status_path.display()))?;

        let reader = BufReader::new(file);
        let mut status = String::new();

        for line in reader.lines() {
            let line = line?;
            status.push_str(&line);
            status.push('\n');
        }

        Ok(status)
    }
}

/// Map DeviceSpeed to USB/IP speed code
fn map_device_speed(speed: DeviceSpeed) -> u8 {
    match speed {
        DeviceSpeed::Low => 1,       // 1.5 Mbps
        DeviceSpeed::Full => 2,      // 12 Mbps
        DeviceSpeed::High => 3,      // 480 Mbps
        DeviceSpeed::Super => 4,     // 5 Gbps
        DeviceSpeed::SuperPlus => 5, // 10 Gbps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_speed_mapping() {
        assert_eq!(map_device_speed(DeviceSpeed::Low), 1);
        assert_eq!(map_device_speed(DeviceSpeed::Full), 2);
        assert_eq!(map_device_speed(DeviceSpeed::High), 3);
        assert_eq!(map_device_speed(DeviceSpeed::Super), 4);
        assert_eq!(map_device_speed(DeviceSpeed::SuperPlus), 5);
    }

    #[test]
    fn test_device_id_generation() {
        // Device ID is just the handle value
        let handle = DeviceHandle(0x12345678);
        assert_eq!(handle.0, 0x12345678);
    }
}
