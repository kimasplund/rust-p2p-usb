//! USB device abstraction
//!
//! This module provides a wrapper around rusb::Device with cached descriptors
//! and convenient conversion to protocol types.

use protocol::{AttachError, DeviceId, DeviceInfo, DeviceSpeed, SuperSpeedConfig};
use rusb::{Context, Device, DeviceDescriptor, DeviceHandle};
use tracing::{debug, warn};

/// USB device wrapper with cached information
pub struct UsbDevice {
    /// Underlying rusb device
    device: Device<Context>,
    /// Device ID (server-assigned)
    id: DeviceId,
    /// Cached device descriptor
    descriptor: DeviceDescriptor,
    /// Device handle (if opened)
    handle: Option<DeviceHandle<Context>>,
    /// Cached device speed for optimization decisions
    speed: DeviceSpeed,
    /// Transfer configuration based on device speed
    transfer_config: SuperSpeedConfig,
    /// Number of interfaces (cached when device is opened)
    num_interfaces: u8,
}

impl UsbDevice {
    /// Create a new USB device wrapper
    ///
    /// Reads and caches the device descriptor and determines optimal transfer
    /// configuration based on device speed.
    pub fn new(device: Device<Context>, id: DeviceId) -> Result<Self, rusb::Error> {
        let descriptor = device.device_descriptor()?;
        let speed = map_device_speed(device.speed());
        let transfer_config = SuperSpeedConfig::for_speed(speed);

        debug!(
            "Creating USB device {:?} - speed: {:?}, max_bulk: {}KB, burst: {}",
            id,
            speed,
            transfer_config.max_bulk_size / 1024,
            transfer_config.enable_burst
        );

        Ok(Self {
            device,
            id,
            descriptor,
            handle: None,
            speed,
            transfer_config,
            num_interfaces: 0,
        })
    }

    /// Get the device ID
    pub fn id(&self) -> DeviceId {
        self.id
    }

    /// Get the bus number
    pub fn bus_number(&self) -> u8 {
        self.device.bus_number()
    }

    /// Get the device address
    pub fn device_address(&self) -> u8 {
        self.device.address()
    }

    /// Get the device speed
    pub fn speed(&self) -> DeviceSpeed {
        self.speed
    }

    /// Get the transfer configuration for this device
    pub fn transfer_config(&self) -> &SuperSpeedConfig {
        &self.transfer_config
    }

    /// Check if this is a SuperSpeed (USB 3.0+) device
    pub fn is_superspeed(&self) -> bool {
        self.speed.is_superspeed()
    }

    /// Get the maximum bulk transfer size for this device
    ///
    /// USB 3.0 SuperSpeed devices support up to 1MB transfers,
    /// while USB 2.0 devices are limited to 64KB.
    pub fn max_bulk_transfer_size(&self) -> usize {
        self.transfer_config.max_bulk_size
    }

    /// Get the optimal URB buffer size for USB/IP transfers
    pub fn optimal_urb_buffer_size(&self) -> usize {
        self.transfer_config.urb_buffer_size
    }

    /// Convert to protocol DeviceInfo
    ///
    /// Reads string descriptors (manufacturer, product, serial) if available.
    pub fn device_info(&self) -> DeviceInfo {
        // Try to open device temporarily to read strings
        let strings = self
            .device
            .open()
            .ok()
            .and_then(|handle| self.read_string_descriptors(&handle));

        let (manufacturer, product, serial_number) = strings.unwrap_or((None, None, None));

        DeviceInfo {
            id: self.id,
            vendor_id: self.descriptor.vendor_id(),
            product_id: self.descriptor.product_id(),
            bus_number: self.bus_number(),
            device_address: self.device_address(),
            manufacturer,
            product,
            serial_number,
            class: self.descriptor.class_code(),
            subclass: self.descriptor.sub_class_code(),
            protocol: self.descriptor.protocol_code(),
            speed: self.speed,
            num_configurations: self.descriptor.num_configurations(),
        }
    }

