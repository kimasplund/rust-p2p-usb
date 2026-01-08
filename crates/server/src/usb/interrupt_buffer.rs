//! Interrupt endpoint buffering for low-latency HID data delivery
//!
//! This module implements proactive polling and buffering of interrupt endpoints.
//! Instead of waiting for client requests to read from USB devices, we continuously
//! poll interrupt endpoints in the background and buffer the data. When a client
//! request arrives, we can immediately return data from the buffer, eliminating
//! the USB read latency from the critical path.
//!
//! # Architecture
//!
//! ```text
//! USB Device                    Server                          Client
//! ──────────                    ──────                          ──────
//!     │                            │                               │
//!     │◄──── Background Poll ──────│                               │
//!     │──── HID Report ───────────►│ (buffered)                    │
//!     │                            │                               │
//!     │◄──── Background Poll ──────│                               │
//!     │──── HID Report ───────────►│ (buffered)                    │
//!     │                            │───── Push to client ─────────►│
//!     │                            │◄──── ACK ─────────────────────│
//!     │                            │                               │
//!     │                            │◄──── CMD_SUBMIT ──────────────│
//!     │                            │───── RET_SUBMIT (immediate!) ─►│
//! ```
//!
//! # Components
//!
//! - `InterruptReport`: A single buffered HID report with sequence number
//! - `EndpointBuffer`: Ring buffer for a single endpoint with overflow handling
//! - `InterruptPoller`: Background thread that continuously polls an endpoint
//! - `InterruptBufferManager`: Coordinates buffers and pollers for all devices
//!
//! # Flow Control
//!
//! - Server maintains per-endpoint ring buffers (configurable size)
//! - Each report has a sequence number for ordering and gap detection
//! - Client acknowledges received sequences for flow control
//! - If buffer overflows, oldest unacknowledged data is dropped with warning

use protocol::integrity::compute_interrupt_checksum;
use protocol::DeviceId;
use rusb::{Context, DeviceHandle};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, error, info, trace, warn};

/// Maximum number of buffered reports per endpoint
pub const MAX_BUFFERED_REPORTS: usize = 64;

/// Timeout for USB interrupt reads (short for responsive polling)
const USB_READ_TIMEOUT_MS: u64 = 50;

/// A single buffered interrupt report with metadata and integrity checksum
#[derive(Debug, Clone)]
pub struct InterruptReport {
    /// Sequence number for ordering and verification
    pub seq: u64,
    /// Endpoint this report came from
    pub endpoint: u8,
    /// The HID report data
    pub data: Vec<u8>,
    /// Timestamp when this report was read from USB (microseconds since epoch)
    pub timestamp_us: u64,
    /// CRC32C checksum for integrity verification
    pub checksum: u32,
}

impl InterruptReport {
    /// Create a new interrupt report with computed checksum
    pub fn new(seq: u64, endpoint: u8, data: Vec<u8>) -> Self {
        let timestamp_us = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_micros() as u64)
            .unwrap_or(0);

        // Compute CRC32C checksum for integrity verification
        let checksum = compute_interrupt_checksum(seq, endpoint, &data, timestamp_us);

        Self {
            seq,
            endpoint,
            data,
            timestamp_us,
            checksum,
        }
    }

    /// Get the age of this report in microseconds
    pub fn age_us(&self) -> u64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_micros() as u64)
            .unwrap_or(0);
        now.saturating_sub(self.timestamp_us)
    }

    /// Verify the integrity of this report
    pub fn verify(&self) -> bool {
        let computed = compute_interrupt_checksum(self.seq, self.endpoint, &self.data, self.timestamp_us);
        computed == self.checksum
    }
}

/// Ring buffer for a single interrupt endpoint
///
/// Thread-safe buffer with condition variable for efficient waiting.
pub struct EndpointBuffer {
    /// Endpoint address (e.g., 0x81 for IN endpoint 1)
    endpoint: u8,
    /// Buffered reports (ring buffer)
    reports: Mutex<std::collections::VecDeque<InterruptReport>>,
    /// Condition variable for waiting on data
    data_available: Condvar,
    /// Next sequence number to assign
    next_seq: AtomicU64,
    /// Last acknowledged sequence number (for flow control)
    last_acked_seq: AtomicU64,
    /// Statistics: total reports received
    total_received: AtomicU64,
    /// Statistics: total reports dropped due to overflow
    total_dropped: AtomicU64,
    /// Maximum buffer size
    max_size: usize,
}

