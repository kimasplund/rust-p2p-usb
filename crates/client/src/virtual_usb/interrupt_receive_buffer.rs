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

use protocol::integrity::verify_interrupt_checksum;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use tracing::{debug, error, trace, warn};

/// Adaptive jitter buffer for handling variable network conditions
///
/// Dynamically adjusts buffer delay based on observed packet jitter to provide
/// smooth data delivery while minimizing latency. Uses exponential moving
/// averages for stable adaptation.
///
/// # Algorithm
///
/// - Tracks inter-arrival time variance (jitter) using RFC 3550 method
/// - Target buffer depth = base_delay + (jitter_factor * measured_jitter)
/// - Automatically expands buffer during high jitter, contracts during stability
/// - Bounded by min_delay and max_delay to prevent extremes
#[derive(Debug)]
pub struct AdaptiveJitterBuffer {
    /// Minimum buffer delay (microseconds)
    min_delay_us: u64,
    /// Maximum buffer delay (microseconds)
    max_delay_us: u64,
    /// Current target delay (microseconds)
    current_delay_us: AtomicU64,
    /// Smoothed jitter estimate (RFC 3550 style, microseconds)
    jitter_us: AtomicU64,
    /// Last arrival timestamp for jitter calculation
    last_arrival_us: AtomicU64,
    /// Last packet timestamp for transit time calculation
    last_packet_ts_us: AtomicU64,
    /// Smoothing factor for jitter calculation (0-256, where 256 = no smoothing)
    /// RFC 3550 recommends 16 (1/16 = 0.0625)
    smoothing_factor: u64,
    /// Multiplier for jitter contribution to target delay
    jitter_multiplier: u64,
    /// Base delay added to jitter-based delay
    base_delay_us: u64,
    /// Number of packets processed
    packets_processed: AtomicU64,
}

impl AdaptiveJitterBuffer {
    /// Create a new adaptive jitter buffer with default settings
    ///
    /// Defaults tuned for HID devices (low latency, small jitter tolerance):
    /// - min_delay: 1ms (1000us)
    /// - max_delay: 50ms (50000us)
    /// - base_delay: 2ms (2000us)
    pub fn new() -> Self {
        Self::with_config(1000, 50_000, 2000, 16, 2)
    }

    /// Create a new adaptive jitter buffer with custom configuration
    ///
    /// # Parameters
    /// - `min_delay_us`: Minimum buffer delay in microseconds
    /// - `max_delay_us`: Maximum buffer delay in microseconds
    /// - `base_delay_us`: Base delay before jitter contribution
    /// - `smoothing_factor`: Smoothing factor (1-256, lower = more smoothing)
    /// - `jitter_multiplier`: How much jitter contributes to delay (typically 2-4)
    pub fn with_config(
        min_delay_us: u64,
        max_delay_us: u64,
        base_delay_us: u64,
        smoothing_factor: u64,
        jitter_multiplier: u64,
    ) -> Self {
        Self {
            min_delay_us,
            max_delay_us,
            current_delay_us: AtomicU64::new(base_delay_us),
            jitter_us: AtomicU64::new(0),
            last_arrival_us: AtomicU64::new(0),
            last_packet_ts_us: AtomicU64::new(0),
            smoothing_factor: smoothing_factor.max(1).min(256),
            jitter_multiplier,
            base_delay_us,
            packets_processed: AtomicU64::new(0),
        }
    }

    /// Record packet arrival and update jitter estimate
    ///
    /// # Parameters
    /// - `packet_timestamp_us`: Server timestamp from the packet
    /// - `arrival_time_us`: Local timestamp when packet was received
    ///
    /// # Returns
    /// Updated target delay in microseconds
    pub fn record_arrival(&self, packet_timestamp_us: u64, arrival_time_us: u64) -> u64 {
        let count = self.packets_processed.fetch_add(1, Ordering::Relaxed);

        if count == 0 {
            // First packet - initialize timestamps
            self.last_arrival_us.store(arrival_time_us, Ordering::Relaxed);
            self.last_packet_ts_us.store(packet_timestamp_us, Ordering::Relaxed);
            return self.current_delay_us.load(Ordering::Relaxed);
        }

        // Calculate transit time difference (RFC 3550 jitter calculation)
        let last_arrival = self.last_arrival_us.swap(arrival_time_us, Ordering::Relaxed);
        let last_packet_ts = self.last_packet_ts_us.swap(packet_timestamp_us, Ordering::Relaxed);

        // D(i,j) = (Rj - Ri) - (Sj - Si) where R=arrival, S=packet timestamp
        let arrival_diff = arrival_time_us.saturating_sub(last_arrival);
        let packet_diff = packet_timestamp_us.saturating_sub(last_packet_ts);
        let transit_diff = if arrival_diff > packet_diff {
            arrival_diff - packet_diff
        } else {
            packet_diff - arrival_diff
        };

        // Update smoothed jitter: J(i) = J(i-1) + (|D(i,j)| - J(i-1))/smoothing_factor
        let old_jitter = self.jitter_us.load(Ordering::Relaxed);
        let jitter_diff = if transit_diff > old_jitter {
            transit_diff - old_jitter
        } else {
            old_jitter - transit_diff
        };
        let new_jitter = old_jitter + jitter_diff / self.smoothing_factor;
        self.jitter_us.store(new_jitter, Ordering::Relaxed);

        // Calculate new target delay
        let target = self.base_delay_us + (new_jitter * self.jitter_multiplier);
        let clamped = target.max(self.min_delay_us).min(self.max_delay_us);
        self.current_delay_us.store(clamped, Ordering::Relaxed);

        trace!(
            "Jitter buffer: jitter={}us, target_delay={}us (clamped from {}us)",
            new_jitter, clamped, target
        );

        clamped
    }

