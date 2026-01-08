//! Client-side interrupt receive buffer
//!
//! Receives proactively streamed interrupt data from server and serves it
//! immediately to the kernel. This eliminates network round-trip latency
//! for HID devices.
//!
//! # Architecture
//!
//! ```text
//! Server                          Client
//!   │                               │
//!   │ InterruptData(seq=1)─────────>│ Store in buffer
//!   │ InterruptData(seq=2)─────────>│ Store in buffer
//!   │ InterruptData(seq=3)─────────>│ Store in buffer
//!   │                               │
//!   │                        Kernel │ USB/IP: Read EP 0x81
//!   │                               │ Serve immediately from buffer
//!   │                               │
//!   │<─────────InterruptAck(seq=3)──│ Acknowledge receipt
//! ```

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use tracing::{debug, trace, warn};

/// Buffered interrupt report received from server
#[derive(Debug, Clone)]
pub struct ReceivedReport {
    /// Sequence number from server
    pub sequence: u64,
    /// Endpoint address
    pub endpoint: u8,
    /// Report data
    pub data: Vec<u8>,
    /// Server timestamp (microseconds since epoch)
    pub server_timestamp_us: u64,
    /// When we received this report
    pub received_at: Instant,
}

/// Per-endpoint receive buffer for interrupt data
pub struct EndpointReceiveBuffer {
    /// Endpoint address
    endpoint: u8,
    /// Buffered reports waiting to be served to kernel
    reports: Mutex<VecDeque<ReceivedReport>>,
    /// Condition variable for waiting threads
    data_available: Condvar,
    /// Next expected sequence number (for gap detection)
    next_expected_seq: AtomicU64,
    /// Highest sequence number received
    highest_seq: AtomicU64,
    /// Total reports received
    total_received: AtomicU64,
    /// Reports served to kernel
    total_served: AtomicU64,
    /// Sequence gaps detected
    gaps_detected: AtomicU64,
    /// Maximum buffer size
    max_size: usize,
    /// Whether streaming is active
    active: Mutex<bool>,
}

impl EndpointReceiveBuffer {
    /// Create a new receive buffer for an endpoint
    pub fn new(endpoint: u8, max_size: usize) -> Self {
        Self {
            endpoint,
            reports: Mutex::new(VecDeque::with_capacity(max_size)),
            data_available: Condvar::new(),
            next_expected_seq: AtomicU64::new(0),
            highest_seq: AtomicU64::new(0),
            total_received: AtomicU64::new(0),
            total_served: AtomicU64::new(0),
            gaps_detected: AtomicU64::new(0),
            max_size,
            active: Mutex::new(false),
        }
    }

    /// Activate the buffer (start accepting reports)
    pub fn activate(&self, start_seq: u64) {
        self.next_expected_seq.store(start_seq, Ordering::SeqCst);
        self.highest_seq.store(start_seq.saturating_sub(1), Ordering::SeqCst);
        *self.active.lock().unwrap() = true;
        debug!(
            "Activated receive buffer for endpoint 0x{:02x}, starting seq={}",
            self.endpoint, start_seq
        );
    }

    /// Deactivate the buffer and clear all pending reports
    pub fn deactivate(&self) {
        *self.active.lock().unwrap() = false;
        let mut reports = self.reports.lock().unwrap();
        let count = reports.len();
        reports.clear();
        self.data_available.notify_all();
        debug!(
            "Deactivated receive buffer for endpoint 0x{:02x}, cleared {} reports",
            self.endpoint, count
        );
    }

    /// Check if buffer is active
    pub fn is_active(&self) -> bool {
        *self.active.lock().unwrap()
    }