impl EndpointBuffer {
    /// Create a new endpoint buffer
    pub fn new(endpoint: u8) -> Self {
        Self::with_capacity(endpoint, MAX_BUFFERED_REPORTS)
    }

    /// Create a new endpoint buffer with specified capacity
    pub fn with_capacity(endpoint: u8, capacity: usize) -> Self {
        Self {
            endpoint,
            reports: Mutex::new(std::collections::VecDeque::with_capacity(capacity)),
            data_available: Condvar::new(),
            next_seq: AtomicU64::new(0),
            last_acked_seq: AtomicU64::new(0),
            total_received: AtomicU64::new(0),
            total_dropped: AtomicU64::new(0),
            max_size: capacity,
        }
    }

    /// Push a new report into the buffer
    ///
    /// If buffer is full, drops oldest unacknowledged report.
    /// Returns the sequence number assigned to this report.
    pub fn push(&self, data: Vec<u8>) -> u64 {
        let seq = self.next_seq.fetch_add(1, Ordering::SeqCst);
        let report = InterruptReport::new(seq, self.endpoint, data);

        let mut reports = self.reports.lock().unwrap();
        self.total_received.fetch_add(1, Ordering::Relaxed);

        // If buffer is full, drop oldest
        if reports.len() >= self.max_size {
            reports.pop_front();
            self.total_dropped.fetch_add(1, Ordering::Relaxed);
            let dropped = self.total_dropped.load(Ordering::Relaxed);
            if dropped % 100 == 1 {
                warn!(
                    "Interrupt buffer overflow on endpoint {:#x} (total dropped: {})",
                    self.endpoint, dropped
                );
            }
        }

        reports.push_back(report);

        // Signal waiters that data is available
        self.data_available.notify_all();

        trace!(
            "Buffered interrupt report on ep {:#x}: seq={}, buffered={}",
            self.endpoint,
            seq,
            reports.len()
        );

        seq
    }

    /// Pop the next report from the buffer (non-blocking)
    pub fn try_pop(&self) -> Option<InterruptReport> {
        let mut reports = self.reports.lock().unwrap();
        reports.pop_front()
    }

    /// Pop the next report, waiting up to timeout if buffer is empty
    pub fn pop_timeout(&self, timeout: Duration) -> Option<InterruptReport> {
        let mut reports = self.reports.lock().unwrap();

        if reports.is_empty() {
            // Wait for data with timeout
            let result = self
                .data_available
                .wait_timeout(reports, timeout)
                .unwrap();
            reports = result.0;
        }

        reports.pop_front()
    }

    /// Pop all available reports (for batch streaming)
    pub fn pop_all(&self) -> Vec<InterruptReport> {
        let mut reports = self.reports.lock().unwrap();
        reports.drain(..).collect()
    }

    /// Peek at the next report without removing it
    pub fn peek(&self) -> Option<InterruptReport> {
        let reports = self.reports.lock().unwrap();
        reports.front().cloned()
    }

    /// Check if buffer has data available
    pub fn has_data(&self) -> bool {
        let reports = self.reports.lock().unwrap();
        !reports.is_empty()
    }

    /// Get number of buffered reports
    pub fn len(&self) -> usize {
        let reports = self.reports.lock().unwrap();
        reports.len()
    }

    /// Check if buffer is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Acknowledge receipt of reports up to and including seq
    pub fn acknowledge(&self, seq: u64) {
        self.last_acked_seq.store(seq, Ordering::SeqCst);
        trace!(
            "Acknowledged seq {} on endpoint {:#x}",
            seq,
            self.endpoint
        );
    }

    /// Get the next sequence number that will be assigned
    pub fn next_seq(&self) -> u64 {
        self.next_seq.load(Ordering::SeqCst)
    }

    /// Get the last acknowledged sequence number
    pub fn last_acked(&self) -> u64 {
        self.last_acked_seq.load(Ordering::SeqCst)
    }

