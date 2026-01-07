//! Device sharing and access tracking
//!
//! Manages multi-client device access, including:
//! - Tracking which clients have attached to which devices
//! - Enforcing sharing mode policies (exclusive, shared, read-only)
//! - Managing lock acquisition and release queues
//! - Notifying clients of queue position changes

use protocol::{
    DeviceHandle, DeviceId, DeviceSharingStatus, LockResult, SharingMode, UnlockResult,
};
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// Information about an attached client for a device
#[derive(Debug, Clone)]
pub struct AttachedClient {
    /// Client identifier (EndpointId string)
    pub client_id: String,
    /// Device handle assigned to this client
    pub handle: DeviceHandle,
    /// Whether this client has the write lock (for ReadOnly mode)
    pub has_write_lock: bool,
    /// Whether this client currently holds exclusive access (for Shared mode)
    pub has_access: bool,
    /// When this client attached
    pub attached_at: Instant,
    /// When this client acquired the lock (if any)
    pub lock_acquired_at: Option<Instant>,
}

/// Queue entry for clients waiting for access
#[derive(Debug, Clone)]
pub struct QueueEntry {
    /// Client identifier
    pub client_id: String,
    /// Device handle
    pub handle: DeviceHandle,
    /// Whether requesting write access
    pub wants_write: bool,
    /// When the request was made
    pub requested_at: Instant,
    /// Timeout for this request (None = wait forever)
    pub timeout: Option<Duration>,
}

/// Device sharing state
#[derive(Debug)]
pub struct DeviceSharingState {
    /// Device ID
    pub device_id: DeviceId,
    /// Sharing mode for this device
    pub mode: SharingMode,
    /// Maximum concurrent clients allowed (for Shared mode)
    pub max_clients: u32,
    /// Lock timeout duration
    pub lock_timeout: Duration,
    /// Currently attached clients
    pub attached_clients: HashMap<DeviceHandle, AttachedClient>,
    /// Queue of clients waiting for access
    pub access_queue: VecDeque<QueueEntry>,
    /// Client that holds the write lock (for ReadOnly mode)
    pub write_lock_holder: Option<DeviceHandle>,
    /// Client that holds exclusive access (for Shared mode with lock)
    pub exclusive_lock_holder: Option<DeviceHandle>,
}

impl DeviceSharingState {
    /// Create a new device sharing state
    pub fn new(
        device_id: DeviceId,
        mode: SharingMode,
        max_clients: u32,
        lock_timeout: Duration,
    ) -> Self {
        Self {
            device_id,
            mode,
            max_clients,
            lock_timeout,
            attached_clients: HashMap::new(),
            access_queue: VecDeque::new(),
            write_lock_holder: None,
            exclusive_lock_holder: None,
        }
    }

    /// Check if a new client can attach to this device
    pub fn can_attach(&self) -> bool {
        match self.mode {
            SharingMode::Exclusive => self.attached_clients.is_empty(),
            SharingMode::Shared => (self.attached_clients.len() as u32) < self.max_clients,
            SharingMode::ReadOnly => (self.attached_clients.len() as u32) < self.max_clients,
        }
    }

    /// Register a new client attachment
    pub fn attach_client(&mut self, client_id: String, handle: DeviceHandle) {
        let client = AttachedClient {
            client_id: client_id.clone(),
            handle,
            has_write_lock: false,
            has_access: self.mode == SharingMode::Exclusive || self.attached_clients.is_empty(),
            attached_at: Instant::now(),
            lock_acquired_at: if self.mode == SharingMode::Exclusive {
                Some(Instant::now())
            } else {
                None
            },
        };

        debug!(
            "Client {} attached to device {:?} with handle {:?}, mode: {}",
            client_id, self.device_id, handle, self.mode
        );

        self.attached_clients.insert(handle, client);
    }

    /// Remove a client from the device
    pub fn detach_client(&mut self, handle: DeviceHandle) -> Option<AttachedClient> {
        let client = self.attached_clients.remove(&handle);

        if let Some(ref c) = client {
            debug!(
                "Client {} detached from device {:?}",
                c.client_id, self.device_id
            );

            // Release any locks held by this client
            if self.write_lock_holder == Some(handle) {
                self.write_lock_holder = None;
            }
            if self.exclusive_lock_holder == Some(handle) {
                self.exclusive_lock_holder = None;
            }

            // Remove from queue if present
            self.access_queue.retain(|e| e.handle != handle);
        }

        client
    }