    /// Store a received interrupt report
    ///
    /// Returns (gap_detected, should_ack) tuple:
    /// - gap_detected: true if sequence gap was detected
    /// - should_ack: true if acknowledgment should be sent
    pub fn store(&self, report: ReceivedReport) -> (bool, bool) {
        if !self.is_active() {
            trace!(
                "Dropping report for inactive buffer ep=0x{:02x}",
                self.endpoint
            );
            return (false, false);
        }

        let seq = report.sequence;
        let expected = self.next_expected_seq.load(Ordering::SeqCst);
        let mut gap_detected = false;

        // Check for sequence gaps
        if seq > expected {
            let gap_size = seq - expected;
            warn!(
                "Sequence gap detected on ep=0x{:02x}: expected {}, got {} (gap={})",
                self.endpoint, expected, seq, gap_size
            );
            self.gaps_detected.fetch_add(1, Ordering::Relaxed);
            gap_detected = true;
        } else if seq < expected {
            trace!(
                "Late/duplicate report on ep=0x{:02x}: seq={} (expected {})",
                self.endpoint, seq, expected
            );
            // Still store it - kernel might need it
        }

        // Update tracking
        self.next_expected_seq.store(seq + 1, Ordering::SeqCst);
        let prev_highest = self.highest_seq.fetch_max(seq, Ordering::SeqCst);

        // Store the report
        let mut reports = self.reports.lock().unwrap();

        // Handle buffer overflow
        if reports.len() >= self.max_size {
            // Drop oldest report
            if let Some(dropped) = reports.pop_front() {
                trace!(
                    "Buffer overflow on ep=0x{:02x}, dropping seq={}",
                    self.endpoint, dropped.sequence
                );
            }
        }

        reports.push_back(report);
        self.total_received.fetch_add(1, Ordering::Relaxed);

        // Notify waiting threads
        self.data_available.notify_one();

        // Acknowledge every 10 reports or on gap
        let should_ack = gap_detected || (seq > prev_highest && seq % 10 == 0);

        (gap_detected, should_ack)
    }

    /// Get the next report for the kernel (non-blocking)
    pub fn try_take(&self) -> Option<ReceivedReport> {
        let mut reports = self.reports.lock().unwrap();
        let report = reports.pop_front();
        if report.is_some() {
            self.total_served.fetch_add(1, Ordering::Relaxed);
        }
        report
    }

    /// Get the next report for the kernel (blocking with timeout)
    pub fn take_timeout(&self, timeout: Duration) -> Option<ReceivedReport> {
        let mut reports = self.reports.lock().unwrap();

        // Fast path: report already available
        if let Some(report) = reports.pop_front() {
            self.total_served.fetch_add(1, Ordering::Relaxed);
            return Some(report);
        }

        // Wait for data with timeout
        let (mut reports, result) = self
            .data_available
            .wait_timeout_while(reports, timeout, |r| r.is_empty() && self.is_active())
            .unwrap();

        if result.timed_out() {
            return None;
        }

        let report = reports.pop_front();
        if report.is_some() {
            self.total_served.fetch_add(1, Ordering::Relaxed);
        }
        report
    }

    /// Get buffer statistics
    pub fn stats(&self) -> ReceiveBufferStats {
        ReceiveBufferStats {
            endpoint: self.endpoint,
            buffered: self.reports.lock().unwrap().len(),
            total_received: self.total_received.load(Ordering::Relaxed),
            total_served: self.total_served.load(Ordering::Relaxed),
            gaps_detected: self.gaps_detected.load(Ordering::Relaxed),
            highest_seq: self.highest_seq.load(Ordering::Relaxed),
            active: self.is_active(),
        }
    }

    /// Get highest acknowledged sequence number (for InterruptAck)
    pub fn highest_acked_seq(&self) -> u64 {
        self.highest_seq.load(Ordering::Relaxed)
    }
}

/// Buffer statistics
#[derive(Debug, Clone)]
pub struct ReceiveBufferStats {
    pub endpoint: u8,
    pub buffered: usize,
    pub total_received: u64,
    pub total_served: u64,
    pub gaps_detected: u64,
    pub highest_seq: u64,
    pub active: bool,
}

/// Manager for all interrupt receive buffers on a device
pub struct DeviceReceiveBufferManager {
    /// Device handle
    device_handle: u32,
    /// Buffers per endpoint
    buffers: Mutex<HashMap<u8, Arc<EndpointReceiveBuffer>>>,
    /// Default buffer size
    default_buffer_size: usize,
}

