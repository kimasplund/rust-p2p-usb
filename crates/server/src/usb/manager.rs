//! USB device manager
//!
//! Handles device enumeration, hot-plug events, and device state tracking.
//! This module runs in the USB thread and manages the device registry.

use crate::usb::device::UsbDevice;
use common::UsbEvent;
use protocol::{AttachError, DetachError, DeviceHandle, DeviceId, DeviceInfo};
use rusb::{Context, Device, Hotplug, HotplugBuilder, Registration, UsbContext};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

/// USB device manager
///
/// Manages the registry of discovered USB devices, handles hot-plug events,
/// and tracks device state (discovered, attached, detached).
pub struct DeviceManager {
    /// USB context for device operations
    context: Context,
    /// Registry of all discovered devices (bus, address) -> UsbDevice
    devices: HashMap<(u8, u8), UsbDevice>,
    /// Mapping of DeviceId -> (bus, address)
    device_ids: HashMap<DeviceId, (u8, u8)>,
    /// Attached devices: DeviceHandle -> DeviceId
    attached: HashMap<DeviceHandle, (DeviceId, String)>,
    /// Next device ID to assign
    next_device_id: u32,
    /// Next device handle to assign
    next_handle_id: u32,
    /// Hot-plug registration
    _hotplug_registration: Option<Registration<Context>>,
    /// Event sender for hot-plug notifications
    event_sender: async_channel::Sender<UsbEvent>,
    /// Device filters (VID:PID patterns)
    allowed_filters: Vec<String>,
}

impl DeviceManager {
    /// Create a new device manager
    pub fn new(
        event_sender: async_channel::Sender<UsbEvent>,
        allowed_filters: Vec<String>,
    ) -> Result<Self, rusb::Error> {
        let context = Context::new()?;

        Ok(Self {
            context,
            devices: HashMap::new(),
            device_ids: HashMap::new(),
            attached: HashMap::new(),
            next_device_id: 1,
            next_handle_id: 1,
            _hotplug_registration: None,
            event_sender,
            allowed_filters,
        })
    }

    /// Initialize device enumeration and hot-plug callbacks
    ///
    /// This should be called once after creating the manager.
    pub fn initialize(&mut self) -> Result<(), rusb::Error> {
        // Enumerate existing devices
        self.enumerate_devices()?;

        // Register hot-plug callbacks
        self.register_hotplug()?;

        info!(
            "Device manager initialized with {} devices",
            self.devices.len()
        );
        Ok(())
    }

    /// Enumerate all currently connected USB devices
    fn enumerate_devices(&mut self) -> Result<(), rusb::Error> {
        let devices = self.context.devices()?;

        for device in devices.iter() {
            if let Err(e) = self.add_device(device) {
                warn!("Failed to add device during enumeration: {}", e);
            }
        }

        debug!("Enumerated {} devices", self.devices.len());
        Ok(())
    }

    /// Register hot-plug callbacks
    fn register_hotplug(&mut self) -> Result<(), rusb::Error> {
        let event_sender = self.event_sender.clone();

        // Create hotplug callback
        let callback = HotplugCallback::new(event_sender);

        let registration = HotplugBuilder::new()
            .enumerate(false) // We already enumerated
            .register(&self.context, Box::new(callback))?;

        self._hotplug_registration = Some(registration);
        debug!("Hot-plug callbacks registered");
        Ok(())
    }

    /// Add a device to the registry
    fn add_device(&mut self, device: Device<Context>) -> Result<DeviceId, rusb::Error> {
        let bus = device.bus_number();
        let address = device.address();
        let key = (bus, address);

        // Check if device is allowed based on filters
        if !self.is_device_allowed(&device) {
            debug!(
                "Device ignored by filter: bus={}, addr={}, vid={:#x}, pid={:#x}",
                bus,
                address,
                device
                    .device_descriptor()
                    .map(|d| d.vendor_id())
                    .unwrap_or(0),
                device
                    .device_descriptor()
                    .map(|d| d.product_id())
                    .unwrap_or(0)
            );
            return Err(rusb::Error::Access); // Treat as access denied / filtered out
        }

        // Check if already tracked
        if let Some(existing_device) = self.devices.get(&key) {
            return Ok(existing_device.id());
        }

        // Skip root hubs - they can't be shared via USB/IP
        // Root hubs are VID 0x1d6b (Linux Foundation) with device class 9 (Hub)
        if let Ok(desc) = device.device_descriptor() {
            if desc.vendor_id() == 0x1d6b && desc.class_code() == 9 {
                debug!(
                    "Skipping root hub: bus={}, addr={}, vid={:#x}, pid={:#x}",
                    bus,
                    address,
                    desc.vendor_id(),
                    desc.product_id()
                );
                return Err(rusb::Error::NotSupported);
            }
        }

        // Assign new device ID
        let device_id = DeviceId(self.next_device_id);
        self.next_device_id += 1;

        // Create USB device wrapper
        let usb_device = UsbDevice::new(device, device_id)?;

        debug!(
            "Added device {:?}: bus={}, addr={}, vid={:#x}, pid={:#x}",
            device_id,
            bus,
            address,
            usb_device.device_info().vendor_id,
            usb_device.device_info().product_id
        );

        self.device_ids.insert(device_id, key);
        self.devices.insert(key, usb_device);

        Ok(device_id)
    }

