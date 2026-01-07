//! Performance metrics tracking for USB transfers
//!
//! This module provides thread-safe metrics collection for monitoring
//! USB transfer performance, bandwidth usage, and connection quality.

use std::collections::VecDeque;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Rolling window size for calculating averages (10 seconds worth of samples)
const ROLLING_WINDOW_SIZE: usize = 100;

/// Sample interval for rolling window (100ms)
pub const SAMPLE_INTERVAL_MS: u64 = 100;

/// Calculate the rolling window duration from sample interval and window size
pub fn rolling_window_duration() -> Duration {
    Duration::from_millis(SAMPLE_INTERVAL_MS * ROLLING_WINDOW_SIZE as u64)
}

/// A single latency measurement
#[derive(Debug, Clone, Copy)]
pub struct LatencySample {
    /// Latency value in microseconds
    pub latency_us: u64,
    /// Timestamp when sample was taken
    pub timestamp: Instant,
}

/// Rolling statistics calculator for latency measurements
#[derive(Debug)]
struct RollingStats {
    samples: VecDeque<LatencySample>,
    window_duration: Duration,
}

impl RollingStats {
    fn new(window_duration: Duration) -> Self {
        Self {
            samples: VecDeque::with_capacity(ROLLING_WINDOW_SIZE),
            window_duration,
        }
    }

    fn add_sample(&mut self, latency_us: u64) {
        let now = Instant::now();
        self.samples.push_back(LatencySample {
            latency_us,
            timestamp: now,
        });
        self.prune_old_samples(now);
    }

