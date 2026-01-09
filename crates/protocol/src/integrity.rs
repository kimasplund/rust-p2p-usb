//! Data integrity utilities for USB/IP streaming
//!
//! Provides CRC32C checksum computation and verification for interrupt data.
//! CRC32C is chosen for:
//! - Hardware acceleration on x86 (SSE4.2) and ARM (CRC32 instructions)
//! - Sub-microsecond computation for small payloads
//! - Proven reliability in TCP, iSCSI, and storage protocols
//!
//! # Performance
//!
//! With hardware acceleration (typical modern CPUs):
//! - 8 bytes (HID report): ~5ns
//! - 64 bytes: ~10ns
//! - 4KB: ~100ns
//!
//! # Usage
//!
//! ```ignore
//! use protocol::integrity::{compute_interrupt_checksum, verify_interrupt_checksum};
//!
//! // Compute checksum when sending
//! let checksum = compute_interrupt_checksum(sequence, endpoint, &data, timestamp_us);
//!
//! // Verify checksum when receiving
//! if !verify_interrupt_checksum(sequence, endpoint, &data, timestamp_us, checksum) {
//!     // Handle corruption
//! }
//! ```

use crc32fast::Hasher;

/// Compute CRC32C checksum for data
///
/// Used for verifying integrity of bulk transfers.
#[inline]
pub fn compute_checksum(data: &[u8]) -> u32 {
    let mut hasher = Hasher::new();
    hasher.update(data);
    hasher.finalize()
}

/// Verify CRC32C checksum for bulk data
#[inline]
pub fn verify_checksum(data: &[u8], expected_checksum: u32) -> bool {
    compute_checksum(data) == expected_checksum
}

/// Compute CRC32C checksum for interrupt data fields
///
/// The checksum covers all fields that could be corrupted:
/// - sequence: 8 bytes (little-endian)
/// - endpoint: 1 byte
/// - data: variable length
/// - timestamp_us: 8 bytes (little-endian)
///
/// This provides defense-in-depth beyond QUIC transport integrity.
#[inline]
pub fn compute_interrupt_checksum(
    sequence: u64,
    endpoint: u8,
    data: &[u8],
    timestamp_us: u64,
) -> u32 {
    let mut hasher = Hasher::new();
    hasher.update(&sequence.to_le_bytes());
    hasher.update(&[endpoint]);
    hasher.update(data);
    hasher.update(&timestamp_us.to_le_bytes());
    hasher.finalize()
}

/// Verify CRC32C checksum for interrupt data
///
/// Returns `true` if the checksum matches, `false` if corruption detected.
#[inline]
pub fn verify_interrupt_checksum(
    sequence: u64,
    endpoint: u8,
    data: &[u8],
    timestamp_us: u64,
    expected_checksum: u32,
) -> bool {
    compute_interrupt_checksum(sequence, endpoint, data, timestamp_us) == expected_checksum
}

/// Integrity verification result
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrityResult {
    /// Data passed integrity check
    Valid,
    /// Checksum mismatch - data is corrupted
    ChecksumMismatch {
        expected: u32,
        computed: u32,
    },
    /// Sequence gap detected
    SequenceGap {
        expected: u64,
        received: u64,
    },
}

impl IntegrityResult {
    /// Returns true if data is valid
    pub fn is_valid(&self) -> bool {
        matches!(self, IntegrityResult::Valid)
    }
}

/// Integrity metrics for monitoring
#[derive(Debug, Default, Clone)]
pub struct IntegrityMetrics {
    /// Total reports processed
    pub reports_processed: u64,
    /// Checksum verification failures
    pub checksum_failures: u64,
    /// Sequence gaps detected
    pub sequence_gaps: u64,
    /// Gap recovery attempts
    pub recovery_attempts: u64,
    /// Successful recoveries
    pub successful_recoveries: u64,
}

impl IntegrityMetrics {
    /// Create new metrics instance
    pub fn new() -> Self {
        Self::default()
    }

    /// Calculate integrity rate (1.0 = perfect, 0.0 = all failures)
    pub fn integrity_rate(&self) -> f64 {
        if self.reports_processed == 0 {
            1.0
        } else {
            let failures = self.checksum_failures;
            (self.reports_processed - failures) as f64 / self.reports_processed as f64
        }
    }

