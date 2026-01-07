//! USB device manager
//!
//! Handles device enumeration, hot-plug events, and device state tracking.
//! This module runs in the USB thread and manages the device registry.

use crate::usb::device::UsbDevice;
use crate::usb::sharing::{DeviceAccessTracker, SharingEvent};
use common::UsbEvent;
use protocol::{
    AttachError, DetachError, DeviceHandle, DeviceId, DeviceInfo, DeviceSharingStatus, LockResult,
    SharingMode, UnlockResult,
};
use rusb::{Context, Device, Hotplug, HotplugBuilder, Registration, UsbContext};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

/// Debounce duration for USB hotplug events (500ms)
const HOTPLUG_DEBOUNCE_DURATION: Duration = Duration::from_millis(500);

/// Type of pending hotplug event
#[derive(Debug, Clone)]
pub enum PendingHotplugEvent {
    /// Device arrived
    Arrived,
    /// Device left
    Left,
}

/// A pending debounced hotplug event
#[derive(Debug, Clone)]
pub struct DebouncedEvent {
    /// Type of event
    pub event_type: PendingHotplugEvent,
    /// When this event should fire (after debounce period)
    pub fire_at: Instant,
    /// Bus number
    pub bus: u8,
    /// Device address
    pub address: u8,
}

/// Shared debounce state between HotplugCallback and DeviceManager
pub type DebounceState = Arc<std::sync::Mutex<HashMap<(u8, u8), DebouncedEvent>>>;

/// Sharing configuration for DeviceManager
#[derive(Debug, Clone)]
pub struct SharingConfig {
    /// Default sharing mode for devices without a specific policy
    pub default_mode: SharingMode,
    /// Default lock timeout in seconds
    pub default_lock_timeout_secs: u32,
    /// Default max concurrent clients
    pub default_max_clients: u32,
}

impl Default for SharingConfig {
    fn default() -> Self {
        Self {
            default_mode: SharingMode::Exclusive,
            default_lock_timeout_secs: 300, // 5 minutes
            default_max_clients: 4,
        }
    }
}

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
    /// Shared debounce state for hotplug events
    debounce_state: DebounceState,
    /// Device access tracker for multi-client sharing
    access_tracker: DeviceAccessTracker,
    /// Sharing configuration
    sharing_config: SharingConfig,
}

impl DeviceManager {
    /// Create a new device manager
    pub fn new(
        event_sender: async_channel::Sender<UsbEvent>,
        allowed_filters: Vec<String>,
    ) -> Result<Self, rusb::Error> {
        Self::with_sharing_config(event_sender, allowed_filters, SharingConfig::default())
    }

    /// Create a new device manager with sharing configuration
    pub fn with_sharing_config(
        event_sender: async_channel::Sender<UsbEvent>,
        allowed_filters: Vec<String>,
        sharing_config: SharingConfig,
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
            debounce_state: Arc::new(std::sync::Mutex::new(HashMap::new())),
            access_tracker: DeviceAccessTracker::new(),
            sharing_config,
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
        let debounce_state = Arc::clone(&self.debounce_state);

        // Create hotplug callback with shared debounce state
        let callback = HotplugCallback::new(debounce_state);

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

        // Register device with access tracker using default sharing config
        self.access_tracker.register_device(
            device_id,
            self.sharing_config.default_mode,
            self.sharing_config.default_max_clients,
            Duration::from_secs(self.sharing_config.default_lock_timeout_secs as u64),
        );

        Ok(device_id)
    }

    /// Remove a device from the registry
    ///
    /// Returns the DeviceId and any invalidated handles with their client IDs.
    fn remove_device(
        &mut self,
        bus: u8,
        address: u8,
    ) -> Option<(DeviceId, Vec<DeviceHandle>, Vec<String>)> {
        let key = (bus, address);

        if let Some(device) = self.devices.remove(&key) {
            let device_id = device.id();
            self.device_ids.remove(&device_id);

            // Collect invalidated handles and affected clients before removing
            let mut invalidated_handles = Vec::new();
            let mut affected_clients = Vec::new();

            self.attached.retain(|handle, (id, client)| {
                if *id == device_id {
                    invalidated_handles.push(*handle);
                    if !affected_clients.contains(client) {
                        affected_clients.push(client.clone());
                    }
                    false // Remove this entry
                } else {
                    true // Keep this entry
                }
            });

            // Unregister from access tracker
            self.access_tracker.unregister_device(device_id);

            debug!(
                "Removed device {:?}: bus={}, addr={}, invalidated {} handles for {} clients",
                device_id,
                bus,
                address,
                invalidated_handles.len(),
                affected_clients.len()
            );

            Some((device_id, invalidated_handles, affected_clients))
        } else {
            None
        }
    }

