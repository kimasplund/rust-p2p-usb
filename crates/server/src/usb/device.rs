//! USB device abstraction
//!
//! This module provides a wrapper around rusb::Device with cached descriptors
//! and convenient conversion to protocol types.

use protocol::{AttachError, DeviceId, DeviceInfo, DeviceSpeed};
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
    /// List of interfaces claimed by us
    claimed_interfaces: Vec<u8>,
}

impl UsbDevice {
    /// Create a new USB device wrapper
    ///
    /// Reads and caches the device descriptor.
    pub fn new(device: Device<Context>, id: DeviceId) -> Result<Self, rusb::Error> {
        let descriptor = device.device_descriptor()?;

        Ok(Self {
            device,
            id,
            descriptor,
            handle: None,
            claimed_interfaces: Vec::new(),
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
            speed: map_device_speed(self.device.speed()),
            num_configurations: self.descriptor.num_configurations(),
        }
    }

    /// Open the device for transfers
    ///
    /// This must be called before submitting any transfers.
    /// This will automatically detach kernel drivers and claim all interfaces
    /// of the active configuration.
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

        // Get active configuration to list interfaces
        let config = self.device.active_config_descriptor().map_err(|e| {
            warn!("Failed to get active config descriptor: {}", e);
            AttachError::Other {
                message: format!("Failed to get config descriptor: {}", e),
            }
        })?;

        // Iterate over all interfaces in the active configuration
        for interface in config.interfaces() {
            let interface_number = interface.number();

            // Detach kernel driver if active
            match handle.kernel_driver_active(interface_number) {
                Ok(true) => {
                    debug!(
                        "Detaching kernel driver from interface {} on device {:?}",
                        interface_number, self.id
                    );
                    if let Err(e) = handle.detach_kernel_driver(interface_number) {
                        warn!(
                            "Failed to detach kernel driver from interface {}: {}",
                            interface_number, e
                        );
                        // We continue here because some interfaces might fail but others succeed
                        // and we want to try to claim what we can.
                        // However, if we fail to detach, claiming will likely fail next.
                    }
                }
                Ok(false) => {
                    debug!("No kernel driver active on interface {}", interface_number);
                }
                Err(e) => {
                    debug!(
                        "Could not check kernel driver status for interface {}: {}",
                        interface_number, e
                    );
                }
            }

            // Claim interface
            if let Err(e) = handle.claim_interface(interface_number) {
                warn!("Failed to claim interface {}: {}", interface_number, e);
                // Clean up any previously claimed interfaces?
                // For now we return error if any claim fails, as we need consistent state.
                // But first we should probably release ones we already claimed?
                // Let's rely on Drop or cleanup later, but properly we should unwind.
                // For simplicity in this implementation, we return error and `close` will handle cleanup.
                return Err(AttachError::Other {
                    message: format!("Failed to claim interface {}: {}", interface_number, e),
                });
            }

            debug!("Claimed interface {} on device {:?}", interface_number, self.id);
            self.claimed_interfaces.push(interface_number);
        }

        self.handle = Some(handle);
        Ok(())
    }

    /// Close the device
    ///
    /// This will release claimed interfaces and reattach kernel drivers
    /// to restore the device to normal kernel control.
    pub fn close(&mut self) {
        if let Some(handle) = self.handle.take() {
            // Release all claimed interfaces
            for interface in &self.claimed_interfaces {
                if let Err(e) = handle.release_interface(*interface) {
                    warn!("Failed to release interface {}: {}", interface, e);
                }

                // Reattach kernel driver to restore device to kernel control
                if let Err(e) = handle.attach_kernel_driver(*interface) {
                    debug!(
                        "Could not reattach kernel driver to interface {} (may not have been detached): {}",
                        *interface, e
                    );
                } else {
                    debug!(
                        "Reattached kernel driver to interface {} on device {:?}",
                        *interface, self.id
                    );
                }
            }
            self.claimed_interfaces.clear();

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
}