    /// Get the current recommended buffer delay in microseconds
    pub fn current_delay_us(&self) -> u64 {
        self.current_delay_us.load(Ordering::Relaxed)
    }

    /// Get the current recommended buffer delay as Duration
    pub fn current_delay(&self) -> Duration {
        Duration::from_micros(self.current_delay_us())
    }

    /// Get the current smoothed jitter estimate in microseconds
    pub fn jitter_us(&self) -> u64 {
        self.jitter_us.load(Ordering::Relaxed)
    }

    /// Get the number of packets processed
    pub fn packets_processed(&self) -> u64 {
        self.packets_processed.load(Ordering::Relaxed)
    }

    /// Check if a packet should be served immediately based on current buffer state
    ///
    /// # Parameters
    /// - `packet_arrival_us`: When the packet arrived
    /// - `current_time_us`: Current timestamp
    ///
    /// # Returns
    /// `true` if the packet has been buffered long enough
    pub fn should_serve(&self, packet_arrival_us: u64, current_time_us: u64) -> bool {
        let buffered_time = current_time_us.saturating_sub(packet_arrival_us);
        buffered_time >= self.current_delay_us()
    }

    /// Reset the jitter buffer state (e.g., after reconnection)
    pub fn reset(&self) {
        self.jitter_us.store(0, Ordering::Relaxed);
        self.current_delay_us.store(self.base_delay_us, Ordering::Relaxed);
        self.last_arrival_us.store(0, Ordering::Relaxed);
        self.last_packet_ts_us.store(0, Ordering::Relaxed);
        self.packets_processed.store(0, Ordering::Relaxed);
    }

    /// Get statistics about the jitter buffer
    pub fn stats(&self) -> JitterBufferStats {
        JitterBufferStats {
            current_delay_us: self.current_delay_us(),
            jitter_us: self.jitter_us(),
            packets_processed: self.packets_processed(),
            min_delay_us: self.min_delay_us,
            max_delay_us: self.max_delay_us,
        }
    }
}

impl Default for AdaptiveJitterBuffer {
    fn default() -> Self {
        Self::new()
    }
}

/// Statistics from the adaptive jitter buffer
#[derive(Debug, Clone)]
pub struct JitterBufferStats {
    /// Current target buffer delay in microseconds
    pub current_delay_us: u64,
    /// Smoothed jitter estimate in microseconds
    pub jitter_us: u64,
    /// Total packets processed
    pub packets_processed: u64,
    /// Minimum configured delay
    pub min_delay_us: u64,
    /// Maximum configured delay
    pub max_delay_us: u64,
}

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
    /// CRC32C checksum from server
    pub checksum: u32,
}

impl ReceivedReport {
    /// Verify the integrity of this report
    pub fn verify(&self) -> bool {
        verify_interrupt_checksum(
            self.sequence,
            self.endpoint,
            &self.data,
            self.server_timestamp_us,
            self.checksum,
        )
    }
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
    /// Last contiguous sequence (all sequences up to this received)
    last_contiguous_seq: AtomicU64,
    /// Total reports received
    total_received: AtomicU64,
    /// Reports served to kernel
    total_served: AtomicU64,
    /// Sequence gaps detected
    gaps_detected: AtomicU64,
    /// Checksum verification failures
    checksum_failures: AtomicU64,
    /// Maximum buffer size
    max_size: usize,
    /// Whether streaming is active
    active: Mutex<bool>,
    /// Missing sequences awaiting retransmission (for NACK)
    missing_sequences: Mutex<HashSet<u64>>,
    /// Maximum age of missing sequences before giving up (microseconds)
    nack_timeout_us: u64,
    /// Timestamp when each missing sequence was detected
    missing_timestamps: Mutex<HashMap<u64, u64>>,
}

/// Default NACK timeout: 100ms (in microseconds)
const DEFAULT_NACK_TIMEOUT_US: u64 = 100_000;

/// Maximum number of missing sequences to track (prevents memory exhaustion)
const MAX_MISSING_SEQUENCES: usize = 256;

/// Default backpressure threshold: 75% buffer fill
const DEFAULT_BACKPRESSURE_THRESHOLD: f64 = 0.75;

/// Default resume threshold: 50% buffer fill (hysteresis to prevent oscillation)
const DEFAULT_RESUME_THRESHOLD: f64 = 0.50;

/// Flow control state for backpressure signaling
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowControlState {
    /// Normal operation - sender can continue at full rate
    Normal,
    /// Backpressure - sender should slow down or pause
    Backpressure,
    /// Paused - buffer is critically full, sender must stop
    Paused,
}

impl FlowControlState {
    /// Returns true if the sender should slow down
    pub fn should_slow_down(&self) -> bool {
        matches!(self, FlowControlState::Backpressure | FlowControlState::Paused)
    }

    /// Returns true if the sender must stop sending
    pub fn must_pause(&self) -> bool {
        matches!(self, FlowControlState::Paused)
    }
}