    /// Get statistics (received, dropped)
    pub fn stats(&self) -> (u64, u64) {
        (
            self.total_received.load(Ordering::Relaxed),
            self.total_dropped.load(Ordering::Relaxed),
        )
    }

    /// Get the endpoint address
    pub fn endpoint(&self) -> u8 {
        self.endpoint
    }
}

/// Callback for when interrupt data is received
pub type InterruptCallback = Box<dyn Fn(DeviceId, InterruptReport) + Send + Sync>;

/// Background poller for interrupt endpoints
///
/// Continuously polls a USB interrupt endpoint and buffers received data.
pub struct InterruptPoller {
    /// Device ID
    device_id: DeviceId,
    /// Endpoint address
    endpoint: u8,
    /// Maximum packet size for this endpoint
    max_packet_size: usize,
    /// Buffer for received data
    buffer: Arc<EndpointBuffer>,
    /// Running flag
    running: Arc<AtomicBool>,
    /// Thread handle
    thread_handle: Option<JoinHandle<()>>,
    /// Optional callback when data is received
    callback: Option<Arc<InterruptCallback>>,
}

impl InterruptPoller {
    /// Create a new interrupt poller (does not start polling yet)
    pub fn new(
        device_id: DeviceId,
        endpoint: u8,
        max_packet_size: usize,
        buffer: Arc<EndpointBuffer>,
    ) -> Self {
        Self {
            device_id,
            endpoint,
            max_packet_size,
            buffer,
            running: Arc::new(AtomicBool::new(false)),
            thread_handle: None,
            callback: None,
        }
    }

    /// Set callback to be called when data is received
    pub fn set_callback(&mut self, callback: InterruptCallback) {
        self.callback = Some(Arc::new(callback));
    }

    /// Start the polling thread
    ///
    /// Note: This needs access to the USB device handle. The handle is passed
    /// to a closure that will be called from the polling thread.
    pub fn start<F>(&mut self, read_fn: F)
    where
        F: Fn(&mut [u8], Duration) -> Result<usize, rusb::Error> + Send + 'static,
    {
        if self.running.load(Ordering::Acquire) {
            return; // Already running
        }

        self.running.store(true, Ordering::Release);
        let running = self.running.clone();
        let buffer = self.buffer.clone();
        let endpoint = self.endpoint;
        let max_packet_size = self.max_packet_size;
        let device_id = self.device_id;
        let callback = self.callback.clone();

        let handle = thread::Builder::new()
            .name(format!("int-poll-{:?}-{:#x}", device_id, endpoint))
            .spawn(move || {
                info!(
                    "Interrupt poller started for device {:?} endpoint {:#x}",
                    device_id, endpoint
                );

                let timeout = Duration::from_millis(USB_READ_TIMEOUT_MS);
                let mut read_buffer = vec![0u8; max_packet_size];

                while running.load(Ordering::Acquire) {
                    match read_fn(&mut read_buffer, timeout) {
                        Ok(len) if len > 0 => {
                            let data = read_buffer[..len].to_vec();
                            let seq = buffer.push(data.clone());

                            // Call callback if set
                            if let Some(ref cb) = callback {
                                let report = InterruptReport::new(seq, endpoint, data);
                                cb(device_id, report);
                            }
                        }
                        Ok(_) => {
                            // Zero-length read, continue polling
                        }
                        Err(rusb::Error::Timeout) => {
                            // Normal - no data available, continue polling
                        }
                        Err(rusb::Error::NoDevice) => {
                            info!(
                                "Device {:?} disconnected, stopping poller for endpoint {:#x}",
                                device_id, endpoint
                            );
                            break;
                        }
                        Err(e) => {
                            trace!(
                                "Interrupt poll error on {:?} endpoint {:#x}: {}",
                                device_id,
                                endpoint,
                                e
                            );
                            // Brief sleep on error to avoid tight loop
                            thread::sleep(Duration::from_millis(10));
                        }
                    }
                }

                info!(
                    "Interrupt poller stopped for device {:?} endpoint {:#x}",
                    device_id, endpoint
                );
            })
            .expect("Failed to spawn interrupt poller thread");

        self.thread_handle = Some(handle);
    }

    /// Stop the polling thread
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }

    /// Check if the poller is running
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    /// Get the buffer
    pub fn buffer(&self) -> &Arc<EndpointBuffer> {
        &self.buffer
    }
}