    /// Open the device for transfers
    ///
    /// This must be called before submitting any transfers.
    /// This will automatically detach kernel drivers and claim interface 0.
    pub fn open(&mut self) -> Result<(), AttachError> {
        if self.handle.is_some() {
            return Ok(()); // Already open
        }

        let handle = self.device.open().map_err(|e| {
            warn!("Failed to open device: {}", e);
            match e {
                rusb::Error::NotFound => AttachError::DeviceNotFound,
                rusb::Error::Access => AttachError::PermissionDenied,
                _ => AttachError::Other {
                    message: e.to_string(),
                },
            }
        })?;

        debug!("Opened device {:?}", self.id);

        // Get the number of interfaces from the active configuration
        let num_interfaces = match self.device.active_config_descriptor() {
            Ok(config) => config.num_interfaces(),
            Err(e) => {
                warn!("Failed to get config descriptor, assuming 1 interface: {}", e);
                1
            }
        };
        debug!("Device {:?} has {} interface(s)", self.id, num_interfaces);

        // Detach kernel drivers from ALL interfaces
        // This is necessary because Linux kernel drivers (like usbhid, usb-storage)
        // will have claimed the interfaces, preventing us from accessing them
        for iface in 0..num_interfaces {
            match handle.kernel_driver_active(iface) {
                Ok(true) => {
                    debug!(
                        "Detaching kernel driver from interface {} on device {:?}",
                        iface, self.id
                    );
                    if let Err(e) = handle.detach_kernel_driver(iface) {
                        warn!("Failed to detach kernel driver from interface {}: {}", iface, e);
                        // Continue anyway - some interfaces may not need detachment
                    }
                }
                Ok(false) => {
                    debug!("No kernel driver active on interface {}", iface);
                }
                Err(e) => {
                    // Some platforms don't support this operation, so just log and continue
                    debug!("Could not check kernel driver status for interface {}: {}", iface, e);
                }
            }
        }

        // Claim all interfaces for our exclusive use
        for iface in 0..num_interfaces {
            if let Err(e) = handle.claim_interface(iface) {
                warn!("Failed to claim interface {}: {}", iface, e);
                // Continue anyway - some interfaces may not be claimable
            } else {
                debug!("Claimed interface {} on device {:?}", iface, self.id);
            }
        }

        // Store number of interfaces for close()
        self.num_interfaces = num_interfaces;

        self.handle = Some(handle);
        Ok(())
    }

    /// Close the device
    ///
    /// This will release all claimed interfaces and reattach kernel drivers
    /// to restore the device to normal kernel control.
    pub fn close(&mut self) {
        if let Some(handle) = self.handle.take() {
            // Release all interfaces before closing
            for iface in 0..self.num_interfaces {
                if let Err(e) = handle.release_interface(iface) {
                    warn!("Failed to release interface {}: {}", iface, e);
                } else {
                    debug!("Released interface {} on device {:?}", iface, self.id);
                }
            }

            // Reattach kernel drivers to restore device to kernel control
            // This allows the device to be used normally on the server again
            // after we're done sharing it via USB/IP
            for iface in 0..self.num_interfaces {
                if let Err(e) = handle.attach_kernel_driver(iface) {
                    // This may fail if no driver was attached originally, which is fine
                    debug!(
                        "Could not reattach kernel driver to interface {} (may not have been detached): {}",
                        iface, e
                    );
                } else {
                    debug!(
                        "Reattached kernel driver to interface {} on device {:?}",
                        iface, self.id
                    );
                }
            }

            self.num_interfaces = 0;
            debug!("Closed device {:?}", self.id);
        }
    }

    /// Check if device is open
    pub fn is_open(&self) -> bool {
        self.handle.is_some()
    }