    /// Attempt to acquire a lock for a client
    pub fn acquire_lock(&mut self, handle: DeviceHandle, write_access: bool) -> LockResult {
        let client = match self.attached_clients.get_mut(&handle) {
            Some(c) => c,
            None => {
                return LockResult::NotAvailable {
                    reason: "Client not attached".to_string(),
                };
            }
        };

        match self.mode {
            SharingMode::Exclusive => {
                // In exclusive mode, the attached client always has the lock
                if client.has_access {
                    LockResult::AlreadyHeld
                } else {
                    LockResult::NotAvailable {
                        reason: "Device in exclusive mode with another client".to_string(),
                    }
                }
            }
            SharingMode::Shared => {
                // In shared mode, clients can request exclusive access
                if self.exclusive_lock_holder == Some(handle) {
                    return LockResult::AlreadyHeld;
                }

                if self.exclusive_lock_holder.is_none() {
                    // No one has the lock, grant it
                    self.exclusive_lock_holder = Some(handle);
                    client.has_access = true;
                    client.lock_acquired_at = Some(Instant::now());
                    info!(
                        "Client {} acquired exclusive lock on device {:?}",
                        client.client_id, self.device_id
                    );
                    LockResult::Acquired
                } else {
                    // Someone else has the lock, add to queue
                    let position = self.add_to_queue(handle, write_access);
                    LockResult::Queued { position }
                }
            }
            SharingMode::ReadOnly => {
                if write_access {
                    // Requesting write lock
                    if self.write_lock_holder == Some(handle) {
                        return LockResult::AlreadyHeld;
                    }

                    if self.write_lock_holder.is_none() {
                        // No one has the write lock, grant it
                        self.write_lock_holder = Some(handle);
                        client.has_write_lock = true;
                        client.lock_acquired_at = Some(Instant::now());
                        info!(
                            "Client {} acquired write lock on device {:?}",
                            client.client_id, self.device_id
                        );
                        LockResult::Acquired
                    } else {
                        // Someone else has the write lock, add to queue
                        let position = self.add_to_queue(handle, true);
                        LockResult::Queued { position }
                    }
                } else {
                    // Read-only access is always available in ReadOnly mode
                    LockResult::Acquired
                }
            }
        }
    }

    /// Release a lock held by a client
    pub fn release_lock(&mut self, handle: DeviceHandle) -> UnlockResult {
        let client = match self.attached_clients.get_mut(&handle) {
            Some(c) => c,
            None => {
                return UnlockResult::Error {
                    message: "Client not attached".to_string(),
                };
            }
        };

        match self.mode {
            SharingMode::Exclusive => {
                // Can't release lock in exclusive mode - must detach
                UnlockResult::Error {
                    message: "Cannot release lock in exclusive mode".to_string(),
                }
            }
            SharingMode::Shared => {
                if self.exclusive_lock_holder == Some(handle) {
                    self.exclusive_lock_holder = None;
                    client.has_access = false;
                    client.lock_acquired_at = None;
                    info!(
                        "Client {} released exclusive lock on device {:?}",
                        client.client_id, self.device_id
                    );
                    UnlockResult::Released
                } else {
                    UnlockResult::NotHeld
                }
            }
            SharingMode::ReadOnly => {
                if self.write_lock_holder == Some(handle) {
                    self.write_lock_holder = None;
                    client.has_write_lock = false;
                    client.lock_acquired_at = None;
                    info!(
                        "Client {} released write lock on device {:?}",
                        client.client_id, self.device_id
                    );
                    UnlockResult::Released
                } else {
                    UnlockResult::NotHeld
                }
            }
        }
    }