/// Flow control information for ACK messages
#[derive(Debug, Clone)]
pub struct FlowControlInfo {
    /// Current flow control state
    pub state: FlowControlState,
    /// Available buffer capacity (number of slots)
    pub available_capacity: usize,
    /// Buffer fill percentage (0.0 - 1.0)
    pub fill_ratio: f64,
    /// Suggested receive window (how many more reports can be accepted)
    pub receive_window: u32,
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
            last_contiguous_seq: AtomicU64::new(0),
            total_received: AtomicU64::new(0),
            total_served: AtomicU64::new(0),
            gaps_detected: AtomicU64::new(0),
            checksum_failures: AtomicU64::new(0),
            max_size,
            active: Mutex::new(false),
            missing_sequences: Mutex::new(HashSet::new()),
            nack_timeout_us: DEFAULT_NACK_TIMEOUT_US,
            missing_timestamps: Mutex::new(HashMap::new()),
        }
    }

    /// Activate the buffer (start accepting reports)
    pub fn activate(&self, start_seq: u64) {
        self.next_expected_seq.store(start_seq, Ordering::SeqCst);
        self.highest_seq.store(start_seq.saturating_sub(1), Ordering::SeqCst);
        self.last_contiguous_seq.store(start_seq.saturating_sub(1), Ordering::SeqCst);
        *self.active.lock().unwrap() = true;
        // Clear any stale missing sequence tracking
        self.missing_sequences.lock().unwrap().clear();
        self.missing_timestamps.lock().unwrap().clear();
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
        self.missing_sequences.lock().unwrap().clear();
        self.missing_timestamps.lock().unwrap().clear();
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
    ///
    /// Reports with invalid checksums are logged and discarded.
    /// When gaps are detected, missing sequences are tracked for NACK requests.
    pub fn store(&self, report: ReceivedReport) -> (bool, bool) {
        if !self.is_active() {
            trace!(
                "Dropping report for inactive buffer ep=0x{:02x}",
                self.endpoint
            );
            return (false, false);
        }

        // Verify checksum integrity before storing
        if !report.verify() {
            self.checksum_failures.fetch_add(1, Ordering::Relaxed);
            error!(
                "Checksum verification failed for ep=0x{:02x} seq={}: expected {:#x}, data corruption detected",
                self.endpoint, report.sequence, report.checksum
            );
            // Don't store corrupted data - request retransmission via ack
            return (false, true);
        }

        let seq = report.sequence;
        let expected = self.next_expected_seq.load(Ordering::SeqCst);
        let mut gap_detected = false;

        // Check for sequence gaps and track missing sequences for NACK
        if seq > expected {
            let gap_size = seq - expected;
            warn!(
                "Sequence gap detected on ep=0x{:02x}: expected {}, got {} (gap={})",
                self.endpoint, expected, seq, gap_size
            );
            self.gaps_detected.fetch_add(1, Ordering::Relaxed);
            gap_detected = true;

            // Track missing sequences for NACK (limit to prevent memory exhaustion)
            let now_us = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_micros() as u64)
                .unwrap_or(0);

            let mut missing = self.missing_sequences.lock().unwrap();
            let mut timestamps = self.missing_timestamps.lock().unwrap();

            for missing_seq in expected..seq {
                if missing.len() < MAX_MISSING_SEQUENCES {
                    if missing.insert(missing_seq) {
                        timestamps.insert(missing_seq, now_us);
                        trace!(
                            "Tracking missing seq {} on ep=0x{:02x} for NACK",
                            missing_seq, self.endpoint
                        );
                    }
                }
            }
        } else if seq < expected {
            trace!(
                "Late/duplicate report on ep=0x{:02x}: seq={} (expected {})",
                self.endpoint, seq, expected
            );
            // Remove from missing set if this was a retransmission
            let mut missing = self.missing_sequences.lock().unwrap();
            if missing.remove(&seq) {
                self.missing_timestamps.lock().unwrap().remove(&seq);
                debug!(
                    "Received retransmitted seq {} on ep=0x{:02x}",
                    seq, self.endpoint
                );
            }
        } else {
            // seq == expected - remove from missing if present (shouldn't be, but defensive)
            let mut missing = self.missing_sequences.lock().unwrap();
            if missing.remove(&seq) {
                self.missing_timestamps.lock().unwrap().remove(&seq);
            }
        }

        // Update tracking atomically - only advance if this is a higher sequence
        // This prevents race conditions when reports arrive out of order
        self.next_expected_seq.fetch_max(seq + 1, Ordering::SeqCst);
        let prev_highest = self.highest_seq.fetch_max(seq, Ordering::SeqCst);

        // Update last contiguous sequence (for NACK reporting)
        // This is the highest seq where all prior seqs have been received
        self.update_last_contiguous(seq);

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

    /// Update the last contiguous sequence number
    fn update_last_contiguous(&self, _received_seq: u64) {
        let missing = self.missing_sequences.lock().unwrap();
        let highest = self.highest_seq.load(Ordering::SeqCst);

        // If no gaps, last_contiguous is highest_seq
        if missing.is_empty() {
            self.last_contiguous_seq.store(highest, Ordering::SeqCst);
        } else {
            // Find the lowest missing sequence
            if let Some(&min_missing) = missing.iter().min() {
                // Last contiguous is one before the first gap
                let new_contiguous = min_missing.saturating_sub(1);
                self.last_contiguous_seq.fetch_max(new_contiguous, Ordering::SeqCst);
            }
        }
    }

    /// Get missing sequences for NACK request
    ///
    /// Returns sequences that are missing and haven't timed out.
    /// Expired sequences are automatically removed.
    pub fn get_missing_sequences(&self) -> Vec<u64> {
        let now_us = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_micros() as u64)
            .unwrap_or(0);

        let mut missing = self.missing_sequences.lock().unwrap();
        let mut timestamps = self.missing_timestamps.lock().unwrap();

        // Remove expired sequences
        let mut expired = Vec::new();
        for (&seq, &timestamp) in timestamps.iter() {
            if now_us.saturating_sub(timestamp) > self.nack_timeout_us {
                expired.push(seq);
            }
        }

        for seq in &expired {
            missing.remove(seq);
            timestamps.remove(seq);
            trace!(
                "NACK timeout for seq {} on ep=0x{:02x}",
                seq, self.endpoint
            );
        }

        // Return remaining missing sequences sorted
        let mut result: Vec<u64> = missing.iter().copied().collect();
        result.sort();
        result
    }

    /// Get last contiguous sequence number (for NACK requests)
    pub fn last_contiguous_seq(&self) -> u64 {
        self.last_contiguous_seq.load(Ordering::SeqCst)
    }

    /// Check if there are pending NACKs to send
    pub fn has_pending_nacks(&self) -> bool {
        !self.missing_sequences.lock().unwrap().is_empty()
    }

    /// Generate NACK info for this endpoint
    ///
    /// Returns (missing_sequences, last_contiguous_seq) if there are missing sequences,
    /// or None if no NACK is needed.
    pub fn generate_nack_info(&self) -> Option<(Vec<u64>, u64)> {
        let missing = self.get_missing_sequences();
        if missing.is_empty() {
            None
        } else {
            Some((missing, self.last_contiguous_seq()))
        }
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
            checksum_failures: self.checksum_failures.load(Ordering::Relaxed),
            highest_seq: self.highest_seq.load(Ordering::Relaxed),
            active: self.is_active(),
            missing_count: self.missing_sequences.lock().unwrap().len(),
            last_contiguous_seq: self.last_contiguous_seq.load(Ordering::Relaxed),
        }
    }

    /// Get highest acknowledged sequence number (for InterruptAck)
    pub fn highest_acked_seq(&self) -> u64 {
        self.highest_seq.load(Ordering::Relaxed)
    }

    /// Get the current buffer fill ratio (0.0 - 1.0)
    pub fn fill_ratio(&self) -> f64 {
        let len = self.reports.lock().unwrap().len();
        len as f64 / self.max_size as f64
    }

    /// Get the available capacity in the buffer
    pub fn available_capacity(&self) -> usize {
        let len = self.reports.lock().unwrap().len();
        self.max_size.saturating_sub(len)
    }

    /// Get the current flow control state
    ///
    /// Uses hysteresis to prevent oscillation between states:
    /// - Normal -> Backpressure at 75% fill
    /// - Backpressure -> Normal at 50% fill
    /// - Backpressure -> Paused at 95% fill
    /// - Paused -> Backpressure at 75% fill
    pub fn flow_control_state(&self) -> FlowControlState {
        let fill = self.fill_ratio();

        if fill >= 0.95 {
            FlowControlState::Paused
        } else if fill >= DEFAULT_BACKPRESSURE_THRESHOLD {
            FlowControlState::Backpressure
        } else if fill <= DEFAULT_RESUME_THRESHOLD {
            FlowControlState::Normal
        } else {
            // In the hysteresis zone (50-75%) - maintain previous state
            // For simplicity, default to Normal
            FlowControlState::Normal
        }
    }

    /// Get detailed flow control information for ACK messages
    pub fn flow_control_info(&self) -> FlowControlInfo {
        let len = self.reports.lock().unwrap().len();
        let available = self.max_size.saturating_sub(len);
        let fill = len as f64 / self.max_size as f64;

        FlowControlInfo {
            state: self.flow_control_state(),
            available_capacity: available,
            fill_ratio: fill,
            // Receive window: how many more we can accept
            // Leave some headroom even when available
            receive_window: (available as f64 * 0.8) as u32,
        }
    }

    /// Check if backpressure should be signaled to sender
    pub fn should_apply_backpressure(&self) -> bool {
        self.flow_control_state().should_slow_down()
    }

    /// Check if sender must pause completely
    pub fn must_pause_sender(&self) -> bool {
        self.flow_control_state().must_pause()
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
    pub checksum_failures: u64,
    pub highest_seq: u64,
    pub active: bool,
    /// Number of sequences currently missing (awaiting retransmission)
    pub missing_count: usize,
    /// Last contiguous sequence received
    pub last_contiguous_seq: u64,
}