impl DeviceReceiveBufferManager {
    /// Create a new manager for a device
    pub fn new(device_handle: u32, default_buffer_size: usize) -> Self {
        Self {
            device_handle,
            buffers: Mutex::new(HashMap::new()),
            default_buffer_size,
        }
    }

    /// Get or create a buffer for an endpoint
    pub fn get_or_create(&self, endpoint: u8) -> Arc<EndpointReceiveBuffer> {
        let mut buffers = self.buffers.lock().unwrap();
        buffers
            .entry(endpoint)
            .or_insert_with(|| {
                debug!(
                    "Creating receive buffer for device {} endpoint 0x{:02x}",
                    self.device_handle, endpoint
                );
                Arc::new(EndpointReceiveBuffer::new(endpoint, self.default_buffer_size))
            })
            .clone()
    }

    /// Get an existing buffer (don't create)
    pub fn get(&self, endpoint: u8) -> Option<Arc<EndpointReceiveBuffer>> {
        self.buffers.lock().unwrap().get(&endpoint).cloned()
    }

    /// Activate streaming for an endpoint
    pub fn activate_endpoint(&self, endpoint: u8, start_seq: u64) {
        let buffer = self.get_or_create(endpoint);
        buffer.activate(start_seq);
    }

    /// Deactivate streaming for an endpoint
    pub fn deactivate_endpoint(&self, endpoint: u8) {
        if let Some(buffer) = self.get(endpoint) {
            buffer.deactivate();
        }
    }

    /// Deactivate all endpoints
    pub fn deactivate_all(&self) {
        let buffers = self.buffers.lock().unwrap();
        for (_, buffer) in buffers.iter() {
            buffer.deactivate();
        }
    }

    /// Store a received report
    pub fn store_report(&self, report: ReceivedReport) -> (bool, bool) {
        let buffer = self.get_or_create(report.endpoint);
        buffer.store(report)
    }

    /// Try to get a report for an endpoint (non-blocking)
    pub fn try_take(&self, endpoint: u8) -> Option<ReceivedReport> {
        self.get(endpoint)?.try_take()
    }

    /// Get a report for an endpoint (blocking with timeout)
    pub fn take_timeout(&self, endpoint: u8, timeout: Duration) -> Option<ReceivedReport> {
        self.get_or_create(endpoint).take_timeout(timeout)
    }

    /// Check if an endpoint has streaming active
    pub fn is_streaming(&self, endpoint: u8) -> bool {
        self.get(endpoint).map(|b| b.is_active()).unwrap_or(false)
    }

    /// Get device handle
    pub fn device_handle(&self) -> u32 {
        self.device_handle
    }

    /// Get stats for all endpoints
    pub fn all_stats(&self) -> Vec<ReceiveBufferStats> {
        self.buffers
            .lock()
            .unwrap()
            .values()
            .map(|b| b.stats())
            .collect()
    }
}

/// Global interrupt receive buffer manager for all devices
pub struct InterruptReceiveManager {
    /// Managers per device handle
    devices: Mutex<HashMap<u32, Arc<DeviceReceiveBufferManager>>>,
    /// Default buffer size
    default_buffer_size: usize,
}

impl InterruptReceiveManager {
    /// Create a new global manager
    pub fn new(default_buffer_size: usize) -> Self {
        Self {
            devices: Mutex::new(HashMap::new()),
            default_buffer_size,
        }
    }

    /// Get or create a manager for a device
    pub fn get_or_create(&self, device_handle: u32) -> Arc<DeviceReceiveBufferManager> {
        let mut devices = self.devices.lock().unwrap();
        devices
            .entry(device_handle)
            .or_insert_with(|| {
                debug!("Creating receive buffer manager for device {}", device_handle);
                Arc::new(DeviceReceiveBufferManager::new(
                    device_handle,
                    self.default_buffer_size,
                ))
            })
            .clone()
    }

    /// Get an existing manager (don't create)
    pub fn get(&self, device_handle: u32) -> Option<Arc<DeviceReceiveBufferManager>> {
        self.devices.lock().unwrap().get(&device_handle).cloned()
    }

