//! Quality of Service (QoS) module for USB transfer prioritization
//!
//! Provides priority-based scheduling and fair bandwidth allocation for USB transfers.
//! Supports:
//! - Priority levels based on device class and transfer type
//! - Priority queue for pending transfers
//! - Fair scheduling between clients
//! - Integration with rate limiting

use protocol::{DeviceHandle, DeviceInfo, RequestId, TransferType, UsbRequest};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// QoS priority levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Priority {
    /// Lowest priority - background/batch operations
    Low = 0,
    /// Normal priority - standard bulk transfers
    Medium = 1,
    /// High priority - control transfers, HID devices
    High = 2,
    /// Critical priority - system-level operations
    Critical = 3,
}

impl Priority {
    /// Get priority for a device class
    pub fn for_device_class(class: u8) -> Self {
        match class {
            0x03 => Self::High,   // HID (Human Interface Device)
            0x08 => Self::Medium, // Mass Storage
            0x07 => Self::Medium, // Printer
            0x02 => Self::Medium, // Communications/CDC
            0x0A => Self::Medium, // CDC Data
            0x01 => Self::High,   // Audio (low latency needed)
            0x0E => Self::High,   // Video (low latency needed)
            0x0B => Self::Medium, // Smart Card
            0xE0 => Self::High,   // Wireless Controller
            0xFE => Self::Low,    // Application Specific
            0xFF => Self::Low,    // Vendor Specific
            _ => Self::Medium,    // Default
        }
    }

    /// Get priority for a transfer type
    pub fn for_transfer_type(transfer: &TransferType) -> Self {
        match transfer {
            TransferType::Control { .. } => Self::High,
            TransferType::Interrupt { .. } => Self::High,
            TransferType::Bulk { .. } => Self::Medium,
            TransferType::Isochronous { .. } => Self::High, // Time-sensitive
        }
    }

    /// Combine device and transfer priorities (take the higher one)
    pub fn combined(device_priority: Self, transfer_priority: Self) -> Self {
        if device_priority >= transfer_priority {
            device_priority
        } else {
            transfer_priority
        }
    }
}

impl Default for Priority {
    fn default() -> Self {
        Self::Medium
    }
}

/// A prioritized transfer request
#[derive(Debug, Clone)]
pub struct PrioritizedRequest {
    /// The USB request
    pub request: UsbRequest,
    /// Client identifier
    pub client_id: String,
    /// Assigned priority
    pub priority: Priority,
    /// Timestamp when the request was queued
    pub queued_at: Instant,
    /// Estimated transfer size in bytes
    pub estimated_bytes: u64,
}

impl PrioritizedRequest {
    /// Create a new prioritized request
    pub fn new(
        request: UsbRequest,
        client_id: String,
        priority: Priority,
        estimated_bytes: u64,
    ) -> Self {
        Self {
            request,
            client_id,
            priority,
            queued_at: Instant::now(),
            estimated_bytes,
        }
    }

    /// Get the request ID
    pub fn id(&self) -> RequestId {
        self.request.id
    }

    /// Get the device handle
    pub fn handle(&self) -> DeviceHandle {
        self.request.handle
    }

    /// Calculate effective priority based on wait time (aging)
    ///
    /// Requests that have waited longer get boosted priority to prevent starvation.
    fn effective_priority(&self) -> (Priority, Duration) {
        let wait_time = self.queued_at.elapsed();

        // Boost priority after waiting too long (prevent starvation)
        let boosted = if wait_time > Duration::from_secs(5) {
            match self.priority {
                Priority::Low => Priority::Medium,
                Priority::Medium => Priority::High,
                other => other,
            }
        } else if wait_time > Duration::from_secs(10) {
            Priority::High
        } else {
            self.priority
        };

        (boosted, wait_time)
    }
}

impl Eq for PrioritizedRequest {}

impl PartialEq for PrioritizedRequest {
    fn eq(&self, other: &Self) -> bool {
        self.request.id == other.request.id
    }
}