    /// Get mutable reference to device handle
    ///
    /// Returns None if device is not open.
    pub fn handle_mut(&mut self) -> Option<&mut DeviceHandle<Context>> {
        self.handle.as_mut()
    }

    /// Claim an interface
    ///
    /// This must be called before submitting transfers to non-zero endpoints.
    pub fn claim_interface(&mut self, interface: u8) -> Result<(), rusb::Error> {
        let handle = self.handle.as_mut().ok_or(rusb::Error::InvalidParam)?;

        handle.claim_interface(interface)?;
        debug!("Claimed interface {} on device {:?}", interface, self.id);
        Ok(())
    }

    /// Release an interface
    pub fn release_interface(&mut self, interface: u8) -> Result<(), rusb::Error> {
        let handle = self.handle.as_mut().ok_or(rusb::Error::InvalidParam)?;

        handle.release_interface(interface)?;
        debug!("Released interface {} on device {:?}", interface, self.id);
        Ok(())
    }

    /// Reset the device
    ///
    /// This will reset the device and invalidate any claimed interfaces.
    pub fn reset(&mut self) -> Result<(), rusb::Error> {
        let handle = self.handle.as_mut().ok_or(rusb::Error::InvalidParam)?;

        handle.reset()?;
        debug!("Reset device {:?}", self.id);
        Ok(())
    }

    /// Read string descriptors from device
    fn read_string_descriptors(
        &self,
        handle: &DeviceHandle<Context>,
    ) -> Option<(Option<String>, Option<String>, Option<String>)> {
        let manufacturer = self
            .descriptor
            .manufacturer_string_index()
            .and_then(|idx| handle.read_string_descriptor_ascii(idx).ok());

        let product = self
            .descriptor
            .product_string_index()
            .and_then(|idx| handle.read_string_descriptor_ascii(idx).ok());

        let serial_number = self
            .descriptor
            .serial_number_string_index()
            .and_then(|idx| handle.read_string_descriptor_ascii(idx).ok());

        Some((manufacturer, product, serial_number))
    }
}

/// Map rusb device speed to protocol DeviceSpeed
fn map_device_speed(speed: rusb::Speed) -> DeviceSpeed {
    match speed {
        rusb::Speed::Low => DeviceSpeed::Low,
        rusb::Speed::Full => DeviceSpeed::Full,
        rusb::Speed::High => DeviceSpeed::High,
        rusb::Speed::Super => DeviceSpeed::Super,
        rusb::Speed::SuperPlus => DeviceSpeed::SuperPlus,
        _ => DeviceSpeed::Full, // Default fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_device_speed() {
        assert_eq!(map_device_speed(rusb::Speed::Low), DeviceSpeed::Low);
        assert_eq!(map_device_speed(rusb::Speed::Full), DeviceSpeed::Full);
        assert_eq!(map_device_speed(rusb::Speed::High), DeviceSpeed::High);
        assert_eq!(map_device_speed(rusb::Speed::Super), DeviceSpeed::Super);
        assert_eq!(
            map_device_speed(rusb::Speed::SuperPlus),
            DeviceSpeed::SuperPlus
        );
    }

    #[test]
    fn test_device_id_copy() {
        let id1 = DeviceId(42);
        let id2 = id1;
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_superspeed_config_for_speed() {
        let low_config = SuperSpeedConfig::for_speed(DeviceSpeed::Low);
        assert_eq!(low_config.max_bulk_size, 4 * 1024);
        assert!(!low_config.enable_burst);

        let high_config = SuperSpeedConfig::for_speed(DeviceSpeed::High);
        assert_eq!(high_config.max_bulk_size, 64 * 1024);
        assert!(!high_config.enable_burst);

        let super_config = SuperSpeedConfig::for_speed(DeviceSpeed::Super);
        assert_eq!(super_config.max_bulk_size, 1024 * 1024);
        assert!(super_config.enable_burst);
        assert_eq!(super_config.urb_buffer_size, 256 * 1024);
    }
}