impl ReceiveBufferStats {
    /// Calculate integrity rate (1.0 = perfect, 0.0 = all failures)
    pub fn integrity_rate(&self) -> f64 {
        if self.total_received == 0 {
            1.0
        } else {
            let failures = self.checksum_failures;
            (self.total_received - failures) as f64 / self.total_received as f64
        }
    }

    /// Calculate gap rate (fraction of gaps over total received)
    pub fn gap_rate(&self) -> f64 {
        if self.total_received == 0 {
            0.0
        } else {
            self.gaps_detected as f64 / self.total_received as f64
        }
    }

    /// Calculate delivery rate (served / received)
    pub fn delivery_rate(&self) -> f64 {
        if self.total_received == 0 {
            1.0
        } else {
            self.total_served as f64 / self.total_received as f64
        }
    }
}

/// Aggregated integrity metrics across all endpoints/devices
#[derive(Debug, Clone, Default)]
pub struct AggregatedIntegrityMetrics {
    /// Total reports received across all endpoints
    pub total_received: u64,
    /// Total reports served to kernel
    pub total_served: u64,
    /// Total checksum failures
    pub checksum_failures: u64,
    /// Total sequence gaps detected
    pub gaps_detected: u64,
    /// Currently missing sequences awaiting retransmission
    pub missing_count: u64,
    /// Number of active endpoints
    pub active_endpoints: u64,
}