impl Ord for PrioritizedRequest {
    fn cmp(&self, other: &Self) -> Ordering {
        let (self_priority, self_wait) = self.effective_priority();
        let (other_priority, other_wait) = other.effective_priority();

        // Higher priority first, then longer wait time
        match self_priority.cmp(&other_priority) {
            Ordering::Equal => self_wait.cmp(&other_wait),
            other => other,
        }
    }
}

impl PartialOrd for PrioritizedRequest {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Fair scheduler state for a single client
#[derive(Debug)]
#[allow(dead_code)]
struct ClientSchedulerState {
    /// Client identifier (kept for debugging)
    client_id: String,
    /// Pending requests for this client (for future queuing support)
    pending: VecDeque<PrioritizedRequest>,
    /// Total bytes transferred in current window
    bytes_transferred: u64,
    /// Window start time
    window_start: Instant,
    /// Last request time (for round-robin fairness)
    last_served: Instant,
}

impl ClientSchedulerState {
    fn new(client_id: String) -> Self {
        Self {
            client_id,
            pending: VecDeque::new(),
            bytes_transferred: 0,
            window_start: Instant::now(),
            last_served: Instant::now(),
        }
    }

    /// Reset the transfer window if needed
    fn maybe_reset_window(&mut self) {
        let elapsed = self.window_start.elapsed();
        if elapsed > Duration::from_secs(1) {
            self.bytes_transferred = 0;
            self.window_start = Instant::now();
        }
    }

    /// Record a transfer
    fn record_transfer(&mut self, bytes: u64) {
        self.maybe_reset_window();
        self.bytes_transferred += bytes;
        self.last_served = Instant::now();
    }
}

/// Priority queue for USB transfer requests
///
/// Implements priority-based scheduling with fair allocation between clients.
#[derive(Debug)]
pub struct PriorityQueue {
    /// Requests ordered by priority
    heap: BinaryHeap<PrioritizedRequest>,
    /// Request lookup by ID
    request_ids: HashMap<RequestId, Priority>,
}

impl PriorityQueue {
    /// Create a new priority queue
    pub fn new() -> Self {
        Self {
            heap: BinaryHeap::new(),
            request_ids: HashMap::new(),
        }
    }

    /// Add a request to the queue
    pub fn push(&mut self, request: PrioritizedRequest) {
        self.request_ids.insert(request.id(), request.priority);
        self.heap.push(request);
    }

    /// Get the next highest-priority request
    pub fn pop(&mut self) -> Option<PrioritizedRequest> {
        let request = self.heap.pop()?;
        self.request_ids.remove(&request.id());
        Some(request)
    }

    /// Peek at the next request without removing it
    pub fn peek(&self) -> Option<&PrioritizedRequest> {
        self.heap.peek()
    }

    /// Check if a request is in the queue
    pub fn contains(&self, id: &RequestId) -> bool {
        self.request_ids.contains_key(id)
    }

    /// Remove a specific request by ID
    pub fn remove(&mut self, id: &RequestId) -> Option<PrioritizedRequest> {
        if !self.request_ids.remove(id).is_some() {
            return None;
        }

        // Rebuild heap without the removed item
        let old_heap = std::mem::take(&mut self.heap);
        let mut removed = None;

        for request in old_heap {
            if request.id() == *id {
                removed = Some(request);
            } else {
                self.heap.push(request);
            }
        }

        removed
    }

    /// Get the number of pending requests
    pub fn len(&self) -> usize {
        self.heap.len()
    }