    /// Add a client to the access queue
    fn add_to_queue(&mut self, handle: DeviceHandle, wants_write: bool) -> u32 {
        // Check if already in queue
        if let Some(pos) = self.access_queue.iter().position(|e| e.handle == handle) {
            return (pos + 1) as u32;
        }

        let client = self.attached_clients.get(&handle);
        let client_id = client.map(|c| c.client_id.clone()).unwrap_or_default();

        let entry = QueueEntry {
            client_id,
            handle,
            wants_write,
            requested_at: Instant::now(),
            timeout: Some(self.lock_timeout),
        };

        self.access_queue.push_back(entry);
        self.access_queue.len() as u32
    }

    /// Get the queue position for a client (0 if has access)
    pub fn get_queue_position(&self, handle: DeviceHandle) -> u32 {
        // Check if client has access
        if let Some(client) = self.attached_clients.get(&handle) {
            if client.has_access || client.has_write_lock {
                return 0;
            }
        }

        // Check position in queue
        if let Some(pos) = self.access_queue.iter().position(|e| e.handle == handle) {
            return (pos + 1) as u32;
        }

        // Not in queue, not attached
        0
    }

    /// Process the queue and grant access to the next waiting client
    ///
    /// Returns the handle of the client that was granted access, if any.
    pub fn process_queue(&mut self) -> Option<DeviceHandle> {
        // Check if lock is available
        let lock_available = match self.mode {
            SharingMode::Exclusive => false, // No queue processing in exclusive mode
            SharingMode::Shared => self.exclusive_lock_holder.is_none(),
            SharingMode::ReadOnly => self.write_lock_holder.is_none(),
        };

        if !lock_available || self.access_queue.is_empty() {
            return None;
        }

        // Find the first non-expired entry
        let now = Instant::now();
        while let Some(entry) = self.access_queue.front() {
            if let Some(timeout) = entry.timeout {
                if now.duration_since(entry.requested_at) > timeout {
                    // Entry expired, remove it
                    warn!(
                        "Queue entry for client {} expired after {:?}",
                        entry.client_id, timeout
                    );
                    self.access_queue.pop_front();
                    continue;
                }
            }
            break;
        }

        // Grant access to the first valid entry
        if let Some(entry) = self.access_queue.pop_front() {
            let handle = entry.handle;

            if let Some(client) = self.attached_clients.get_mut(&handle) {
                match self.mode {
                    SharingMode::Shared => {
                        self.exclusive_lock_holder = Some(handle);
                        client.has_access = true;
                        client.lock_acquired_at = Some(Instant::now());
                    }
                    SharingMode::ReadOnly if entry.wants_write => {
                        self.write_lock_holder = Some(handle);
                        client.has_write_lock = true;
                        client.lock_acquired_at = Some(Instant::now());
                    }
                    _ => {}
                }

                info!(
                    "Granted access to queued client {} for device {:?}",
                    client.client_id, self.device_id
                );

                return Some(handle);
            }
        }

        None
    }

    /// Check for expired locks and release them
    ///
    /// Returns the handle of the client whose lock was expired, if any.
    pub fn check_lock_timeout(&mut self) -> Option<DeviceHandle> {
        let now = Instant::now();

        // Check exclusive lock timeout
        if let Some(holder) = self.exclusive_lock_holder {
            if let Some(client) = self.attached_clients.get(&holder) {
                if let Some(acquired_at) = client.lock_acquired_at {
                    if now.duration_since(acquired_at) > self.lock_timeout {
                        info!(
                            "Lock expired for client {} on device {:?}",
                            client.client_id, self.device_id
                        );
                        // Will be released by caller
                        return Some(holder);
                    }
                }
            }
        }

        // Check write lock timeout
        if let Some(holder) = self.write_lock_holder {
            if let Some(client) = self.attached_clients.get(&holder) {
                if let Some(acquired_at) = client.lock_acquired_at {
                    if now.duration_since(acquired_at) > self.lock_timeout {
                        info!(
                            "Write lock expired for client {} on device {:?}",
                            client.client_id, self.device_id
                        );
                        return Some(holder);
                    }
                }
            }
        }

        None
    }