    /// Handle device removal (from hot-plug callback)
    fn handle_device_left_internal(&mut self, bus: u8, address: u8) {
        if let Some((device_id, invalidated_handles, affected_clients)) =
            self.remove_device(bus, address)
        {
            info!(
                "Device {:?} left: {} handles invalidated, {} clients affected",
                device_id,
                invalidated_handles.len(),
                affected_clients.len()
            );

            if let Err(e) = self.event_sender.send_blocking(UsbEvent::DeviceLeft {
                device_id,
                invalidated_handles,
                affected_clients,
            }) {
                error!("Failed to send DeviceLeft event: {}", e);
            }
        }
    }

    /// Handle device arrival (from hot-plug callback) - internal implementation
    fn handle_device_arrived_internal(&mut self, bus: u8, address: u8) {
        let devices = match self.context.devices() {
            Ok(d) => d,
            Err(e) => {
                warn!("Failed to enumerate devices for arrival: {}", e);
                return;
            }
        };

        for device in devices.iter() {
            if device.bus_number() == bus && device.address() == address {
                match self.add_device(device) {
                    Ok(device_id) => {
                        if let Some(usb_device) = self.get_device_by_id(device_id) {
                            let device_info = usb_device.device_info();

                            if let Err(e) =
                                self.event_sender.send_blocking(UsbEvent::DeviceArrived {
                                    device: device_info,
                                })
                            {
                                error!("Failed to send DeviceArrived event: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Failed to add arrived device: {}", e);
                    }
                }
                return;
            }
        }
        debug!(
            "Device arrived but not found in enumeration: bus={}, addr={}",
            bus, address
        );
    }

    /// Process any debounced hotplug events that are ready to fire
    ///
    /// This should be called periodically from the USB worker event loop.
    /// Returns the number of events processed.
    pub fn process_debounced_events(&mut self) -> usize {
        let now = Instant::now();
        let mut events_to_process = Vec::new();

        // Collect events that are ready to fire
        {
            let mut state = self.debounce_state.lock().unwrap();
            let ready_keys: Vec<(u8, u8)> = state
                .iter()
                .filter(|(_, event)| now >= event.fire_at)
                .map(|(key, _)| *key)
                .collect();

            for key in ready_keys {
                if let Some(event) = state.remove(&key) {
                    events_to_process.push(event);
                }
            }
        }

        let count = events_to_process.len();

        // Process each ready event
        for event in events_to_process {
            info!(
                "Debounce timer fired for device bus={}, addr={}: {:?}",
                event.bus, event.address, event.event_type
            );

            match event.event_type {
                PendingHotplugEvent::Arrived => {
                    self.handle_device_arrived_internal(event.bus, event.address);
                }
                PendingHotplugEvent::Left => {
                    self.handle_device_left_internal(event.bus, event.address);
                }
            }
        }

        count
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
        // Check if device exists first
        if !self.device_ids.contains_key(&device_id) {
            return Err(AttachError::DeviceNotFound);
        }

        // Check sharing policy via access tracker
        if !self.access_tracker.can_attach(device_id) {
            // Get the sharing mode to provide a better error message
            let mode = self.access_tracker.get_mode(device_id);
            return Err(AttachError::PolicyDenied {
                reason: match mode {
                    Some(SharingMode::Exclusive) => {
                        "Device is in exclusive mode and already attached".to_string()
                    }
                    Some(SharingMode::Shared) | Some(SharingMode::ReadOnly) => {
                        "Maximum concurrent clients reached".to_string()
                    }
                    None => "Device not available for sharing".to_string(),
                },
            });
        }

        // Legacy check: For Exclusive mode, also check our old attached map
        // (This is a fallback - access_tracker.can_attach should handle this)
        if self.sharing_config.default_mode == SharingMode::Exclusive {
            if self.attached.values().any(|(id, _)| *id == device_id) {
                return Err(AttachError::AlreadyAttached);
            }
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

        // Register attachment with access tracker
        if let Some(state) = self.access_tracker.get_state_mut(device_id) {
            state.attach_client(client_id.clone(), handle);
        }

        self.attached.insert(handle, (device_id, client_id.clone()));

        info!(
            "Attached device {:?} as handle {:?} for client {}",
            device_id, handle, client_id
        );

        Ok(handle)
    }

    /// Detach a device
    ///
    /// Returns the device_id that was detached (for queue processing)
    pub fn detach_device(&mut self, handle: DeviceHandle) -> Result<(), DetachError> {
        let (device_id, client_id) = self
            .attached
            .remove(&handle)
            .ok_or(DetachError::HandleNotFound)?;

        // Detach from access tracker
        if let Some(state) = self.access_tracker.get_state_mut(device_id) {
            state.detach_client(handle);
        }

        // Close the device only if no other clients are attached
        let other_clients = self.attached.values().any(|(id, _)| *id == device_id);
        if !other_clients {
            if let Some(device) = self.get_device_by_id_mut(device_id) {
                device.close();
            }
        }

        info!(
            "Detached handle {:?} (device {:?}) for client {}",
            handle, device_id, client_id
        );

        Ok(())
    }

    /// Process the queue for a device after detachment
    ///
    /// Should be called after detach_device to grant access to waiting clients.
    /// Returns sharing events to send to clients.
    pub fn process_device_queue(&mut self, device_id: DeviceId) -> Vec<SharingEvent> {
        let mut events = Vec::new();

        if let Some(state) = self.access_tracker.get_state_mut(device_id) {
            // Process the queue
            if let Some(granted_handle) = state.process_queue() {
                events.push(SharingEvent::AccessGranted {
                    device_id,
                    handle: granted_handle,
                });

                // Update queue positions for remaining clients
                for (handle, position) in state.get_all_queue_positions() {
                    events.push(SharingEvent::QueuePositionChanged {
                        device_id,
                        handle,
                        new_position: position,
                    });
                }
            }
        }

        events
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

    /// Get sharing status for a device
    pub fn get_sharing_status(
        &self,
        device_id: DeviceId,
        handle: Option<DeviceHandle>,
    ) -> Result<DeviceSharingStatus, AttachError> {
        // Check if device exists
        if !self.device_ids.contains_key(&device_id) {
            return Err(AttachError::DeviceNotFound);
        }

        // Get status from access tracker
        if let Some(state) = self.access_tracker.get_state(device_id) {
            Ok(state.get_status(handle))
        } else {
            // Device exists but not registered with tracker - return default status
            Ok(DeviceSharingStatus {
                device_id,
                sharing_mode: self.sharing_config.default_mode,
                attached_clients: 0,
                has_write_lock: false,
                queue_position: 0,
                queue_length: 0,
            })
        }
    }

    /// Acquire a lock on a device
    pub fn acquire_lock(&mut self, handle: DeviceHandle, write_access: bool) -> LockResult {
        // Find the device for this handle
        let device_id = match self.access_tracker.get_device_for_handle(handle) {
            Some(id) => id,
            None => {
                // Try to get device_id from attached map
                match self.attached.get(&handle) {
                    Some((id, _)) => *id,
                    None => {
                        return LockResult::NotAvailable {
                            reason: "Handle not found".to_string(),
                        };
                    }
                }
            }
        };

        // Acquire lock via access tracker
        if let Some(state) = self.access_tracker.get_state_mut(device_id) {
            state.acquire_lock(handle, write_access)
        } else {
            LockResult::NotAvailable {
                reason: "Device not registered for sharing".to_string(),
            }
        }
    }

    /// Release a lock on a device
    pub fn release_lock(&mut self, handle: DeviceHandle) -> UnlockResult {
        // Find the device for this handle
        let device_id = match self.access_tracker.get_device_for_handle(handle) {
            Some(id) => id,
            None => {
                // Try to get device_id from attached map
                match self.attached.get(&handle) {
                    Some((id, _)) => *id,
                    None => {
                        return UnlockResult::Error {
                            message: "Handle not found".to_string(),
                        };
                    }
                }
            }
        };

        // Release lock via access tracker
        if let Some(state) = self.access_tracker.get_state_mut(device_id) {
            state.release_lock(handle)
        } else {
            UnlockResult::Error {
                message: "Device not registered for sharing".to_string(),
            }
        }
    }

    /// Process all device queues and check for lock timeouts
    ///
    /// Returns sharing events to send to clients.
    pub fn process_all_queues(&mut self) -> Vec<SharingEvent> {
        self.access_tracker.process_all()
    }

    /// Get the device ID for a handle
    pub fn get_device_id_for_handle(&self, handle: DeviceHandle) -> Option<DeviceId> {
        self.attached.get(&handle).map(|(id, _)| *id)
    }

    /// Get client ID for a handle
    pub fn get_client_id_for_handle(&self, handle: DeviceHandle) -> Option<String> {
        self.attached.get(&handle).map(|(_, client)| client.clone())
    }

    /// Get the sharing mode for a device
    pub fn get_sharing_mode(&self, device_id: DeviceId) -> Option<SharingMode> {
        self.access_tracker.get_mode(device_id)
    }
}

/// Hot-plug callback handler
///
/// This struct implements the Hotplug trait to receive notifications
/// about device arrival and removal. Events are debounced by 500ms per device
/// to handle rapid plug/unplug cycles gracefully.
struct HotplugCallback {
    /// Shared debounce state with DeviceManager
    debounce_state: DebounceState,
}

impl HotplugCallback {
    fn new(debounce_state: DebounceState) -> Self {
        Self { debounce_state }
    }

    /// Schedule a debounced event for a device
    ///
    /// If an event for this device is already pending, it will be replaced
    /// (the timer is reset). This handles rapid plug/unplug cycles by only
    /// processing the final state after 500ms of stability.
    fn schedule_debounced_event(&self, bus: u8, address: u8, event_type: PendingHotplugEvent) {
        let key = (bus, address);
        let fire_at = Instant::now() + HOTPLUG_DEBOUNCE_DURATION;

        let mut state = self.debounce_state.lock().unwrap();

        // Check if there's an existing pending event for this device
        if let Some(existing) = state.get(&key) {
            debug!(
                "Debounce: replacing pending {:?} event with {:?} for bus={}, addr={}",
                existing.event_type, event_type, bus, address
            );
        } else {
            debug!(
                "Debounce: scheduling {:?} event for bus={}, addr={} (fires in 500ms)",
                event_type, bus, address
            );
        }

        // Insert or replace the pending event
        state.insert(
            key,
            DebouncedEvent {
                event_type,
                fire_at,
                bus,
                address,
            },
        );
    }
}

impl<T: UsbContext> Hotplug<T> for HotplugCallback {
    fn device_arrived(&mut self, device: Device<T>) {
        let bus = device.bus_number();
        let address = device.address();

        debug!(
            "Hot-plug callback: device arrived (bus={}, addr={})",
            bus, address
        );

        // Schedule debounced arrival event
        self.schedule_debounced_event(bus, address, PendingHotplugEvent::Arrived);
    }

    fn device_left(&mut self, device: Device<T>) {
        let bus = device.bus_number();
        let address = device.address();

        debug!(
            "Hot-plug callback: device left (bus={}, addr={})",
            bus, address
        );

        // Schedule debounced removal event
        self.schedule_debounced_event(bus, address, PendingHotplugEvent::Left);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_id_assignment() {
        let (tx, _rx) = async_channel::bounded(1);
        // This test might fail in environments without USB access (like CI or Sandbox)
        // We catch the error and skip if USB context initialization fails
        match DeviceManager::new(tx, vec![]) {
            Ok(manager) => {
                assert_eq!(manager.next_device_id, 1);
                assert_eq!(manager.next_handle_id, 1);
            }
            Err(_) => {
                println!(
                    "Skipping test_device_id_assignment: USB context initialization failed (expected in sandbox)"
                );
            }
        }
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