impl Drop for InterruptPoller {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Manages interrupt buffers and pollers for a single device
pub struct DeviceInterruptManager {
    /// Device ID
    device_id: DeviceId,
    /// Per-endpoint buffers
    buffers: HashMap<u8, Arc<EndpointBuffer>>,
    /// Per-endpoint pollers
    pollers: HashMap<u8, InterruptPoller>,
    /// Global running flag for this device
    running: Arc<AtomicBool>,
}

impl DeviceInterruptManager {
    /// Create a new device interrupt manager
    pub fn new(device_id: DeviceId) -> Self {
        Self {
            device_id,
            buffers: HashMap::new(),
            pollers: HashMap::new(),
            running: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Register an interrupt endpoint
    pub fn register_endpoint(&mut self, endpoint: u8, max_packet_size: usize) {
        if self.buffers.contains_key(&endpoint) {
            return; // Already registered
        }

        let buffer = Arc::new(EndpointBuffer::new(endpoint));
        self.buffers.insert(endpoint, buffer.clone());

        let poller = InterruptPoller::new(self.device_id, endpoint, max_packet_size, buffer);
        self.pollers.insert(endpoint, poller);

        debug!(
            "Registered interrupt endpoint {:#x} for device {:?} (max_packet={})",
            endpoint, self.device_id, max_packet_size
        );
    }

    /// Start polling for an endpoint
    pub fn start_polling<F>(&mut self, endpoint: u8, read_fn: F)
    where
        F: Fn(&mut [u8], Duration) -> Result<usize, rusb::Error> + Send + 'static,
    {
        if let Some(poller) = self.pollers.get_mut(&endpoint) {
            poller.start(read_fn);
        }
    }

    /// Stop polling for an endpoint
    pub fn stop_polling(&mut self, endpoint: u8) {
        if let Some(poller) = self.pollers.get_mut(&endpoint) {
            poller.stop();
        }
    }

    /// Stop all polling
    pub fn stop_all(&mut self) {
        self.running.store(false, Ordering::Release);
        for poller in self.pollers.values_mut() {
            poller.stop();
        }
    }

    /// Get buffer for an endpoint
    pub fn buffer(&self, endpoint: u8) -> Option<&Arc<EndpointBuffer>> {
        self.buffers.get(&endpoint)
    }

    /// Pop data from an endpoint's buffer (non-blocking)
    pub fn try_pop(&self, endpoint: u8) -> Option<InterruptReport> {
        self.buffers.get(&endpoint).and_then(|b| b.try_pop())
    }

    /// Pop data from an endpoint's buffer (with timeout)
    pub fn pop_timeout(&self, endpoint: u8, timeout: Duration) -> Option<InterruptReport> {
        self.buffers
            .get(&endpoint)
            .and_then(|b| b.pop_timeout(timeout))
    }

    /// Pop all data from an endpoint's buffer
    pub fn pop_all(&self, endpoint: u8) -> Vec<InterruptReport> {
        self.buffers
            .get(&endpoint)
            .map(|b| b.pop_all())
            .unwrap_or_default()
    }

    /// Check if an endpoint has buffered data
    pub fn has_data(&self, endpoint: u8) -> bool {
        self.buffers
            .get(&endpoint)
            .map(|b| b.has_data())
            .unwrap_or(false)
    }

    /// Acknowledge receipt of data
    pub fn acknowledge(&self, endpoint: u8, seq: u64) {
        if let Some(buffer) = self.buffers.get(&endpoint) {
            buffer.acknowledge(seq);
        }
    }

    /// Get list of registered endpoints
    pub fn endpoints(&self) -> Vec<u8> {
        self.buffers.keys().copied().collect()
    }

    /// Set callback for when data is received on any endpoint
    pub fn set_callback(&mut self, endpoint: u8, callback: InterruptCallback) {
        if let Some(poller) = self.pollers.get_mut(&endpoint) {
            poller.set_callback(callback);
        }
    }
}

impl Drop for DeviceInterruptManager {
    fn drop(&mut self) {
        self.stop_all();
    }
}

/// Global interrupt buffer manager
///
/// Manages interrupt buffers for all devices.
pub struct InterruptBufferManager {
    /// Per-device managers
    devices: Mutex<HashMap<DeviceId, DeviceInterruptManager>>,
    /// Channel for streaming data to clients
    stream_tx: Option<mpsc::UnboundedSender<(DeviceId, InterruptReport)>>,
}

impl InterruptBufferManager {
    /// Create a new interrupt buffer manager
    pub fn new() -> Self {
        Self {
            devices: Mutex::new(HashMap::new()),
            stream_tx: None,
        }
    }

    /// Create with a channel for streaming data
    pub fn with_stream(
        tx: mpsc::UnboundedSender<(DeviceId, InterruptReport)>,
    ) -> Self {
        Self {
            devices: Mutex::new(HashMap::new()),
            stream_tx: Some(tx),
        }
    }

    /// Register a device
    pub fn register_device(&self, device_id: DeviceId) {
        let mut devices = self.devices.lock().unwrap();
        devices
            .entry(device_id)
            .or_insert_with(|| DeviceInterruptManager::new(device_id));
    }

    /// Register an interrupt endpoint for a device
    pub fn register_endpoint(
        &self,
        device_id: DeviceId,
        endpoint: u8,
        max_packet_size: usize,
    ) {
        let mut devices = self.devices.lock().unwrap();
        if let Some(manager) = devices.get_mut(&device_id) {
            manager.register_endpoint(endpoint, max_packet_size);
        }
    }

    /// Unregister a device (stops all polling)
    pub fn unregister_device(&self, device_id: DeviceId) {
        let mut devices = self.devices.lock().unwrap();
        if let Some(mut manager) = devices.remove(&device_id) {
            manager.stop_all();
        }
    }

    /// Push data to an endpoint's buffer (used when not using background polling)
    pub fn push(&self, device_id: DeviceId, endpoint: u8, data: Vec<u8>) -> Option<u64> {
        let devices = self.devices.lock().unwrap();
        if let Some(manager) = devices.get(&device_id) {
            if let Some(buffer) = manager.buffer(endpoint) {
                let seq = buffer.push(data.clone());

                // Also send to stream if configured
                if let Some(ref tx) = self.stream_tx {
                    let report = InterruptReport::new(seq, endpoint, data);
                    let _ = tx.send((device_id, report));
                }

                return Some(seq);
            }
        }
        None
    }

    /// Try to pop data from an endpoint's buffer
    pub fn try_pop(&self, device_id: DeviceId, endpoint: u8) -> Option<InterruptReport> {
        let devices = self.devices.lock().unwrap();
        devices.get(&device_id).and_then(|m| m.try_pop(endpoint))
    }

    /// Pop data with timeout
    pub fn pop_timeout(
        &self,
        device_id: DeviceId,
        endpoint: u8,
        timeout: Duration,
    ) -> Option<InterruptReport> {
        // Need to release lock before waiting
        let buffer = {
            let devices = self.devices.lock().unwrap();
            devices
                .get(&device_id)
                .and_then(|m| m.buffer(endpoint))
                .cloned()
        };

        buffer.and_then(|b| b.pop_timeout(timeout))
    }

    /// Pop all data from an endpoint
    pub fn pop_all(&self, device_id: DeviceId, endpoint: u8) -> Vec<InterruptReport> {
        let devices = self.devices.lock().unwrap();
        devices
            .get(&device_id)
            .map(|m| m.pop_all(endpoint))
            .unwrap_or_default()
    }

    /// Check if an endpoint has data
    pub fn has_data(&self, device_id: DeviceId, endpoint: u8) -> bool {
        let devices = self.devices.lock().unwrap();
        devices
            .get(&device_id)
            .map(|m| m.has_data(endpoint))
            .unwrap_or(false)
    }

    /// Acknowledge receipt of data
    pub fn acknowledge(&self, device_id: DeviceId, endpoint: u8, seq: u64) {
        let devices = self.devices.lock().unwrap();
        if let Some(manager) = devices.get(&device_id) {
            manager.acknowledge(endpoint, seq);
        }
    }

    /// Get buffer for an endpoint
    pub fn buffer(&self, device_id: DeviceId, endpoint: u8) -> Option<Arc<EndpointBuffer>> {
        let devices = self.devices.lock().unwrap();
        devices
            .get(&device_id)
            .and_then(|m| m.buffer(endpoint))
            .cloned()
    }

    /// Access device manager (for starting polling etc)
    pub fn with_device<F, R>(&self, device_id: DeviceId, f: F) -> Option<R>
    where
        F: FnOnce(&mut DeviceInterruptManager) -> R,
    {
        let mut devices = self.devices.lock().unwrap();
        devices.get_mut(&device_id).map(f)
    }
}

impl Default for InterruptBufferManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_endpoint_buffer_push_pop() {
        let buffer = EndpointBuffer::new(0x81);

        let seq1 = buffer.push(vec![1, 2, 3]);
        let seq2 = buffer.push(vec![4, 5, 6]);

        assert_eq!(seq1, 0);
        assert_eq!(seq2, 1);
        assert_eq!(buffer.len(), 2);
        assert!(buffer.has_data());

        let report1 = buffer.try_pop().unwrap();
        assert_eq!(report1.seq, 0);
        assert_eq!(report1.data, vec![1, 2, 3]);

        let report2 = buffer.try_pop().unwrap();
        assert_eq!(report2.seq, 1);
        assert_eq!(report2.data, vec![4, 5, 6]);

        assert!(!buffer.has_data());
        assert!(buffer.try_pop().is_none());
    }

    #[test]
    fn test_endpoint_buffer_overflow() {
        let buffer = EndpointBuffer::with_capacity(0x81, 4);

        // Fill beyond capacity
        for i in 0..10 {
            buffer.push(vec![i as u8]);
        }

        // Should have dropped oldest entries
        assert_eq!(buffer.len(), 4);
        let (received, dropped) = buffer.stats();
        assert_eq!(received, 10);
        assert_eq!(dropped, 6);

        // First available should be seq 6 (dropped 0-5)
        let report = buffer.try_pop().unwrap();
        assert_eq!(report.seq, 6);
    }

    #[test]
    fn test_endpoint_buffer_timeout() {
        let buffer = Arc::new(EndpointBuffer::new(0x81));
        let buffer_clone = buffer.clone();

        // Spawn thread to push data after a delay
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            buffer_clone.push(vec![1, 2, 3]);
        });