    /// Check if the queue is empty
    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    /// Clear all requests
    pub fn clear(&mut self) {
        self.heap.clear();
        self.request_ids.clear();
    }
}

impl Default for PriorityQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// Fair scheduler for multi-client bandwidth allocation
///
/// Ensures no single client can monopolize bandwidth while respecting priorities.
#[derive(Debug)]
pub struct FairScheduler {
    /// Per-client state
    clients: HashMap<String, ClientSchedulerState>,
    /// Maximum bytes per client per second (fairness quota)
    client_quota: u64,
    /// Round-robin index for tie-breaking
    round_robin_index: usize,
}

impl FairScheduler {
    /// Create a new fair scheduler
    ///
    /// # Arguments
    /// * `client_quota` - Maximum bytes per client per second for fair sharing
    pub fn new(client_quota: u64) -> Self {
        Self {
            clients: HashMap::new(),
            client_quota,
            round_robin_index: 0,
        }
    }

    /// Register a client
    pub fn register_client(&mut self, client_id: &str) {
        if !self.clients.contains_key(client_id) {
            self.clients.insert(
                client_id.to_string(),
                ClientSchedulerState::new(client_id.to_string()),
            );
        }
    }

    /// Unregister a client
    pub fn unregister_client(&mut self, client_id: &str) {
        self.clients.remove(client_id);
    }

    /// Check if a client is within their fair quota
    pub fn is_within_quota(&mut self, client_id: &str) -> bool {
        if let Some(state) = self.clients.get_mut(client_id) {
            state.maybe_reset_window();
            state.bytes_transferred < self.client_quota
        } else {
            true // Unknown clients get full quota
        }
    }

    /// Record a transfer for a client
    pub fn record_transfer(&mut self, client_id: &str, bytes: u64) {
        if let Some(state) = self.clients.get_mut(client_id) {
            state.record_transfer(bytes);
        }
    }

    /// Get bytes remaining in quota for a client
    pub fn quota_remaining(&mut self, client_id: &str) -> u64 {
        if let Some(state) = self.clients.get_mut(client_id) {
            state.maybe_reset_window();
            self.client_quota.saturating_sub(state.bytes_transferred)
        } else {
            self.client_quota
        }
    }

    /// Select the next client to serve (round-robin among eligible clients)
    pub fn select_next_client(&mut self) -> Option<String> {
        let clients: Vec<String> = self.clients.keys().cloned().collect();
        if clients.is_empty() {
            return None;
        }

        // Round-robin through clients
        for i in 0..clients.len() {
            let idx = (self.round_robin_index + i) % clients.len();
            let client_id = &clients[idx];

            if self.is_within_quota(client_id) {
                self.round_robin_index = (idx + 1) % clients.len();
                return Some(client_id.clone());
            }
        }

        // All clients over quota, pick least-served
        let mut clients_with_time: Vec<_> = self
            .clients
            .iter()
            .map(|(id, state)| (id.clone(), state.last_served))
            .collect();
        clients_with_time.sort_by_key(|(_, t)| *t);

        clients_with_time.first().map(|(id, _)| id.clone())
    }
}

/// QoS manager combining priority queue and fair scheduling
#[derive(Debug)]
pub struct QosManager {
    /// Priority queue for pending requests
    queue: Mutex<PriorityQueue>,
    /// Fair scheduler for client bandwidth allocation
    scheduler: Mutex<FairScheduler>,
    /// Device class cache for priority lookup
    device_classes: Mutex<HashMap<DeviceHandle, u8>>,
    /// QoS enabled flag
    enabled: bool,
}

impl QosManager {
    /// Create a new QoS manager
    ///
    /// # Arguments
    /// * `enabled` - Whether QoS is enabled
    /// * `client_quota` - Per-client bandwidth quota in bytes per second
    pub fn new(enabled: bool, client_quota: u64) -> Self {
        Self {
            queue: Mutex::new(PriorityQueue::new()),
            scheduler: Mutex::new(FairScheduler::new(client_quota)),
            device_classes: Mutex::new(HashMap::new()),
            enabled,
        }
    }

    /// Check if QoS is enabled
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Register a device's class for priority lookup
    pub async fn register_device(&self, handle: DeviceHandle, device_class: u8) {
        let mut classes = self.device_classes.lock().await;
        classes.insert(handle, device_class);
    }

