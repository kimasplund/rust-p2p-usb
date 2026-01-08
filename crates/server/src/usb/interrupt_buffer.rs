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
//!     │                            │                               │
//!     │                            │◄──── CMD_SUBMIT ──────────────│
//!     │                            │───── RET_SUBMIT (from buffer)─►│
//!     │                            │        (immediate!)            │
//! ```
//!
//! # Benefits
//!
//! - Eliminates USB polling latency from client request path
//! - HID key-up events arrive immediately after key-down
//! - Prevents keyboard auto-repeat caused by network latency
//! - Gracefully handles network congestion with bounded buffer

use protocol::DeviceId;
use rusb::{Context, DeviceHandle};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, error, trace, warn};

/// Maximum number of buffered reports per endpoint
const MAX_BUFFERED_REPORTS: usize = 32;

/// Polling interval for interrupt endpoints when no data is available
const POLL_INTERVAL_MS: u64 = 1;

/// Timeout for USB interrupt reads (short to allow responsive polling)
const USB_READ_TIMEOUT_MS: u64 = 10;

/// A single buffered interrupt report
#[derive(Debug, Clone)]
pub struct InterruptReport {
    /// Sequence number for ordering and verification
    pub seq: u64,
    /// The HID report data
    pub data: Vec<u8>,
    /// Timestamp when this report was read from USB
    pub timestamp_us: u64,
}

/// Buffer for a single interrupt endpoint
pub struct EndpointBuffer {
    /// Endpoint address (e.g., 0x81 for IN endpoint 1)
    endpoint: u8,
    /// Buffered reports (FIFO queue)
    reports: std::collections::VecDeque<InterruptReport>,
    /// Next sequence number to assign
    next_seq: u64,
    /// Total reports received (for stats)
    total_received: u64,
    /// Total reports dropped due to full buffer
    total_dropped: u64,
}

impl EndpointBuffer {
    pub fn new(endpoint: u8) -> Self {
        Self {
            endpoint,
            reports: std::collections::VecDeque::with_capacity(MAX_BUFFERED_REPORTS),
            next_seq: 0,
            total_received: 0,
            total_dropped: 0,
        }
    }

    /// Push a new report into the buffer
    pub fn push(&mut self, data: Vec<u8>) {
        let timestamp_us = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_micros() as u64)
            .unwrap_or(0);

        let report = InterruptReport {
            seq: self.next_seq,
            data,
            timestamp_us,
        };
        self.next_seq += 1;
        self.total_received += 1;

        // If buffer is full, drop oldest report
        if self.reports.len() >= MAX_BUFFERED_REPORTS {
            self.reports.pop_front();
            self.total_dropped += 1;
            warn!(
                "Interrupt buffer overflow on endpoint {:#x}, dropped oldest report (total dropped: {})",
                self.endpoint, self.total_dropped
            );
        }

        self.reports.push_back(report);
        trace!(
            "Buffered interrupt report on ep {:#x}: seq={}, len={}, buffered={}",
            self.endpoint,
            self.next_seq - 1,
            self.reports.back().map(|r| r.data.len()).unwrap_or(0),
            self.reports.len()
        );
    }

    /// Pop the next report from the buffer (FIFO)
    pub fn pop(&mut self) -> Option<InterruptReport> {
        self.reports.pop_front()
    }

    /// Check if buffer has data available
    pub fn has_data(&self) -> bool {
        !self.reports.is_empty()
    }

    /// Get number of buffered reports
    pub fn len(&self) -> usize {
        self.reports.len()
    }

    /// Get stats
    pub fn stats(&self) -> (u64, u64) {
        (self.total_received, self.total_dropped)
    }
}

/// Manages interrupt buffers for a single device
pub struct DeviceInterruptBuffers {
    /// Device ID
    device_id: DeviceId,
    /// Per-endpoint buffers, keyed by endpoint address
    buffers: HashMap<u8, EndpointBuffer>,
    /// Flag to stop background polling
    running: Arc<AtomicBool>,
}