    /// Remove a device from the registry
    fn remove_device(&mut self, bus: u8, address: u8) -> Option<DeviceId> {
        let key = (bus, address);

        if let Some(device) = self.devices.remove(&key) {
            let device_id = device.id();
            self.device_ids.remove(&device_id);

            // Remove from attached list if present
            self.attached
                .retain(|_handle, (id, _client)| *id != device_id);

            debug!(
                "Removed device {:?}: bus={}, addr={}",
                device_id, bus, address
            );

            Some(device_id)
        } else {
            None
        }
    }

    /// Handle device arrival (from hot-plug callback)
    pub fn handle_device_arrived(&mut self, device: Device<Context>) {
        match self.add_device(device.clone()) {
            Ok(device_id) => {
                // Get device info and send event
                if let Some(usb_device) = self.get_device_by_id(device_id) {
                    let device_info = usb_device.device_info();

                    if let Err(e) = self.event_sender.send_blocking(UsbEvent::DeviceArrived {
                        device: device_info,
                    }) {
                        error!("Failed to send DeviceArrived event: {}", e);
                    }
                }
            }
            Err(e) => {
                warn!("Failed to add arrived device: {}", e);
            }
        }
    }

    /// Handle device removal (from hot-plug callback)
    pub fn handle_device_left(&mut self, bus: u8, address: u8) {
        if let Some(device_id) = self.remove_device(bus, address)
            && let Err(e) = self.event_sender.send_blocking(UsbEvent::DeviceLeft {
                device_id,
                invalidated_handles: Vec::new(), // TODO: Phase 3 will populate this
                affected_clients: Vec::new(),    // TODO: Phase 3 will populate this
            })
        {
            error!("Failed to send DeviceLeft event: {}", e);
        }
    }

    /// List all discovered devices
    pub fn list_devices(&self) -> Vec<DeviceInfo> {
        self.devices
            .values()
            .map(|device| device.device_info())
            .collect()
    }

    /// Attach a device for a client
    pub fn attach_device(
        &mut self,
        device_id: DeviceId,
        client_id: String,
    ) -> Result<DeviceHandle, AttachError> {
        // Check if already attached (before borrowing device)
        if self.attached.values().any(|(id, _)| *id == device_id) {
            return Err(AttachError::AlreadyAttached);
        }

        // Check if device exists and open it
        let device = self
            .get_device_by_id_mut(device_id)
            .ok_or(AttachError::DeviceNotFound)?;

        // Open the device
        device.open()?;

        // Assign handle
        let handle = DeviceHandle(self.next_handle_id);
        self.next_handle_id += 1;

        self.attached.insert(handle, (device_id, client_id.clone()));

        info!(
            "Attached device {:?} as handle {:?} for client {}",
            device_id, handle, client_id
        );

        Ok(handle)
    }

    /// Detach a device
    pub fn detach_device(&mut self, handle: DeviceHandle) -> Result<(), DetachError> {
        let (device_id, client_id) = self
            .attached
            .remove(&handle)
            .ok_or(DetachError::HandleNotFound)?;

        // Close the device
        if let Some(device) = self.get_device_by_id_mut(device_id) {
            device.close();
        }

        info!(
            "Detached handle {:?} (device {:?}) for client {}",
            handle, device_id, client_id
        );

        Ok(())
    }

    /// Get device by DeviceId
    fn get_device_by_id(&self, device_id: DeviceId) -> Option<&UsbDevice> {
        let key = self.device_ids.get(&device_id)?;
        self.devices.get(key)
    }

    /// Get mutable device by DeviceId
    fn get_device_by_id_mut(&mut self, device_id: DeviceId) -> Option<&mut UsbDevice> {
        let key = *self.device_ids.get(&device_id)?;
        self.devices.get_mut(&key)
    }

