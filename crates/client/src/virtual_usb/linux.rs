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
//! Write to `attach`: `<port> <sockfd> <devid> <speed>`
//!
//! - `port`: VHCI port number (0-7 for vhci_hcd.0)
//! - `sockfd`: Socket file descriptor (-1 for userspace implementation)
//! - `devid`: Unique device ID (busnum << 16 | devnum)
//! - `speed`: Device speed (1=low, 2=full, 3=high, 5=super, 6=super+)
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
    /// Bitmap for high-speed ports (0-7 for USB 2.0 and below)
    /// Each bit represents a port: bit 0 = port 0, bit 7 = port 7
    /// 1 = allocated, 0 = free
    hs_ports: Arc<RwLock<u8>>,
    /// Bitmap for super-speed ports (8-15 for USB 3.0+)
    /// Each bit represents a port: bit 0 = port 8, bit 7 = port 15
    /// 1 = allocated, 0 = free
    ss_ports: Arc<RwLock<u8>>,
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
            hs_ports: Arc::new(RwLock::new(0)), // All high-speed ports free (bitmap)
            ss_ports: Arc::new(RwLock::new(0)), // All super-speed ports free (bitmap)
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

        // Map device speed to USB/IP speed code
        let speed = map_device_speed(device_info.speed);

        // Allocate a VHCI port (speed-aware: HS ports 0-7, SS ports 8-15)
        let port = self.allocate_port(device_info.speed).await?;

        debug!(
            "Device speed mapping: {:?} -> {} (port={}, devid={})",
            device_info.speed, speed, port, handle.0
        );

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

    /// Allocate a VHCI port based on device speed
    ///
    /// vhci_hcd has separate port ranges:
    /// - Ports 0-7: High-speed (hs) for USB 2.0 and below (Low, Full, High)
    /// - Ports 8-15: Super-speed (ss) for USB 3.0+ (Super, SuperPlus)
    ///
    /// Uses bitmap-based allocation to find the first free port and mark it as allocated.
    async fn allocate_port(&self, speed: DeviceSpeed) -> Result<u8> {
        match speed {
            // USB 2.0 and below: use high-speed ports (0-7)
            DeviceSpeed::Low | DeviceSpeed::Full | DeviceSpeed::High => {
                let mut bitmap = self.hs_ports.write().await;

                // Find first free bit (0) using trailing_ones
                // trailing_ones returns the count of consecutive 1s from bit 0
                // If all bits are 1, trailing_ones() returns 8
                let free_bit = (*bitmap).trailing_ones() as u8;

                if free_bit >= 8 {
                    return Err(anyhow!(
                        "No available high-speed VHCI ports (all 8 USB 2.0 ports in use, detach a device to free a port)"
                    ));
                }

                // Set the bit to mark port as allocated
                *bitmap |= 1 << free_bit;

                debug!(
                    "Allocated high-speed port {} (bitmap: {:08b})",
                    free_bit, *bitmap
                );

                Ok(free_bit)
            }
            // USB 3.0+: use super-speed ports (8-15)
            DeviceSpeed::Super | DeviceSpeed::SuperPlus => {
                let mut bitmap = self.ss_ports.write().await;

                // Find first free bit (0)
                let free_bit = (*bitmap).trailing_ones() as u8;

                if free_bit >= 8 {
                    return Err(anyhow!(
                        "No available super-speed VHCI ports (all 8 USB 3.0+ ports in use, detach a device to free a port)"
                    ));
                }

                // Set the bit to mark port as allocated
                *bitmap |= 1 << free_bit;

                // Super-speed ports are offset by 8 (ports 8-15)
                let port = free_bit + 8;

                debug!(
                    "Allocated super-speed port {} (bitmap: {:08b})",
                    port, *bitmap
                );

                Ok(port)
            }
        }
    }

    /// Free a VHCI port for reuse
    ///
    /// Clears the corresponding bit in the port bitmap based on port number.
    /// Safe to call with already-free ports (idempotent).
    async fn free_port(&self, port: u8) {
        if port < 8 {
            // High-speed port (0-7)
            let mut bitmap = self.hs_ports.write().await;
            *bitmap &= !(1 << port);
            debug!("Freed high-speed port {} (bitmap: {:08b})", port, *bitmap);
        } else if port < 16 {
            // Super-speed port (8-15), stored as bits 0-7 in ss_ports
            let bit = port - 8;
            let mut bitmap = self.ss_ports.write().await;
            *bitmap &= !(1 << bit);
            debug!("Freed super-speed port {} (bitmap: {:08b})", port, *bitmap);
        } else {
            // Invalid port number - log warning but don't fail
            debug!("Attempted to free invalid port {} (ignored)", port);
        }
    }

    /// Attach a device to the VHCI controller via sysfs
    async fn attach_to_vhci(
        &self,
        port: u8,
        speed: u8,
        devid: u32,
        sockfd: std::os::unix::io::RawFd,
    ) -> Result<()> {
        let attach_path = self.vhci_path.join("attach");

        // Format: <port> <sockfd> <devid> <speed>
        // sockfd = real socket FD from socket bridge
        let attach_string = format!("{} {} {} {}\n", port, sockfd, devid, speed);

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
        DeviceSpeed::Low => 1,       // USB_SPEED_LOW - 1.5 Mbps
        DeviceSpeed::Full => 2,      // USB_SPEED_FULL - 12 Mbps
        DeviceSpeed::High => 3,      // USB_SPEED_HIGH - 480 Mbps
        DeviceSpeed::Super => 5,     // USB_SPEED_SUPER - 5 Gbps (NOT 4 - that's WIRELESS)
        DeviceSpeed::SuperPlus => 6, // USB_SPEED_SUPER_PLUS - 10+ Gbps
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
        assert_eq!(map_device_speed(DeviceSpeed::Super), 5); // USB_SPEED_SUPER
        assert_eq!(map_device_speed(DeviceSpeed::SuperPlus), 6); // USB_SPEED_SUPER_PLUS
    }

    #[test]
    fn test_device_id_generation() {
        // Device ID is just the handle value
        let handle = DeviceHandle(0x12345678);
        assert_eq!(handle.0, 0x12345678);
    }

    /// Test helper: Create a minimal manager for port allocation tests
    /// Uses a mock vhci_path since we only test port allocation logic
    fn create_test_manager() -> LinuxVirtualUsbManager {
        LinuxVirtualUsbManager {
            attached_devices: Arc::new(RwLock::new(HashMap::new())),
            socket_bridges: Arc::new(RwLock::new(HashMap::new())),
            vhci_path: PathBuf::from("/sys/devices/platform/vhci_hcd.0"),
            hs_ports: Arc::new(RwLock::new(0)),
            ss_ports: Arc::new(RwLock::new(0)),
        }
    }

    #[tokio::test]
    async fn test_allocate_all_hs_ports() {
        let manager = create_test_manager();

        // Allocate all 8 high-speed ports
        for expected_port in 0..8u8 {
            let port = manager.allocate_port(DeviceSpeed::High).await.unwrap();
            assert_eq!(
                port, expected_port,
                "Expected port {} but got {}",
                expected_port, port
            );
        }

        // Verify bitmap is full
        let bitmap = *manager.hs_ports.read().await;
        assert_eq!(
            bitmap, 0xFF,
            "Expected all ports allocated (0xFF), got {:08b}",
            bitmap
        );
    }

    #[tokio::test]
    async fn test_allocate_all_ss_ports() {
        let manager = create_test_manager();

        // Allocate all 8 super-speed ports (should return 8-15)
        for expected_port in 8..16u8 {
            let port = manager.allocate_port(DeviceSpeed::Super).await.unwrap();
            assert_eq!(
                port, expected_port,
                "Expected port {} but got {}",
                expected_port, port
            );
        }

        // Verify bitmap is full
        let bitmap = *manager.ss_ports.read().await;
        assert_eq!(
            bitmap, 0xFF,
            "Expected all ports allocated (0xFF), got {:08b}",
            bitmap
        );
    }

    #[tokio::test]
    async fn test_hs_port_exhaustion_error() {
        let manager = create_test_manager();

        // Allocate all 8 ports
        for _ in 0..8 {
            manager.allocate_port(DeviceSpeed::Full).await.unwrap();
        }

        // 9th allocation should fail with descriptive error
        let result = manager.allocate_port(DeviceSpeed::Full).await;
        assert!(result.is_err());

        let error_msg = result.unwrap_err().to_string();
        assert!(
            error_msg.contains("high-speed") && error_msg.contains("detach"),
            "Error message should mention 'high-speed' and 'detach': {}",
            error_msg
        );
    }

    #[tokio::test]
    async fn test_ss_port_exhaustion_error() {
        let manager = create_test_manager();

        // Allocate all 8 super-speed ports
        for _ in 0..8 {
            manager.allocate_port(DeviceSpeed::SuperPlus).await.unwrap();
        }

        // 9th allocation should fail with descriptive error
        let result = manager.allocate_port(DeviceSpeed::SuperPlus).await;
        assert!(result.is_err());

        let error_msg = result.unwrap_err().to_string();
        assert!(
            error_msg.contains("super-speed") && error_msg.contains("detach"),
            "Error message should mention 'super-speed' and 'detach': {}",
            error_msg
        );
    }

    #[tokio::test]
    async fn test_free_and_reallocate_hs_port() {
        let manager = create_test_manager();

        // Allocate ports 0, 1, 2
        let port0 = manager.allocate_port(DeviceSpeed::High).await.unwrap();
        let port1 = manager.allocate_port(DeviceSpeed::High).await.unwrap();
        let port2 = manager.allocate_port(DeviceSpeed::High).await.unwrap();

        assert_eq!(port0, 0);
        assert_eq!(port1, 1);
        assert_eq!(port2, 2);

        // Free port 1
        manager.free_port(1).await;

        // Next allocation should reuse port 1 (first free bit)
        let reused_port = manager.allocate_port(DeviceSpeed::High).await.unwrap();
        assert_eq!(
            reused_port, 1,
            "Should reuse freed port 1, got {}",
            reused_port
        );

        // Next allocation should use port 3
        let port3 = manager.allocate_port(DeviceSpeed::High).await.unwrap();
        assert_eq!(port3, 3, "Should allocate port 3, got {}", port3);
    }

    #[tokio::test]
    async fn test_free_and_reallocate_ss_port() {
        let manager = create_test_manager();

        // Allocate ports 8, 9, 10
        let port8 = manager.allocate_port(DeviceSpeed::Super).await.unwrap();
        let port9 = manager.allocate_port(DeviceSpeed::Super).await.unwrap();
        let port10 = manager.allocate_port(DeviceSpeed::Super).await.unwrap();

        assert_eq!(port8, 8);
        assert_eq!(port9, 9);
        assert_eq!(port10, 10);

        // Free port 9
        manager.free_port(9).await;

        // Next allocation should reuse port 9
        let reused_port = manager.allocate_port(DeviceSpeed::Super).await.unwrap();
        assert_eq!(
            reused_port, 9,
            "Should reuse freed port 9, got {}",
            reused_port
        );
    }

    #[tokio::test]
    async fn test_free_multiple_ports_and_reallocate() {
        let manager = create_test_manager();

        // Allocate all 8 HS ports
        for _ in 0..8 {
            manager.allocate_port(DeviceSpeed::High).await.unwrap();
        }

        // Free ports 3, 5, 7 (non-contiguous)
        manager.free_port(3).await;
        manager.free_port(5).await;
        manager.free_port(7).await;

        // Bitmap should be 0b01010111 (ports 0,1,2,4,6 = bits 0,1,2,4,6)
        let bitmap = *manager.hs_ports.read().await;
        assert_eq!(
            bitmap, 0b01010111,
            "Expected bitmap 0b01010111, got {:08b}",
            bitmap
        );

        // Reallocate - should get 3, then 5, then 7
        let port_a = manager.allocate_port(DeviceSpeed::High).await.unwrap();
        let port_b = manager.allocate_port(DeviceSpeed::High).await.unwrap();
        let port_c = manager.allocate_port(DeviceSpeed::High).await.unwrap();

        assert_eq!(port_a, 3, "First realloc should be port 3");
        assert_eq!(port_b, 5, "Second realloc should be port 5");
        assert_eq!(port_c, 7, "Third realloc should be port 7");

        // Now all ports should be allocated again
        let result = manager.allocate_port(DeviceSpeed::High).await;
        assert!(result.is_err(), "Should fail when all ports are allocated");
    }

    #[tokio::test]
    async fn test_free_already_free_port_is_idempotent() {
        let manager = create_test_manager();

        // Free port 5 twice - should not panic or corrupt state
        manager.free_port(5).await;
        manager.free_port(5).await;

        // Allocate should still get port 0 first (first free bit)
        let port = manager.allocate_port(DeviceSpeed::High).await.unwrap();
        assert_eq!(port, 0);
    }

    #[tokio::test]
    async fn test_free_invalid_port_is_safe() {
        let manager = create_test_manager();

        // Free invalid port numbers - should not panic
        manager.free_port(16).await;
        manager.free_port(100).await;
        manager.free_port(255).await;

        // State should be unchanged
        let hs = *manager.hs_ports.read().await;
        let ss = *manager.ss_ports.read().await;
        assert_eq!(hs, 0, "HS bitmap should be unchanged");
        assert_eq!(ss, 0, "SS bitmap should be unchanged");
    }

    #[tokio::test]
    async fn test_hs_and_ss_ports_are_independent() {
        let manager = create_test_manager();

        // Fill all HS ports
        for _ in 0..8 {
            manager.allocate_port(DeviceSpeed::High).await.unwrap();
        }

        // Should still be able to allocate SS ports
        let ss_port = manager.allocate_port(DeviceSpeed::Super).await.unwrap();
        assert_eq!(ss_port, 8, "SS allocation should work even when HS is full");

        // HS should still fail
        let hs_result = manager.allocate_port(DeviceSpeed::High).await;
        assert!(hs_result.is_err(), "HS should fail when full");
    }

    #[tokio::test]
    async fn test_speed_to_port_range_mapping() {
        let manager = create_test_manager();

        // Low, Full, High speeds should use HS ports (0-7)
        let low = manager.allocate_port(DeviceSpeed::Low).await.unwrap();
        let full = manager.allocate_port(DeviceSpeed::Full).await.unwrap();
        let high = manager.allocate_port(DeviceSpeed::High).await.unwrap();

        assert!(low < 8, "Low speed should use HS port");
        assert!(full < 8, "Full speed should use HS port");
        assert!(high < 8, "High speed should use HS port");

        // Super, SuperPlus speeds should use SS ports (8-15)
        let super_speed = manager.allocate_port(DeviceSpeed::Super).await.unwrap();
        let super_plus = manager.allocate_port(DeviceSpeed::SuperPlus).await.unwrap();

        assert!(
            super_speed >= 8 && super_speed < 16,
            "Super speed should use SS port"
        );
        assert!(
            super_plus >= 8 && super_plus < 16,
            "SuperPlus speed should use SS port"
        );
    }
}