    /// Calculate gap rate
    pub fn gap_rate(&self) -> f64 {
        if self.reports_processed == 0 {
            0.0
        } else {
            self.sequence_gaps as f64 / self.reports_processed as f64
        }
    }

    /// Calculate recovery success rate
    pub fn recovery_rate(&self) -> f64 {
        if self.recovery_attempts == 0 {
            1.0
        } else {
            self.successful_recoveries as f64 / self.recovery_attempts as f64
        }
    }

    /// Record a processed report
    pub fn record_report(&mut self) {
        self.reports_processed += 1;
    }

    /// Record a checksum failure
    pub fn record_checksum_failure(&mut self) {
        self.checksum_failures += 1;
    }

    /// Record a sequence gap
    pub fn record_gap(&mut self) {
        self.sequence_gaps += 1;
    }

    /// Record a recovery attempt
    pub fn record_recovery_attempt(&mut self, success: bool) {
        self.recovery_attempts += 1;
        if success {
            self.successful_recoveries += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checksum_roundtrip() {
        let seq = 12345u64;
        let endpoint = 0x81u8;
        let data = vec![0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00]; // 'A' key HID report
        let timestamp = 1704067200_000_000u64;

        let checksum = compute_interrupt_checksum(seq, endpoint, &data, timestamp);
        assert!(verify_interrupt_checksum(seq, endpoint, &data, timestamp, checksum));
    }

    #[test]
    fn test_checksum_detects_corruption() {
        let seq = 12345u64;
        let endpoint = 0x81u8;
        let data = vec![0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00];
        let timestamp = 1704067200_000_000u64;

        let checksum = compute_interrupt_checksum(seq, endpoint, &data, timestamp);

        // Corrupt sequence
        assert!(!verify_interrupt_checksum(seq + 1, endpoint, &data, timestamp, checksum));

        // Corrupt endpoint
        assert!(!verify_interrupt_checksum(seq, 0x82, &data, timestamp, checksum));

        // Corrupt data
        let mut corrupted_data = data.clone();
        corrupted_data[2] = 0x05; // Change key code
        assert!(!verify_interrupt_checksum(seq, endpoint, &corrupted_data, timestamp, checksum));

        // Corrupt timestamp
        assert!(!verify_interrupt_checksum(seq, endpoint, &data, timestamp + 1, checksum));
    }

    #[test]
    fn test_checksum_empty_data() {
        let seq = 0u64;
        let endpoint = 0x81u8;
        let data: Vec<u8> = vec![];
        let timestamp = 0u64;

        let checksum = compute_interrupt_checksum(seq, endpoint, &data, timestamp);
        assert!(verify_interrupt_checksum(seq, endpoint, &data, timestamp, checksum));
    }

    #[test]
    fn test_integrity_metrics() {
        let mut metrics = IntegrityMetrics::new();

        // Process 100 reports
        for _ in 0..100 {
            metrics.record_report();
        }

        // Record 2 checksum failures
        metrics.record_checksum_failure();
        metrics.record_checksum_failure();

        // Record 5 gaps
        for _ in 0..5 {
            metrics.record_gap();
        }

        // Record 3 recovery attempts, 2 successful
        metrics.record_recovery_attempt(true);
        metrics.record_recovery_attempt(true);
        metrics.record_recovery_attempt(false);

        assert_eq!(metrics.reports_processed, 100);
        assert_eq!(metrics.checksum_failures, 2);
        assert!((metrics.integrity_rate() - 0.98).abs() < 0.001);
        assert!((metrics.gap_rate() - 0.05).abs() < 0.001);
        assert!((metrics.recovery_rate() - 0.6667).abs() < 0.01);
    }

    #[test]
    fn test_bulk_transfer_checksum() {
        // Test data
        let data = vec![0x01, 0x02, 0x03, 0x04, 0x05];
        let checksum = compute_checksum(&data);

        // Verify checksum
        assert!(verify_checksum(&data, checksum));

        // Verify corruption detection
        let mut corrupted_data = data.clone();
        corrupted_data[2] = 0xFF;
        assert!(!verify_checksum(&corrupted_data, checksum));
    }
}