        // Wait for data with timeout
        let report = buffer.pop_timeout(Duration::from_millis(200));
        assert!(report.is_some());
        assert_eq!(report.unwrap().data, vec![1, 2, 3]);

        handle.join().unwrap();
    }

    #[test]
    fn test_device_interrupt_manager() {
        let mut manager = DeviceInterruptManager::new(DeviceId(1));

        manager.register_endpoint(0x81, 8);
        manager.register_endpoint(0x82, 8);

        // Push data directly to buffers (simulating what poller would do)
        manager.buffers.get(&0x81).unwrap().push(vec![1, 2, 3]);
        manager.buffers.get(&0x82).unwrap().push(vec![4, 5, 6]);

        assert!(manager.has_data(0x81));
        assert!(manager.has_data(0x82));
        assert!(!manager.has_data(0x83)); // Not registered

        let report1 = manager.try_pop(0x81).unwrap();
        assert_eq!(report1.data, vec![1, 2, 3]);

        let report2 = manager.try_pop(0x82).unwrap();
        assert_eq!(report2.data, vec![4, 5, 6]);
    }

    #[test]
    fn test_interrupt_buffer_manager() {
        let manager = InterruptBufferManager::new();

        manager.register_device(DeviceId(1));
        manager.register_endpoint(DeviceId(1), 0x81, 8);

        let seq = manager.push(DeviceId(1), 0x81, vec![1, 2, 3]);
        assert_eq!(seq, Some(0));

        assert!(manager.has_data(DeviceId(1), 0x81));

        let report = manager.try_pop(DeviceId(1), 0x81).unwrap();
        assert_eq!(report.data, vec![1, 2, 3]);

        manager.unregister_device(DeviceId(1));
        assert!(!manager.has_data(DeviceId(1), 0x81));
    }

    #[test]
    fn test_interrupt_report_age() {
        let report = InterruptReport::new(0, 0x81, vec![1, 2, 3]);
        thread::sleep(Duration::from_millis(10));
        let age = report.age_us();
        assert!(age >= 10_000); // At least 10ms in microseconds
    }
}