    /// Get the sharing status for this device (from a specific client's perspective)
    pub fn get_status(&self, for_handle: Option<DeviceHandle>) -> DeviceSharingStatus {
        let (has_write_lock, queue_position) = if let Some(handle) = for_handle {
            let has_write = self.write_lock_holder == Some(handle);
            let position = self.get_queue_position(handle);
            (has_write, position)
        } else {
            (false, 0)
        };

        DeviceSharingStatus {
            device_id: self.device_id,
            sharing_mode: self.mode,
            attached_clients: self.attached_clients.len() as u32,
            has_write_lock,
            queue_position,
            queue_length: self.access_queue.len() as u32
                + if self.exclusive_lock_holder.is_some() || self.write_lock_holder.is_some() {
                    1
                } else {
                    0
                },
        }
    }

    /// Get all queue positions for notification purposes
    pub fn get_all_queue_positions(&self) -> Vec<(DeviceHandle, u32)> {
        self.access_queue
            .iter()
            .enumerate()
            .map(|(i, e)| (e.handle, (i + 1) as u32))
            .collect()
    }
}

/// Device access tracker - manages sharing state for all devices
#[derive(Debug, Default)]
pub struct DeviceAccessTracker {
    /// Sharing state per device
    devices: HashMap<DeviceId, DeviceSharingState>,
}

impl DeviceAccessTracker {
    /// Create a new device access tracker
    pub fn new() -> Self {
        Self {
            devices: HashMap::new(),
        }
    }

    /// Register a device with sharing configuration
    pub fn register_device(
        &mut self,
        device_id: DeviceId,
        mode: SharingMode,
        max_clients: u32,
        lock_timeout: Duration,
    ) {
        let state = DeviceSharingState::new(device_id, mode, max_clients, lock_timeout);
        self.devices.insert(device_id, state);
        debug!(
            "Registered device {:?} with mode: {}, max_clients: {}",
            device_id, mode, max_clients
        );
    }

    /// Unregister a device (e.g., when unplugged)
    pub fn unregister_device(&mut self, device_id: DeviceId) -> Option<DeviceSharingState> {
        let state = self.devices.remove(&device_id);
        if state.is_some() {
            debug!("Unregistered device {:?}", device_id);
        }
        state
    }

    /// Get sharing state for a device
    pub fn get_state(&self, device_id: DeviceId) -> Option<&DeviceSharingState> {
        self.devices.get(&device_id)
    }

    /// Get mutable sharing state for a device
    pub fn get_state_mut(&mut self, device_id: DeviceId) -> Option<&mut DeviceSharingState> {
        self.devices.get_mut(&device_id)
    }

    /// Check if a device is registered
    pub fn is_registered(&self, device_id: DeviceId) -> bool {
        self.devices.contains_key(&device_id)
    }

    /// Get the sharing mode for a device
    pub fn get_mode(&self, device_id: DeviceId) -> Option<SharingMode> {
        self.devices.get(&device_id).map(|s| s.mode)
    }

    /// Check if a client can attach to a device
    pub fn can_attach(&self, device_id: DeviceId) -> bool {
        self.devices
            .get(&device_id)
            .map(|s| s.can_attach())
            .unwrap_or(true) // If not registered, allow (will use default mode)
    }

    /// Get the device ID associated with a handle
    pub fn get_device_for_handle(&self, handle: DeviceHandle) -> Option<DeviceId> {
        for (device_id, state) in &self.devices {
            if state.attached_clients.contains_key(&handle) {
                return Some(*device_id);
            }
        }
        None
    }

    /// Process all queues and check for lock timeouts
    ///
    /// Returns a list of (device_id, handle, event) for each event that occurred.
    pub fn process_all(&mut self) -> Vec<SharingEvent> {
        let mut events = Vec::new();

        for (device_id, state) in &mut self.devices {
            // Check for lock timeouts
            if let Some(expired_handle) = state.check_lock_timeout() {
                // Force release the lock
                let _ = state.release_lock(expired_handle);
                events.push(SharingEvent::LockExpired {
                    device_id: *device_id,
                    handle: expired_handle,
                });
            }

            // Process queue after potential lock release
            if let Some(granted_handle) = state.process_queue() {
                events.push(SharingEvent::AccessGranted {
                    device_id: *device_id,
                    handle: granted_handle,
                });

                // Update queue positions for remaining clients
                for (handle, position) in state.get_all_queue_positions() {
                    events.push(SharingEvent::QueuePositionChanged {
                        device_id: *device_id,
                        handle,
                        new_position: position,
                    });
                }
            }
        }

        events
    }
}