impl DeviceInterruptBuffers {
    pub fn new(device_id: DeviceId) -> Self {
        Self {
            device_id,
            buffers: HashMap::new(),
            running: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Register an interrupt endpoint for buffering
    pub fn register_endpoint(&mut self, endpoint: u8) {
        if !self.buffers.contains_key(&endpoint) {
            debug!(
                "Registering interrupt buffer for device {:?} endpoint {:#x}",
                self.device_id, endpoint
            );
            self.buffers.insert(endpoint, EndpointBuffer::new(endpoint));
        }
    }

    /// Push data to an endpoint's buffer
    pub fn push(&mut self, endpoint: u8, data: Vec<u8>) {
        if let Some(buffer) = self.buffers.get_mut(&endpoint) {
            buffer.push(data);
        }
    }

    /// Pop data from an endpoint's buffer
    pub fn pop(&mut self, endpoint: u8) -> Option<InterruptReport> {
        self.buffers.get_mut(&endpoint).and_then(|b| b.pop())
    }

    /// Check if endpoint has buffered data
    pub fn has_data(&self, endpoint: u8) -> bool {
        self.buffers.get(&endpoint).map(|b| b.has_data()).unwrap_or(false)
    }

    /// Get the running flag for stopping background tasks
    pub fn running(&self) -> Arc<AtomicBool> {
        self.running.clone()
    }

    /// Stop all background polling
    pub fn stop(&self) {
        self.running.store(false, Ordering::Release);
    }
}

/// Global interrupt buffer manager
///
/// Manages interrupt buffers for all devices and coordinates background polling.
pub struct InterruptBufferManager {
    /// Per-device buffers, protected by mutex for thread-safe access
    device_buffers: Arc<Mutex<HashMap<DeviceId, DeviceInterruptBuffers>>>,
}

impl InterruptBufferManager {
    pub fn new() -> Self {
        Self {
            device_buffers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register a device and its interrupt endpoints for buffering
    pub fn register_device(&self, device_id: DeviceId, interrupt_endpoints: Vec<u8>) {
        let mut buffers = self.device_buffers.lock().unwrap();
        let device_buffers = buffers
            .entry(device_id)
            .or_insert_with(|| DeviceInterruptBuffers::new(device_id));

        for endpoint in interrupt_endpoints {
            device_buffers.register_endpoint(endpoint);
        }

        debug!(
            "Registered device {:?} with {} interrupt endpoints for buffering",
            device_id,
            device_buffers.buffers.len()
        );
    }

    /// Unregister a device (stops background polling)
    pub fn unregister_device(&self, device_id: DeviceId) {
        let mut buffers = self.device_buffers.lock().unwrap();
        if let Some(device_buffers) = buffers.remove(&device_id) {
            device_buffers.stop();
            debug!("Unregistered device {:?} from interrupt buffering", device_id);
        }
    }

    /// Push data to a device's endpoint buffer
    pub fn push(&self, device_id: DeviceId, endpoint: u8, data: Vec<u8>) {
        let mut buffers = self.device_buffers.lock().unwrap();
        if let Some(device_buffers) = buffers.get_mut(&device_id) {
            device_buffers.push(endpoint, data);
        }
    }

    /// Pop data from a device's endpoint buffer
    pub fn pop(&self, device_id: DeviceId, endpoint: u8) -> Option<InterruptReport> {
        let mut buffers = self.device_buffers.lock().unwrap();
        buffers.get_mut(&device_id).and_then(|db| db.pop(endpoint))
    }

    /// Check if a device's endpoint has buffered data
    pub fn has_data(&self, device_id: DeviceId, endpoint: u8) -> bool {
        let buffers = self.device_buffers.lock().unwrap();
        buffers.get(&device_id).map(|db| db.has_data(endpoint)).unwrap_or(false)
    }

    /// Get the running flag for a device's background tasks
    pub fn get_running_flag(&self, device_id: DeviceId) -> Option<Arc<AtomicBool>> {
        let buffers = self.device_buffers.lock().unwrap();
        buffers.get(&device_id).map(|db| db.running())
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
        let mut buffer = EndpointBuffer::new(0x81);

        buffer.push(vec![1, 2, 3]);
        buffer.push(vec![4, 5, 6]);

        assert_eq!(buffer.len(), 2);
        assert!(buffer.has_data());

        let report1 = buffer.pop().unwrap();
        assert_eq!(report1.seq, 0);
        assert_eq!(report1.data, vec![1, 2, 3]);

        let report2 = buffer.pop().unwrap();
        assert_eq!(report2.seq, 1);
        assert_eq!(report2.data, vec![4, 5, 6]);

        assert!(!buffer.has_data());
        assert!(buffer.pop().is_none());
    }

    #[test]
    fn test_endpoint_buffer_overflow() {
        let mut buffer = EndpointBuffer::new(0x81);

        // Fill beyond capacity
        for i in 0..MAX_BUFFERED_REPORTS + 5 {
            buffer.push(vec![i as u8]);
        }

        // Should have dropped oldest entries
        assert_eq!(buffer.len(), MAX_BUFFERED_REPORTS);
        let (received, dropped) = buffer.stats();
        assert_eq!(received, MAX_BUFFERED_REPORTS as u64 + 5);
        assert_eq!(dropped, 5);

        // First available should be the 6th one we pushed (seq 5)
        let report = buffer.pop().unwrap();
        assert_eq!(report.seq, 5);
    }

    #[test]
    fn test_device_interrupt_buffers() {
        let mut device_buffers = DeviceInterruptBuffers::new(DeviceId(1));

        device_buffers.register_endpoint(0x81);
        device_buffers.register_endpoint(0x82);

        device_buffers.push(0x81, vec![1, 2, 3]);
        device_buffers.push(0x82, vec![4, 5, 6]);

        assert!(device_buffers.has_data(0x81));
        assert!(device_buffers.has_data(0x82));
        assert!(!device_buffers.has_data(0x83)); // Not registered

        let report1 = device_buffers.pop(0x81).unwrap();
        assert_eq!(report1.data, vec![1, 2, 3]);

        let report2 = device_buffers.pop(0x82).unwrap();
        assert_eq!(report2.data, vec![4, 5, 6]);
    }

    #[test]
    fn test_interrupt_buffer_manager() {
        let manager = InterruptBufferManager::new();

        manager.register_device(DeviceId(1), vec![0x81, 0x82]);
        manager.register_device(DeviceId(2), vec![0x83]);

        manager.push(DeviceId(1), 0x81, vec![1, 2, 3]);
        manager.push(DeviceId(2), 0x83, vec![4, 5, 6]);

        assert!(manager.has_data(DeviceId(1), 0x81));
        assert!(!manager.has_data(DeviceId(1), 0x82)); // No data pushed
        assert!(manager.has_data(DeviceId(2), 0x83));

        let report = manager.pop(DeviceId(1), 0x81).unwrap();
        assert_eq!(report.data, vec![1, 2, 3]);

        manager.unregister_device(DeviceId(1));
        assert!(!manager.has_data(DeviceId(1), 0x81)); // Device removed
    }
}