    /// Register device info for priority lookup
    pub async fn register_device_info(&self, handle: DeviceHandle, info: &DeviceInfo) {
        self.register_device(handle, info.class).await;
    }

    /// Unregister a device
    pub async fn unregister_device(&self, handle: DeviceHandle) {
        let mut classes = self.device_classes.lock().await;
        classes.remove(&handle);
    }

    /// Register a client
    pub async fn register_client(&self, client_id: &str) {
        let mut scheduler = self.scheduler.lock().await;
        scheduler.register_client(client_id);
    }

    /// Unregister a client
    pub async fn unregister_client(&self, client_id: &str) {
        let mut scheduler = self.scheduler.lock().await;
        scheduler.unregister_client(client_id);
    }

    /// Calculate priority for a request
    pub async fn calculate_priority(&self, request: &UsbRequest) -> Priority {
        let transfer_priority = Priority::for_transfer_type(&request.transfer);

        let device_classes = self.device_classes.lock().await;
        let device_priority = device_classes
            .get(&request.handle)
            .map(|&class| Priority::for_device_class(class))
            .unwrap_or(Priority::Medium);

        Priority::combined(device_priority, transfer_priority)
    }

    /// Estimate transfer size in bytes
    pub fn estimate_transfer_size(transfer: &TransferType) -> u64 {
        match transfer {
            TransferType::Control { data, .. } => 8 + data.len() as u64, // Setup packet + data
            TransferType::Bulk { data, .. } => data.len() as u64,
            TransferType::Interrupt { data, .. } => data.len() as u64,
            TransferType::Isochronous { data, .. } => data.len() as u64,
        }
    }

    /// Enqueue a request
    pub async fn enqueue(&self, request: UsbRequest, client_id: &str) {
        if !self.enabled {
            return;
        }

        let priority = self.calculate_priority(&request).await;
        let estimated_bytes = Self::estimate_transfer_size(&request.transfer);

        let prioritized =
            PrioritizedRequest::new(request, client_id.to_string(), priority, estimated_bytes);

        let mut queue = self.queue.lock().await;
        queue.push(prioritized);
    }

    /// Dequeue the next request
    pub async fn dequeue(&self) -> Option<PrioritizedRequest> {
        if !self.enabled {
            return None;
        }

        let mut queue = self.queue.lock().await;
        queue.pop()
    }

    /// Check if a client is within their fair quota
    pub async fn is_client_within_quota(&self, client_id: &str) -> bool {
        let mut scheduler = self.scheduler.lock().await;
        scheduler.is_within_quota(client_id)
    }

    /// Record a completed transfer
    pub async fn record_transfer(&self, client_id: &str, bytes: u64) {
        let mut scheduler = self.scheduler.lock().await;
        scheduler.record_transfer(client_id, bytes);
    }

    /// Get the number of pending requests
    pub async fn pending_count(&self) -> usize {
        let queue = self.queue.lock().await;
        queue.len()
    }

    /// Cancel a pending request
    pub async fn cancel(&self, request_id: &RequestId) -> bool {
        let mut queue = self.queue.lock().await;
        queue.remove(request_id).is_some()
    }