/// Events generated by the sharing system
#[derive(Debug, Clone)]
pub enum SharingEvent {
    /// A client's lock expired
    LockExpired {
        device_id: DeviceId,
        handle: DeviceHandle,
    },
    /// A client was granted access from the queue
    AccessGranted {
        device_id: DeviceId,
        handle: DeviceHandle,
    },
    /// A client's queue position changed
    QueuePositionChanged {
        device_id: DeviceId,
        handle: DeviceHandle,
        new_position: u32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exclusive_mode() {
        let device_id = DeviceId(1);
        let mut state = DeviceSharingState::new(
            device_id,
            SharingMode::Exclusive,
            1,
            Duration::from_secs(300),
        );

        // First client can attach
        assert!(state.can_attach());
        state.attach_client("client1".to_string(), DeviceHandle(1));

        // Second client cannot attach
        assert!(!state.can_attach());

        // First client has access
        let status = state.get_status(Some(DeviceHandle(1)));
        assert_eq!(status.attached_clients, 1);
        assert_eq!(status.queue_position, 0);

        // After detach, new client can attach
        state.detach_client(DeviceHandle(1));
        assert!(state.can_attach());
    }

    #[test]
    fn test_shared_mode_basic() {
        let device_id = DeviceId(1);
        let mut state =
            DeviceSharingState::new(device_id, SharingMode::Shared, 4, Duration::from_secs(300));

        // Multiple clients can attach
        state.attach_client("client1".to_string(), DeviceHandle(1));
        state.attach_client("client2".to_string(), DeviceHandle(2));
        assert!(state.can_attach());

        // First client can acquire lock
        assert_eq!(
            state.acquire_lock(DeviceHandle(1), false),
            LockResult::Acquired
        );

        // Second client gets queued
        let result = state.acquire_lock(DeviceHandle(2), false);
        assert!(matches!(result, LockResult::Queued { position: 1 }));

        // Release lock
        assert_eq!(state.release_lock(DeviceHandle(1)), UnlockResult::Released);

        // Process queue - second client gets access
        let granted = state.process_queue();
        assert_eq!(granted, Some(DeviceHandle(2)));
    }

    #[test]
    fn test_read_only_mode() {
        let device_id = DeviceId(1);
        let mut state = DeviceSharingState::new(
            device_id,
            SharingMode::ReadOnly,
            4,
            Duration::from_secs(300),
        );

        state.attach_client("client1".to_string(), DeviceHandle(1));
        state.attach_client("client2".to_string(), DeviceHandle(2));

        // Read access is always granted
        assert_eq!(
            state.acquire_lock(DeviceHandle(1), false),
            LockResult::Acquired
        );
        assert_eq!(
            state.acquire_lock(DeviceHandle(2), false),
            LockResult::Acquired
        );

        // First write request succeeds
        assert_eq!(
            state.acquire_lock(DeviceHandle(1), true),
            LockResult::Acquired
        );

        // Second write request gets queued
        let result = state.acquire_lock(DeviceHandle(2), true);
        assert!(matches!(result, LockResult::Queued { position: 1 }));

        // Release write lock
        assert_eq!(state.release_lock(DeviceHandle(1)), UnlockResult::Released);
    }

    #[test]
    fn test_device_access_tracker() {
        let mut tracker = DeviceAccessTracker::new();

        let device_id = DeviceId(1);
        tracker.register_device(device_id, SharingMode::Shared, 4, Duration::from_secs(300));

        assert!(tracker.is_registered(device_id));
        assert_eq!(tracker.get_mode(device_id), Some(SharingMode::Shared));
        assert!(tracker.can_attach(device_id));

        // Register attachment
        if let Some(state) = tracker.get_state_mut(device_id) {
            state.attach_client("client1".to_string(), DeviceHandle(1));
        }

        // Find device by handle
        assert_eq!(
            tracker.get_device_for_handle(DeviceHandle(1)),
            Some(device_id)
        );
        assert_eq!(tracker.get_device_for_handle(DeviceHandle(999)), None);

        // Unregister
        tracker.unregister_device(device_id);
        assert!(!tracker.is_registered(device_id));
    }
}