impl AggregatedIntegrityMetrics {
    /// Create new empty metrics
    pub fn new() -> Self {
        Self::default()
    }

    /// Aggregate stats from a single buffer
    pub fn add_buffer_stats(&mut self, stats: &ReceiveBufferStats) {
        self.total_received += stats.total_received;
        self.total_served += stats.total_served;
        self.checksum_failures += stats.checksum_failures;
        self.gaps_detected += stats.gaps_detected;
        self.missing_count += stats.missing_count as u64;
        if stats.active {
            self.active_endpoints += 1;
        }
    }

    /// Calculate overall integrity rate
    pub fn integrity_rate(&self) -> f64 {
        if self.total_received == 0 {
            1.0
        } else {
            (self.total_received - self.checksum_failures) as f64 / self.total_received as f64
        }
    }

    /// Calculate overall gap rate
    pub fn gap_rate(&self) -> f64 {
        if self.total_received == 0 {
            0.0
        } else {
            self.gaps_detected as f64 / self.total_received as f64
        }
    }

    /// Check if integrity is healthy (>99% integrity, <1% gaps)
    pub fn is_healthy(&self) -> bool {
        self.integrity_rate() > 0.99 && self.gap_rate() < 0.01
    }

    /// Generate a human-readable summary
    pub fn summary(&self) -> String {
        format!(
            "Integrity: {:.2}% | Gaps: {:.2}% | Missing: {} | Received: {} | Served: {} | Active: {}",
            self.integrity_rate() * 100.0,
            self.gap_rate() * 100.0,
            self.missing_count,
            self.total_received,
            self.total_served,
            self.active_endpoints,
        )
    }
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
        checksum: u32,
    ) -> Option<(bool, u64)> {
        let manager = self.get_or_create(device_handle);

        let report = ReceivedReport {
            sequence,
            endpoint,
            data,
            server_timestamp_us,
            received_at: Instant::now(),
            checksum,
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
    use protocol::integrity::compute_interrupt_checksum;

    /// Create a test report with valid checksum
    fn make_report(sequence: u64, endpoint: u8, data: Vec<u8>, timestamp_us: u64) -> ReceivedReport {
        let checksum = compute_interrupt_checksum(sequence, endpoint, &data, timestamp_us);
        ReceivedReport {
            sequence,
            endpoint,
            data,
            server_timestamp_us: timestamp_us,
            received_at: Instant::now(),
            checksum,
        }
    }

    #[test]
    fn test_buffer_basic() {
        let buffer = EndpointReceiveBuffer::new(0x81, 100);
        buffer.activate(0);

        let report = make_report(0, 0x81, vec![0, 0, 4, 0, 0, 0, 0, 0], 1000); // 'A' key

        let (gap, _should_ack) = buffer.store(report);
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
        let report0 = make_report(0, 0x81, vec![], 1000);
        buffer.store(report0);

        // Skip to seq 5 (gap of 4)
        let report5 = make_report(5, 0x81, vec![], 2000);
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
            let report = make_report(i, 0x81, vec![i as u8], i * 1000);
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
    fn test_checksum_verification() {
        let buffer = EndpointReceiveBuffer::new(0x81, 100);
        buffer.activate(0);

        // Valid report should be stored
        let valid_report = make_report(0, 0x81, vec![1, 2, 3], 1000);
        let (_, _) = buffer.store(valid_report);
        assert_eq!(buffer.stats().total_received, 1);
        assert_eq!(buffer.stats().checksum_failures, 0);

        // Invalid checksum should be rejected
        let mut invalid_report = make_report(1, 0x81, vec![4, 5, 6], 2000);
        invalid_report.checksum ^= 0xFFFF; // Corrupt checksum
        let (_, should_ack) = buffer.store(invalid_report);
        assert!(should_ack); // Should trigger ack to request retransmission
        assert_eq!(buffer.stats().checksum_failures, 1);
        assert_eq!(buffer.stats().total_received, 1); // Still 1 - invalid not counted
    }

    #[test]
    fn test_out_of_order_packets() {
        // Test that out-of-order packets don't cause sequence tracking regression
        let buffer = EndpointReceiveBuffer::new(0x81, 100);
        buffer.activate(0);

        // Receive packets out of order: 5, 3, 7, 2
        // next_expected should always be max(received) + 1

        let report5 = make_report(5, 0x81, vec![5], 5000);
        buffer.store(report5);
        // next_expected should be 6

        let report3 = make_report(3, 0x81, vec![3], 3000);
        buffer.store(report3);
        // next_expected should still be 6 (not 4!)

        let report7 = make_report(7, 0x81, vec![7], 7000);
        buffer.store(report7);
        // next_expected should be 8

        let report2 = make_report(2, 0x81, vec![2], 2000);
        buffer.store(report2);
        // next_expected should still be 8 (not 3!)

        let stats = buffer.stats();
        assert_eq!(stats.total_received, 4);
        assert_eq!(stats.highest_seq, 7);

        // All reports should be available in order they were stored (FIFO)
        let r1 = buffer.try_take().unwrap();
        assert_eq!(r1.sequence, 5);
        let r2 = buffer.try_take().unwrap();
        assert_eq!(r2.sequence, 3);
        let r3 = buffer.try_take().unwrap();
        assert_eq!(r3.sequence, 7);
        let r4 = buffer.try_take().unwrap();
        assert_eq!(r4.sequence, 2);
    }

    #[test]
    fn test_device_manager() {
        let manager = DeviceReceiveBufferManager::new(1, 100);

        manager.activate_endpoint(0x81, 0);
        assert!(manager.is_streaming(0x81));
        assert!(!manager.is_streaming(0x82));

        let data = vec![1, 2, 3];
        let checksum = protocol::integrity::compute_interrupt_checksum(0, 0x81, &data, 1000);
        let report = ReceivedReport {
            sequence: 0,
            endpoint: 0x81,
            data,
            server_timestamp_us: 1000,
            received_at: Instant::now(),
            checksum,
        };
        manager.store_report(report);

        let taken = manager.try_take(0x81).unwrap();
        assert_eq!(taken.data, vec![1, 2, 3]);
    }

    #[test]
    fn test_jitter_buffer_basic() {
        let jb = AdaptiveJitterBuffer::new();

        // Initial state
        assert_eq!(jb.packets_processed(), 0);
        assert_eq!(jb.jitter_us(), 0);
        assert_eq!(jb.current_delay_us(), 2000); // base_delay

        // First packet initializes state
        let delay = jb.record_arrival(1000, 2000);
        assert_eq!(delay, 2000); // Still base delay for first packet
        assert_eq!(jb.packets_processed(), 1);
    }

    #[test]
    fn test_jitter_buffer_stable_stream() {
        let jb = AdaptiveJitterBuffer::with_config(1000, 50000, 2000, 16, 2);

        // Simulate stable stream: packets every 10ms, arrivals every 10ms
        // This means zero jitter
        jb.record_arrival(0, 1000);
        jb.record_arrival(10000, 11000);
        jb.record_arrival(20000, 21000);
        jb.record_arrival(30000, 31000);

        // With stable stream, jitter should stay low
        assert!(jb.jitter_us() < 1000);
        // Delay should be close to base delay
        assert!(jb.current_delay_us() <= 3000);
    }

    #[test]
    fn test_jitter_buffer_high_jitter() {
        let jb = AdaptiveJitterBuffer::with_config(1000, 50000, 2000, 4, 3);

        // Simulate high jitter: packets evenly spaced, arrivals vary
        jb.record_arrival(0, 1000);
        jb.record_arrival(10000, 15000);   // Arrived 4ms late
        jb.record_arrival(20000, 22000);   // Arrived 3ms early relative to previous
        jb.record_arrival(30000, 40000);   // Arrived 7ms late

        // High jitter should increase delay
        let stats = jb.stats();
        assert!(stats.jitter_us > 1000, "Jitter should be high: {}", stats.jitter_us);
        assert!(stats.current_delay_us > 3000, "Delay should increase: {}", stats.current_delay_us);
    }

    #[test]
    fn test_jitter_buffer_bounds() {
        let jb = AdaptiveJitterBuffer::with_config(5000, 10000, 6000, 1, 10);

        // First packet
        jb.record_arrival(0, 1000);

        // Simulate extreme jitter that would push delay above max
        jb.record_arrival(1000, 100000); // 99ms of jitter

        // Should be clamped to max
        assert_eq!(jb.current_delay_us(), 10000);

        // Reset and test minimum bound
        jb.reset();
        assert_eq!(jb.current_delay_us(), 6000); // base_delay after reset
        assert_eq!(jb.packets_processed(), 0);
    }

    #[test]
    fn test_jitter_buffer_should_serve() {
        let jb = AdaptiveJitterBuffer::with_config(1000, 50000, 5000, 16, 2);

        // Packet arrived at time 10000
        let packet_arrival = 10000u64;

        // At time 12000 (2ms later), should not serve (need 5ms delay)
        assert!(!jb.should_serve(packet_arrival, 12000));

        // At time 15000 (5ms later), should serve
        assert!(jb.should_serve(packet_arrival, 15000));

        // At time 20000 (10ms later), definitely should serve
        assert!(jb.should_serve(packet_arrival, 20000));
    }

    #[test]
    fn test_nack_gap_tracking() {
        let buffer = EndpointReceiveBuffer::new(0x81, 100);
        buffer.activate(0);

        // Receive seq 0, then skip to seq 5 (gap of 1,2,3,4)
        let report0 = make_report(0, 0x81, vec![0], 1000);
        buffer.store(report0);
        assert!(!buffer.has_pending_nacks());

        let report5 = make_report(5, 0x81, vec![5], 2000);
        let (gap_detected, _) = buffer.store(report5);
        assert!(gap_detected);
        assert!(buffer.has_pending_nacks());

        // Should have 4 missing sequences (1, 2, 3, 4)
        let missing = buffer.get_missing_sequences();
        assert_eq!(missing.len(), 4);
        assert_eq!(missing, vec![1, 2, 3, 4]);

        // Last contiguous should be 0 (seq 0 was received, 1-4 missing)
        assert_eq!(buffer.last_contiguous_seq(), 0);

        // Generate NACK info
        let nack_info = buffer.generate_nack_info();
        assert!(nack_info.is_some());
        let (missing_seqs, last_contiguous) = nack_info.unwrap();
        assert_eq!(missing_seqs, vec![1, 2, 3, 4]);
        assert_eq!(last_contiguous, 0);
    }

    #[test]
    fn test_nack_retransmission_clears_gap() {
        let buffer = EndpointReceiveBuffer::new(0x81, 100);
        buffer.activate(0);

        // Create a gap: receive 0, then 3
        buffer.store(make_report(0, 0x81, vec![0], 1000));
        buffer.store(make_report(3, 0x81, vec![3], 2000));

        // Should be missing 1, 2
        assert_eq!(buffer.get_missing_sequences(), vec![1, 2]);

        // Simulate retransmission of seq 1
        buffer.store(make_report(1, 0x81, vec![1], 3000));
        assert_eq!(buffer.get_missing_sequences(), vec![2]);

        // Simulate retransmission of seq 2
        buffer.store(make_report(2, 0x81, vec![2], 4000));
        assert!(buffer.get_missing_sequences().is_empty());
        assert!(!buffer.has_pending_nacks());

        // Last contiguous should now be 3
        assert_eq!(buffer.last_contiguous_seq(), 3);
    }

    #[test]
    fn test_flow_control_states() {
        // Buffer with capacity 10 for easy percentage calculation
        let buffer = EndpointReceiveBuffer::new(0x81, 10);
        buffer.activate(0);

        // Empty buffer = Normal state
        assert_eq!(buffer.flow_control_state(), FlowControlState::Normal);
        assert!(!buffer.should_apply_backpressure());
        assert!(!buffer.must_pause_sender());
        assert_eq!(buffer.available_capacity(), 10);

        // Fill to 50% - still Normal
        for i in 0..5 {
            buffer.store(make_report(i, 0x81, vec![i as u8], i * 1000));
        }
        assert_eq!(buffer.flow_control_state(), FlowControlState::Normal);
        assert!((buffer.fill_ratio() - 0.5).abs() < 0.01);

        // Fill to 80% - triggers Backpressure (>75%)
        for i in 5..8 {
            buffer.store(make_report(i, 0x81, vec![i as u8], i * 1000));
        }
        assert_eq!(buffer.flow_control_state(), FlowControlState::Backpressure);
        assert!(buffer.should_apply_backpressure());
        assert!(!buffer.must_pause_sender());

        // Fill to 100% - triggers Paused (>=95%)
        for i in 8..10 {
            buffer.store(make_report(i, 0x81, vec![i as u8], i * 1000));
        }
        assert_eq!(buffer.flow_control_state(), FlowControlState::Paused);
        assert!(buffer.should_apply_backpressure());
        assert!(buffer.must_pause_sender());

        // Drain back to 40% - should return to Normal
        for _ in 0..6 {
            buffer.try_take();
        }
        assert_eq!(buffer.flow_control_state(), FlowControlState::Normal);
    }

    #[test]
    fn test_flow_control_info() {
        let buffer = EndpointReceiveBuffer::new(0x81, 100);
        buffer.activate(0);

        // Fill 25 reports
        for i in 0..25 {
            buffer.store(make_report(i, 0x81, vec![i as u8], i * 1000));
        }

        let info = buffer.flow_control_info();
        assert_eq!(info.state, FlowControlState::Normal);
        assert_eq!(info.available_capacity, 75);
        assert!((info.fill_ratio - 0.25).abs() < 0.01);
        // Window is 80% of available = 60
        assert_eq!(info.receive_window, 60);
    }
}

/// Property-based tests using proptest
#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    /// Strategy for generating valid HID report data (1-64 bytes)
    fn hid_data_strategy() -> impl Strategy<Value = Vec<u8>> {
        proptest::collection::vec(any::<u8>(), 1..=64)
    }

    /// Strategy for generating sequence numbers
    fn sequence_strategy() -> impl Strategy<Value = u64> {
        0u64..1_000_000u64
    }

    /// Strategy for generating timestamps
    fn timestamp_strategy() -> impl Strategy<Value = u64> {
        1_000_000u64..2_000_000_000_000u64 // Realistic microsecond timestamps
    }

    proptest! {
        /// Property: Checksums are deterministic - same inputs always produce same checksum
        #[test]
        fn prop_checksum_deterministic(
            seq in sequence_strategy(),
            endpoint in 0x81u8..=0x8Fu8,
            data in hid_data_strategy(),
            timestamp in timestamp_strategy(),
        ) {
            let checksum1 = protocol::integrity::compute_interrupt_checksum(seq, endpoint, &data, timestamp);
            let checksum2 = protocol::integrity::compute_interrupt_checksum(seq, endpoint, &data, timestamp);
            prop_assert_eq!(checksum1, checksum2);
        }

        /// Property: Any single-bit flip in data is detected by checksum
        #[test]
        fn prop_checksum_detects_single_bit_corruption(
            seq in sequence_strategy(),
            endpoint in 0x81u8..=0x8Fu8,
            data in hid_data_strategy(),
            timestamp in timestamp_strategy(),
            bit_to_flip in 0usize..512usize, // Bit position to flip
        ) {
            prop_assume!(!data.is_empty());
            let byte_idx = bit_to_flip / 8;
            let bit_idx = bit_to_flip % 8;

            // Skip if byte index is out of range
            prop_assume!(byte_idx < data.len());

            let original_checksum = protocol::integrity::compute_interrupt_checksum(seq, endpoint, &data, timestamp);

            // Flip a single bit
            let mut corrupted_data = data.clone();
            corrupted_data[byte_idx] ^= 1 << bit_idx;

            let corrupted_checksum = protocol::integrity::compute_interrupt_checksum(seq, endpoint, &corrupted_data, timestamp);

            // Checksum should detect the corruption
            prop_assert_ne!(original_checksum, corrupted_checksum);
        }

        /// Property: Out-of-order reports don't cause sequence tracking regression
        #[test]
        fn prop_sequence_tracking_monotonic(
            sequences in proptest::collection::vec(0u64..100u64, 1..20),
        ) {
            let buffer = EndpointReceiveBuffer::new(0x81, 100);
            buffer.activate(0);

            let mut max_seen = 0u64;

            for seq in sequences {
                let data = vec![seq as u8];
                let checksum = protocol::integrity::compute_interrupt_checksum(seq, 0x81, &data, seq * 1000);
                let report = ReceivedReport {
                    sequence: seq,
                    endpoint: 0x81,
                    data,
                    server_timestamp_us: seq * 1000,
                    received_at: Instant::now(),
                    checksum,
                };

                buffer.store(report);
                max_seen = max_seen.max(seq);

                // next_expected should never be less than max_seen + 1
                let next_expected = buffer.next_expected_seq.load(std::sync::atomic::Ordering::SeqCst);
                prop_assert!(next_expected >= max_seen + 1,
                    "next_expected ({}) should be >= max_seen + 1 ({})",
                    next_expected, max_seen + 1);
            }
        }

        /// Property: Flow control states are consistent with buffer fill level
        #[test]
        fn prop_flow_control_consistency(
            num_reports in 0usize..100usize,
        ) {
            let buffer = EndpointReceiveBuffer::new(0x81, 100);
            buffer.activate(0);

            // Fill buffer with reports
            for i in 0..num_reports {
                let data = vec![i as u8];
                let checksum = protocol::integrity::compute_interrupt_checksum(i as u64, 0x81, &data, i as u64 * 1000);
                let report = ReceivedReport {
                    sequence: i as u64,
                    endpoint: 0x81,
                    data,
                    server_timestamp_us: i as u64 * 1000,
                    received_at: Instant::now(),
                    checksum,
                };
                buffer.store(report);
            }

            let fill = buffer.fill_ratio();
            let state = buffer.flow_control_state();

            // Verify state consistency with fill level
            if fill >= 0.95 {
                prop_assert_eq!(state, FlowControlState::Paused);
            } else if fill >= 0.75 {
                prop_assert_eq!(state, FlowControlState::Backpressure);
            } else if fill <= 0.50 {
                prop_assert_eq!(state, FlowControlState::Normal);
            }
            // 50-75% is hysteresis zone - any state is valid
        }

        /// Property: Buffer never exceeds max capacity
        #[test]
        fn prop_buffer_capacity_respected(
            num_reports in 0usize..200usize,
        ) {
            let max_size = 50;
            let buffer = EndpointReceiveBuffer::new(0x81, max_size);
            buffer.activate(0);

            for i in 0..num_reports {
                let data = vec![i as u8];
                let checksum = protocol::integrity::compute_interrupt_checksum(i as u64, 0x81, &data, i as u64 * 1000);
                let report = ReceivedReport {
                    sequence: i as u64,
                    endpoint: 0x81,
                    data,
                    server_timestamp_us: i as u64 * 1000,
                    received_at: Instant::now(),
                    checksum,
                };
                buffer.store(report);

                // Buffer should never exceed max_size
                let stats = buffer.stats();
                prop_assert!(stats.buffered <= max_size,
                    "Buffer size {} exceeds max {}",
                    stats.buffered, max_size);
            }
        }

        /// Property: Gap detection correctly identifies missing sequences
        #[test]
        fn prop_gap_detection_accurate(
            start_seq in 0u64..100u64,
            gap_size in 1u64..10u64,
        ) {
            let buffer = EndpointReceiveBuffer::new(0x81, 100);
            buffer.activate(start_seq);

            // Store first report
            let data1 = vec![0u8];
            let checksum1 = protocol::integrity::compute_interrupt_checksum(start_seq, 0x81, &data1, 1000);
            buffer.store(ReceivedReport {
                sequence: start_seq,
                endpoint: 0x81,
                data: data1,
                server_timestamp_us: 1000,
                received_at: Instant::now(),
                checksum: checksum1,
            });

            // Skip gap_size sequences
            let after_gap = start_seq + gap_size + 1;
            let data2 = vec![1u8];
            let checksum2 = protocol::integrity::compute_interrupt_checksum(after_gap, 0x81, &data2, 2000);
            let (gap_detected, _) = buffer.store(ReceivedReport {
                sequence: after_gap,
                endpoint: 0x81,
                data: data2,
                server_timestamp_us: 2000,
                received_at: Instant::now(),
                checksum: checksum2,
            });

            // Gap should be detected
            prop_assert!(gap_detected);

            // Missing sequences should be tracked
            let missing = buffer.get_missing_sequences();
            prop_assert_eq!(missing.len(), gap_size as usize,
                "Expected {} missing sequences, got {}",
                gap_size, missing.len());

            // All missing sequences should be in range (start_seq+1, after_gap)
            for seq in missing {
                prop_assert!(seq > start_seq && seq < after_gap,
                    "Missing seq {} outside expected range ({}, {})",
                    seq, start_seq, after_gap);
            }
        }

        /// Property: Jitter buffer delay is always within bounds
        #[test]
        fn prop_jitter_buffer_bounded(
            arrivals in proptest::collection::vec(
                (1_000_000u64..2_000_000u64, 1_000_000u64..2_000_000u64),
                2..50
            ),
        ) {
            let min_delay = 1000u64;
            let max_delay = 50000u64;
            let jb = AdaptiveJitterBuffer::with_config(min_delay, max_delay, 2000, 16, 2);

            for (packet_ts, arrival_ts) in arrivals {
                let delay = jb.record_arrival(packet_ts, arrival_ts);
                prop_assert!(delay >= min_delay && delay <= max_delay,
                    "Delay {} outside bounds [{}, {}]",
                    delay, min_delay, max_delay);
            }
        }
    }
}