    fn prune_old_samples(&mut self, now: Instant) {
        let cutoff = now - self.window_duration;
        while let Some(front) = self.samples.front() {
            if front.timestamp < cutoff {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    fn min(&self) -> Option<u64> {
        self.samples.iter().map(|s| s.latency_us).min()
    }

    fn max(&self) -> Option<u64> {
        self.samples.iter().map(|s| s.latency_us).max()
    }

    fn avg(&self) -> Option<u64> {
        if self.samples.is_empty() {
            return None;
        }
        let sum: u64 = self.samples.iter().map(|s| s.latency_us).sum();
        Some(sum / self.samples.len() as u64)
    }

    fn count(&self) -> usize {
        self.samples.len()
    }
}

/// Throughput sample for bandwidth calculation
#[derive(Debug, Clone, Copy)]
struct ThroughputSample {
    /// Bytes transferred
    bytes: u64,
    /// Timestamp when transfer completed
    timestamp: Instant,
}

/// Rolling throughput calculator
#[derive(Debug)]
struct RollingThroughput {
    samples: VecDeque<ThroughputSample>,
    window_duration: Duration,
}

impl RollingThroughput {
    fn new(window_duration: Duration) -> Self {
        Self {
            samples: VecDeque::with_capacity(ROLLING_WINDOW_SIZE),
            window_duration,
        }
    }

    fn add_sample(&mut self, bytes: u64) {
        let now = Instant::now();
        self.samples.push_back(ThroughputSample {
            bytes,
            timestamp: now,
        });
        self.prune_old_samples(now);
    }

    fn prune_old_samples(&mut self, now: Instant) {
        let cutoff = now - self.window_duration;
        while let Some(front) = self.samples.front() {
            if front.timestamp < cutoff {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    /// Calculate bytes per second over the rolling window
    fn bytes_per_second(&self) -> f64 {
        if self.samples.len() < 2 {
            return 0.0;
        }

        let total_bytes: u64 = self.samples.iter().map(|s| s.bytes).sum();
        let first = self.samples.front().map(|s| s.timestamp);
        let last = self.samples.back().map(|s| s.timestamp);

        match (first, last) {
            (Some(first), Some(last)) => {
                let duration = last.duration_since(first);
                if duration.as_secs_f64() > 0.0 {
                    total_bytes as f64 / duration.as_secs_f64()
                } else {
                    0.0
                }
            }
            _ => 0.0,
        }
    }
}

/// Latency statistics snapshot
#[derive(Debug, Clone, Copy, Default)]
pub struct LatencyStats {
    /// Minimum latency in microseconds
    pub min_us: u64,
    /// Maximum latency in microseconds
    pub max_us: u64,
    /// Average latency in microseconds
    pub avg_us: u64,
    /// Number of samples in the window
    pub sample_count: usize,
}

impl LatencyStats {
    /// Format latency for display (converts to ms)
    pub fn format_min(&self) -> String {
        format!("{:.2} ms", self.min_us as f64 / 1000.0)
    }

    pub fn format_max(&self) -> String {
        format!("{:.2} ms", self.max_us as f64 / 1000.0)
    }

    pub fn format_avg(&self) -> String {
        format!("{:.2} ms", self.avg_us as f64 / 1000.0)
    }
}

/// Transfer metrics for a single device or connection
#[derive(Debug)]
pub struct TransferMetrics {
    /// Total bytes sent
    bytes_sent: AtomicU64,
    /// Total bytes received
    bytes_received: AtomicU64,
    /// Total transfers completed successfully
    transfers_completed: AtomicU64,
    /// Total transfers failed
    transfers_failed: AtomicU64,
    /// Total retries
    retries: AtomicU64,
    /// Active transfer count
    active_transfers: AtomicU64,
    /// Rolling latency statistics (protected by RwLock for mutable access)
    latency_stats: RwLock<RollingStats>,
    /// Rolling throughput for sent data
    throughput_tx: RwLock<RollingThroughput>,
    /// Rolling throughput for received data
    throughput_rx: RwLock<RollingThroughput>,
    /// Connection start time
    connected_at: RwLock<Option<Instant>>,
}

impl Default for TransferMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl TransferMetrics {
    /// Create new transfer metrics
    pub fn new() -> Self {
        let window = rolling_window_duration();
        Self {
            bytes_sent: AtomicU64::new(0),
            bytes_received: AtomicU64::new(0),
            transfers_completed: AtomicU64::new(0),
            transfers_failed: AtomicU64::new(0),
            retries: AtomicU64::new(0),
            active_transfers: AtomicU64::new(0),
            latency_stats: RwLock::new(RollingStats::new(window)),
            throughput_tx: RwLock::new(RollingThroughput::new(window)),
            throughput_rx: RwLock::new(RollingThroughput::new(window)),
            connected_at: RwLock::new(None),
        }
    }

    /// Mark connection as started
    pub fn mark_connected(&self) {
        let mut connected_at = self.connected_at.write().unwrap();
        *connected_at = Some(Instant::now());
    }

    /// Mark connection as ended
    pub fn mark_disconnected(&self) {
        let mut connected_at = self.connected_at.write().unwrap();
        *connected_at = None;
    }

    /// Get connection uptime
    pub fn uptime(&self) -> Option<Duration> {
        let connected_at = self.connected_at.read().unwrap();
        connected_at.map(|t| t.elapsed())
    }

    /// Record a transfer start
    pub fn transfer_started(&self) {
        self.active_transfers.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a transfer completion
    pub fn transfer_completed(&self, bytes_sent: u64, bytes_received: u64, latency: Duration) {
        self.active_transfers.fetch_sub(1, Ordering::Relaxed);
        self.transfers_completed.fetch_add(1, Ordering::Relaxed);
        self.bytes_sent.fetch_add(bytes_sent, Ordering::Relaxed);
        self.bytes_received
            .fetch_add(bytes_received, Ordering::Relaxed);

        // Record latency sample
        let latency_us = latency.as_micros() as u64;
        if let Ok(mut stats) = self.latency_stats.write() {
            stats.add_sample(latency_us);
        }

        // Record throughput samples
        if bytes_sent > 0 {
            if let Ok(mut tx) = self.throughput_tx.write() {
                tx.add_sample(bytes_sent);
            }
        }
        if bytes_received > 0 {
            if let Ok(mut rx) = self.throughput_rx.write() {
                rx.add_sample(bytes_received);
            }
        }
    }

    /// Record a transfer failure
    pub fn transfer_failed(&self) {
        self.active_transfers.fetch_sub(1, Ordering::Relaxed);
        self.transfers_failed.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a transfer with success/failure indication
    ///
    /// Convenience method that calls either transfer_completed or transfer_failed
    /// based on the success parameter.
    pub fn record_transfer(
        &self,
        bytes_sent: u64,
        bytes_received: u64,
        latency: Duration,
        success: bool,
    ) {
        self.transfer_started();
        if success {
            self.transfer_completed(bytes_sent, bytes_received, latency);
        } else {
            self.transfer_failed();
        }
    }

    /// Record a retry
    pub fn record_retry(&self) {
        self.retries.fetch_add(1, Ordering::Relaxed);
    }

    /// Get total bytes sent
    pub fn total_bytes_sent(&self) -> u64 {
        self.bytes_sent.load(Ordering::Relaxed)
    }

    /// Get total bytes received
    pub fn total_bytes_received(&self) -> u64 {
        self.bytes_received.load(Ordering::Relaxed)
    }

    /// Get total transfers completed
    pub fn transfers_completed(&self) -> u64 {
        self.transfers_completed.load(Ordering::Relaxed)
    }

    /// Get total transfers failed
    pub fn transfers_failed(&self) -> u64 {
        self.transfers_failed.load(Ordering::Relaxed)
    }

    /// Get total retries
    pub fn total_retries(&self) -> u64 {
        self.retries.load(Ordering::Relaxed)
    }

    /// Get active transfer count
    pub fn active_transfers(&self) -> u64 {
        self.active_transfers.load(Ordering::Relaxed)
    }

    /// Get latency statistics
    pub fn latency_stats(&self) -> LatencyStats {
        let stats = self.latency_stats.read().unwrap();
        LatencyStats {
            min_us: stats.min().unwrap_or(0),
            max_us: stats.max().unwrap_or(0),
            avg_us: stats.avg().unwrap_or(0),
            sample_count: stats.count(),
        }
    }

    /// Get send throughput in bytes per second
    pub fn throughput_tx_bps(&self) -> f64 {
        let tx = self.throughput_tx.read().unwrap();
        tx.bytes_per_second()
    }

    /// Get receive throughput in bytes per second
    pub fn throughput_rx_bps(&self) -> f64 {
        let rx = self.throughput_rx.read().unwrap();
        rx.bytes_per_second()
    }

    /// Calculate packet loss rate (failed / total)
    pub fn loss_rate(&self) -> f64 {
        let completed = self.transfers_completed.load(Ordering::Relaxed);
        let failed = self.transfers_failed.load(Ordering::Relaxed);
        let total = completed + failed;
        if total == 0 {
            0.0
        } else {
            failed as f64 / total as f64
        }
    }

    /// Calculate retry rate (retries / completed)
    pub fn retry_rate(&self) -> f64 {
        let completed = self.transfers_completed.load(Ordering::Relaxed);
        let retries = self.retries.load(Ordering::Relaxed);
        if completed == 0 {
            0.0
        } else {
            retries as f64 / completed as f64
        }
    }

    /// Reset all metrics
    pub fn reset(&self) {
        self.bytes_sent.store(0, Ordering::Relaxed);
        self.bytes_received.store(0, Ordering::Relaxed);
        self.transfers_completed.store(0, Ordering::Relaxed);
        self.transfers_failed.store(0, Ordering::Relaxed);
        self.retries.store(0, Ordering::Relaxed);

        let window = rolling_window_duration();
        if let Ok(mut stats) = self.latency_stats.write() {
            *stats = RollingStats::new(window);
        }
        if let Ok(mut tx) = self.throughput_tx.write() {
            *tx = RollingThroughput::new(window);
        }
        if let Ok(mut rx) = self.throughput_rx.write() {
            *rx = RollingThroughput::new(window);
        }
    }
}

/// Snapshot of metrics for display or serialization
#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    /// Total bytes sent
    pub bytes_sent: u64,
    /// Total bytes received
    pub bytes_received: u64,
    /// Transfers completed
    pub transfers_completed: u64,
    /// Transfers failed
    pub transfers_failed: u64,
    /// Total retries
    pub retries: u64,
    /// Active transfers
    pub active_transfers: u64,
    /// Latency statistics
    pub latency: LatencyStats,
    /// Send throughput (bytes/sec)
    pub throughput_tx_bps: f64,
    /// Receive throughput (bytes/sec)
    pub throughput_rx_bps: f64,
    /// Loss rate (0.0 - 1.0)
    pub loss_rate: f64,
    /// Retry rate
    pub retry_rate: f64,
    /// Connection uptime
    pub uptime: Option<Duration>,
}

impl MetricsSnapshot {
    /// Create a snapshot from TransferMetrics
    pub fn from_metrics(metrics: &TransferMetrics) -> Self {
        Self {
            bytes_sent: metrics.total_bytes_sent(),
            bytes_received: metrics.total_bytes_received(),
            transfers_completed: metrics.transfers_completed(),
            transfers_failed: metrics.transfers_failed(),
            retries: metrics.total_retries(),
            active_transfers: metrics.active_transfers(),
            latency: metrics.latency_stats(),
            throughput_tx_bps: metrics.throughput_tx_bps(),
            throughput_rx_bps: metrics.throughput_rx_bps(),
            loss_rate: metrics.loss_rate(),
            retry_rate: metrics.retry_rate(),
            uptime: metrics.uptime(),
        }
    }

    /// Format throughput for display
    pub fn format_throughput_tx(&self) -> String {
        format_bytes_per_second(self.throughput_tx_bps)
    }

    pub fn format_throughput_rx(&self) -> String {
        format_bytes_per_second(self.throughput_rx_bps)
    }

    /// Format total bytes for display
    pub fn format_bytes_sent(&self) -> String {
        format_bytes(self.bytes_sent)
    }

    pub fn format_bytes_received(&self) -> String {
        format_bytes(self.bytes_received)
    }

    /// Format loss rate as percentage
    pub fn format_loss_rate(&self) -> String {
        format!("{:.1}%", self.loss_rate * 100.0)
    }

    /// Format uptime for display
    pub fn format_uptime(&self) -> String {
        match self.uptime {
            Some(d) => format_duration(d),
            None => "N/A".to_string(),
        }
    }

    /// Get connection quality indicator (0-100)
    pub fn connection_quality(&self) -> u8 {
        // Start at 100, subtract based on issues
        let mut quality: f64 = 100.0;

        // Penalize high latency (target: <20ms)
        let avg_latency_ms = self.latency.avg_us as f64 / 1000.0;
        if avg_latency_ms > 100.0 {
            quality -= 30.0;
        } else if avg_latency_ms > 50.0 {
            quality -= 20.0;
        } else if avg_latency_ms > 20.0 {
            quality -= 10.0;
        }

        // Penalize packet loss
        if self.loss_rate > 0.1 {
            quality -= 30.0;
        } else if self.loss_rate > 0.05 {
            quality -= 20.0;
        } else if self.loss_rate > 0.01 {
            quality -= 10.0;
        }

        // Penalize high retry rate
        if self.retry_rate > 0.3 {
            quality -= 20.0;
        } else if self.retry_rate > 0.1 {
            quality -= 10.0;
        }

        quality.max(0.0) as u8
    }

    /// Get connection quality label
    pub fn connection_quality_label(&self) -> &'static str {
        match self.connection_quality() {
            90..=100 => "Excellent",
            70..=89 => "Good",
            50..=69 => "Fair",
            30..=49 => "Poor",
            _ => "Critical",
        }
    }
}

/// Format bytes as human-readable string
pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Format bytes per second as human-readable string
pub fn format_bytes_per_second(bps: f64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    if bps >= GB {
        format!("{:.2} GB/s", bps / GB)
    } else if bps >= MB {
        format!("{:.2} MB/s", bps / MB)
    } else if bps >= KB {
        format!("{:.2} KB/s", bps / KB)
    } else {
        format!("{:.0} B/s", bps)
    }
}

/// Format duration for display
pub fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    let secs = secs % 60;

    if hours > 0 {
        format!("{}h {}m {}s", hours, mins, secs)
    } else if mins > 0 {
        format!("{}m {}s", mins, secs)
    } else {
        format!("{}s", secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transfer_metrics_basic() {
        let metrics = TransferMetrics::new();

        metrics.transfer_started();
        assert_eq!(metrics.active_transfers(), 1);

        metrics.transfer_completed(100, 200, Duration::from_millis(5));
        assert_eq!(metrics.active_transfers(), 0);
        assert_eq!(metrics.total_bytes_sent(), 100);
        assert_eq!(metrics.total_bytes_received(), 200);
        assert_eq!(metrics.transfers_completed(), 1);
    }

    #[test]
    fn test_latency_stats() {
        let metrics = TransferMetrics::new();

        // Add some samples
        metrics.transfer_completed(0, 0, Duration::from_millis(5));
        metrics.transfer_completed(0, 0, Duration::from_millis(10));
        metrics.transfer_completed(0, 0, Duration::from_millis(15));

        let stats = metrics.latency_stats();
        assert_eq!(stats.min_us, 5000);
        assert_eq!(stats.max_us, 15000);
        assert_eq!(stats.avg_us, 10000);
        assert_eq!(stats.sample_count, 3);
    }

    #[test]
    fn test_loss_rate() {
        let metrics = TransferMetrics::new();

        // 9 successful, 1 failed = 10% loss
        for _ in 0..9 {
            metrics.transfer_started();
            metrics.transfer_completed(0, 0, Duration::from_millis(1));
        }
        metrics.transfer_started();
        metrics.transfer_failed();

        let loss = metrics.loss_rate();
        assert!((loss - 0.1).abs() < 0.001);
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1024 * 1024), "1.00 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.00 GB");
    }

    #[test]
    fn test_connection_quality() {
        let snapshot = MetricsSnapshot {
            bytes_sent: 0,
            bytes_received: 0,
            transfers_completed: 100,
            transfers_failed: 0,
            retries: 0,
            active_transfers: 0,
            latency: LatencyStats {
                min_us: 5000,
                max_us: 15000,
                avg_us: 10000,
                sample_count: 100,
            },
            throughput_tx_bps: 1000.0,
            throughput_rx_bps: 1000.0,
            loss_rate: 0.0,
            retry_rate: 0.0,
            uptime: Some(Duration::from_secs(60)),
        };

        assert!(snapshot.connection_quality() >= 90);
        assert_eq!(snapshot.connection_quality_label(), "Excellent");
    }

    #[test]
    fn test_metrics_reset() {
        let metrics = TransferMetrics::new();

        metrics.transfer_started();
        metrics.transfer_completed(100, 200, Duration::from_millis(5));

        assert_eq!(metrics.transfers_completed(), 1);

        metrics.reset();

        assert_eq!(metrics.transfers_completed(), 0);
        assert_eq!(metrics.total_bytes_sent(), 0);
    }
}