    /// Get device by handle
    pub fn get_device_by_handle(&mut self, handle: DeviceHandle) -> Option<&mut UsbDevice> {
        let (device_id, _client) = self.attached.get(&handle)?;
        self.get_device_by_id_mut(*device_id)
    }

    /// Get USB context
    pub fn context(&self) -> &Context {
        &self.context
    }
    /// Check if a device is allowed by the configured filters
    fn is_device_allowed(&self, device: &Device<Context>) -> bool {
        let desc = match device.device_descriptor() {
            Ok(d) => d,
            Err(_) => return false,
        };

        Self::check_filter(desc.vendor_id(), desc.product_id(), &self.allowed_filters)
    }

    /// Check if a VID/PID pair is allowed by the filters
    fn check_filter(vid: u16, pid: u16, filters: &[String]) -> bool {
        // If no filters are defined, all devices are allowed
        if filters.is_empty() {
            return true;
        }

        // Check if device matches any filter
        for filter in filters {
            // Filter format: "0xVID:0xPID" or "0xVID:*"
            // We assume filters are validated by config loader
            let parts: Vec<&str> = filter.split(':').collect();
            if parts.len() != 2 {
                continue;
            }

            let filter_vid_str = parts[0];
            let filter_pid_str = parts[1];

            // Check VID
            let vid_match = if filter_vid_str == "*" {
                true
            } else {
                u16::from_str_radix(filter_vid_str.trim_start_matches("0x"), 16)
                    .map(|v| v == vid)
                    .unwrap_or(false)
            };

            if !vid_match {
                continue;
            }

            // Check PID
            let pid_match = if filter_pid_str == "*" {
                true
            } else {
                u16::from_str_radix(filter_pid_str.trim_start_matches("0x"), 16)
                    .map(|p| p == pid)
                    .unwrap_or(false)
            };

            if pid_match {
                return true;
            }
        }

        false
    }
}

/// Hot-plug callback handler
///
/// This struct implements the Hotplug trait to receive notifications
/// about device arrival and removal.
struct HotplugCallback {
    event_sender: async_channel::Sender<UsbEvent>,
    // Store device info temporarily since we can't access manager in callback
    devices_cache: Arc<std::sync::Mutex<HashMap<(u8, u8), DeviceId>>>,
}

impl HotplugCallback {
    fn new(event_sender: async_channel::Sender<UsbEvent>) -> Self {
        Self {
            event_sender,
            devices_cache: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }
}

impl<T: UsbContext> Hotplug<T> for HotplugCallback {
    fn device_arrived(&mut self, device: Device<T>) {
        // We can't directly access the device manager from here
        // The actual handling is done in the worker thread via handle_device_arrived
        // This is just a notification that we received the event
        debug!(
            "Hot-plug callback: device arrived (bus={}, addr={})",
            device.bus_number(),
            device.address()
        );
    }

    fn device_left(&mut self, device: Device<T>) {
        // Similar to device_arrived, actual handling is in worker thread
        debug!(
            "Hot-plug callback: device left (bus={}, addr={})",
            device.bus_number(),
            device.address()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_id_assignment() {
        let (tx, _rx) = async_channel::bounded(1);
        let manager = DeviceManager::new(tx, vec![]).unwrap();

        assert_eq!(manager.next_device_id, 1);
        assert_eq!(manager.next_handle_id, 1);
    }

    #[test]
    fn test_filter_logic() {
        let filters = vec![
            "0x1234:0x5678".to_string(), // Exact match
            "0xABCD:*".to_string(),      // Wildcard PID
        ];

        // Should match exact
        assert!(DeviceManager::check_filter(0x1234, 0x5678, &filters));

        // Should match wildcard
        assert!(DeviceManager::check_filter(0xABCD, 0x1111, &filters));
        assert!(DeviceManager::check_filter(0xABCD, 0x9999, &filters));

        // Should not match
        assert!(!DeviceManager::check_filter(0x1234, 0x9999, &filters)); // Wrong PID
        assert!(!DeviceManager::check_filter(0x9999, 0x5678, &filters)); // Wrong VID
        assert!(!DeviceManager::check_filter(0x0000, 0x0000, &filters));

        // Empty filters = allow all
        assert!(DeviceManager::check_filter(0x1234, 0x5678, &[]));
    }

    #[test]
    fn test_device_handle_assignment() {
        let id1 = DeviceHandle(1);
        let id2 = DeviceHandle(2);
        assert_ne!(id1, id2);
    }
}