    /// Remove a device (on detach)
    pub fn remove_device(&self, device_handle: u32) {
        if let Some(manager) = self.devices.lock().unwrap().remove(&device_handle) {
            manager.deactivate_all();
            debug!("Removed receive buffer manager for device {}", device_handle);
        }
    }

    /// Process incoming InterruptData message
    ///
    /// Returns (gap_detected, should_send_ack, ack_seq) if an ack should be sent
    pub fn process_interrupt_data(
        &self,
        device_handle: u32,
        endpoint: u8,
        sequence: u64,
        data: Vec<u8>,
        server_timestamp_us: u64,
    ) -> Option<(bool, u64)> {
        let manager = self.get_or_create(device_handle);

        let report = ReceivedReport {
            sequence,
            endpoint,
            data,
            server_timestamp_us,
            received_at: Instant::now(),
        };

        let (gap_detected, should_ack) = manager.store_report(report);

        if should_ack {
            let buffer = manager.get(endpoint)?;
            Some((gap_detected, buffer.highest_acked_seq()))
        } else {
            None
        }
    }
}

impl Default for InterruptReceiveManager {
    fn default() -> Self {
        Self::new(1024) // Default to 1024 reports per endpoint
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_buffer_basic() {
        let buffer = EndpointReceiveBuffer::new(0x81, 100);
        buffer.activate(0);

        let report = ReceivedReport {
            sequence: 0,
            endpoint: 0x81,
            data: vec![0, 0, 4, 0, 0, 0, 0, 0], // 'A' key
            server_timestamp_us: 1000,
            received_at: Instant::now(),
        };

        let (gap, should_ack) = buffer.store(report);
        assert!(!gap);

        let taken = buffer.try_take().unwrap();
        assert_eq!(taken.sequence, 0);
        assert_eq!(taken.data[2], 4); // 'A' key
    }

    #[test]
    fn test_buffer_gap_detection() {
        let buffer = EndpointReceiveBuffer::new(0x81, 100);
        buffer.activate(0);

        // Store seq 0
        let report0 = ReceivedReport {
            sequence: 0,
            endpoint: 0x81,
            data: vec![],
            server_timestamp_us: 1000,
            received_at: Instant::now(),
        };
        buffer.store(report0);

        // Skip to seq 5 (gap of 4)
        let report5 = ReceivedReport {
            sequence: 5,
            endpoint: 0x81,
            data: vec![],
            server_timestamp_us: 2000,
            received_at: Instant::now(),
        };
        let (gap, _) = buffer.store(report5);
        assert!(gap);

        let stats = buffer.stats();
        assert_eq!(stats.gaps_detected, 1);
    }

    #[test]
    fn test_buffer_overflow() {
        let buffer = EndpointReceiveBuffer::new(0x81, 3);
        buffer.activate(0);

        // Fill buffer beyond capacity
        for i in 0..5 {
            let report = ReceivedReport {
                sequence: i,
                endpoint: 0x81,
                data: vec![i as u8],
                server_timestamp_us: i * 1000,
                received_at: Instant::now(),
            };
            buffer.store(report);
        }

        let stats = buffer.stats();
        assert_eq!(stats.buffered, 3); // Only 3 kept
        assert_eq!(stats.total_received, 5); // But 5 received

        // Should have dropped oldest (0, 1)
        let first = buffer.try_take().unwrap();
        assert_eq!(first.sequence, 2);
    }

    #[test]
    fn test_device_manager() {
        let manager = DeviceReceiveBufferManager::new(1, 100);

        manager.activate_endpoint(0x81, 0);
        assert!(manager.is_streaming(0x81));
        assert!(!manager.is_streaming(0x82));

        let report = ReceivedReport {
            sequence: 0,
            endpoint: 0x81,
            data: vec![1, 2, 3],
            server_timestamp_us: 1000,
            received_at: Instant::now(),
        };
        manager.store_report(report);

        let taken = manager.try_take(0x81).unwrap();
        assert_eq!(taken.data, vec![1, 2, 3]);
    }
}