    /// Cancel all requests for a device
    pub async fn cancel_device(&self, handle: DeviceHandle) {
        let mut queue = self.queue.lock().await;

        // Find all requests for this device
        let old_len = queue.len();
        let to_remove: Vec<RequestId> = {
            let heap = &queue.heap;
            heap.iter()
                .filter(|r| r.handle() == handle)
                .map(|r| r.id())
                .collect()
        };

        // Remove them
        for id in to_remove {
            queue.remove(&id);
        }

        if queue.len() != old_len {
            tracing::debug!(
                "Cancelled {} requests for device {:?}",
                old_len - queue.len(),
                handle
            );
        }
    }
}

/// Shared QoS manager handle
pub type SharedQosManager = Arc<QosManager>;

/// Create a shared QoS manager
pub fn create_qos_manager(enabled: bool, client_quota_mbps: u64) -> SharedQosManager {
    let quota_bytes = client_quota_mbps * 1_000_000 / 8; // Convert Mbps to bytes/sec
    Arc::new(QosManager::new(enabled, quota_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{DeviceHandle, RequestId};

    fn create_test_request(id: u64, handle: u32) -> UsbRequest {
        UsbRequest {
            id: RequestId(id),
            handle: DeviceHandle(handle),
            transfer: TransferType::Bulk {
                endpoint: 0x01,
                data: vec![0u8; 64],
                timeout_ms: 1000,
                checksum: None,
            },
        }
    }

    #[test]
    fn test_priority_ordering() {
        assert!(Priority::High > Priority::Medium);
        assert!(Priority::Medium > Priority::Low);
        assert!(Priority::Critical > Priority::High);
    }

    #[test]
    fn test_priority_for_device_class() {
        assert_eq!(Priority::for_device_class(0x03), Priority::High); // HID
        assert_eq!(Priority::for_device_class(0x08), Priority::Medium); // Mass Storage
        assert_eq!(Priority::for_device_class(0xFF), Priority::Low); // Vendor
    }

    #[test]
    fn test_priority_for_transfer_type() {
        let control = TransferType::Control {
            request_type: 0x80,
            request: 0x06,
            value: 0x0100,
            index: 0,
            data: vec![],
        };
        assert_eq!(Priority::for_transfer_type(&control), Priority::High);

        let bulk = TransferType::Bulk {
            endpoint: 0x01,
            data: vec![],
            timeout_ms: 1000,
            checksum: None,
        };
        assert_eq!(Priority::for_transfer_type(&bulk), Priority::Medium);
    }

    #[test]
    fn test_priority_queue() {
        let mut queue = PriorityQueue::new();

        let low = PrioritizedRequest::new(
            create_test_request(1, 1),
            "client1".to_string(),
            Priority::Low,
            64,
        );
        let high = PrioritizedRequest::new(
            create_test_request(2, 1),
            "client1".to_string(),
            Priority::High,
            64,
        );
        let medium = PrioritizedRequest::new(
            create_test_request(3, 1),
            "client1".to_string(),
            Priority::Medium,
            64,
        );

        queue.push(low);
        queue.push(high);
        queue.push(medium);

        // Should come out in priority order: high, medium, low
        assert_eq!(queue.pop().unwrap().priority, Priority::High);
        assert_eq!(queue.pop().unwrap().priority, Priority::Medium);
        assert_eq!(queue.pop().unwrap().priority, Priority::Low);
    }

    #[test]
    fn test_fair_scheduler() {
        let mut scheduler = FairScheduler::new(1000);

        scheduler.register_client("client1");
        scheduler.register_client("client2");

        assert!(scheduler.is_within_quota("client1"));
        assert!(scheduler.is_within_quota("client2"));

        // Record transfer for client1
        scheduler.record_transfer("client1", 500);
        assert!(scheduler.is_within_quota("client1")); // Still under 1000

        scheduler.record_transfer("client1", 600);
        assert!(!scheduler.is_within_quota("client1")); // Over 1000

        // client2 should still be within quota
        assert!(scheduler.is_within_quota("client2"));
    }

    #[tokio::test]
    async fn test_qos_manager() {
        let manager = QosManager::new(true, 1_000_000);

        manager.register_client("client1").await;
        manager.register_device(DeviceHandle(1), 0x03).await; // HID device

        let request = create_test_request(1, 1);
        let priority = manager.calculate_priority(&request).await;

        // HID device should get high priority
        assert_eq!(priority, Priority::High);

        // Enqueue and dequeue
        manager.enqueue(request, "client1").await;
        assert_eq!(manager.pending_count().await, 1);

        let dequeued = manager.dequeue().await;
        assert!(dequeued.is_some());
        assert_eq!(manager.pending_count().await, 0);
    }
}
