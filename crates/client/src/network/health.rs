//! Connection health monitoring
//!
//! Provides health monitoring for P2P connections including:
//! - RTT (Round-Trip Time) measurement via heartbeat
//! - Connection quality assessment (Good/Fair/Poor)
//! - Connection state machine with degraded state detection
//! - Packet loss tracking

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Number of RTT samples to keep for averaging
const RTT_SAMPLE_COUNT: usize = 10;

/// Heartbeat interval (5 seconds as specified)
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

/// Heartbeat timeout before considering connection unhealthy (15 seconds as specified)
pub const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(15);

/// RTT threshold for "Good" quality (under 50ms)
const RTT_GOOD_THRESHOLD_MS: u64 = 50;

/// RTT threshold for "Fair" quality (under 150ms, above is "Poor")
const RTT_FAIR_THRESHOLD_MS: u64 = 150;

/// Packet loss threshold for degraded state (> 20%)
const PACKET_LOSS_DEGRADED_THRESHOLD: f64 = 0.20;

/// Connection quality levels
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionQuality {
    /// Excellent connection (RTT < 50ms, no packet loss)
    Good,
    /// Acceptable connection (RTT 50-150ms or minor packet loss)
    Fair,
    /// Poor connection (RTT > 150ms or significant packet loss)
    Poor,
    /// Unknown quality (no measurements yet)
    Unknown,
}

impl std::fmt::Display for ConnectionQuality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectionQuality::Good => write!(f, "Good"),
            ConnectionQuality::Fair => write!(f, "Fair"),
            ConnectionQuality::Poor => write!(f, "Poor"),
            ConnectionQuality::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Connection health state machine
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthState {
    /// Initial state, waiting for first heartbeat
    Connecting,
    /// Connection is healthy
    Connected,
    /// Connection quality has degraded but still alive
    Degraded,
    /// Connection lost (heartbeat timeout)
    Disconnected,
}

impl std::fmt::Display for HealthState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HealthState::Connecting => write!(f, "Connecting"),
            HealthState::Connected => write!(f, "Connected"),
            HealthState::Degraded => write!(f, "Degraded"),
            HealthState::Disconnected => write!(f, "Disconnected"),
        }
    }
}

/// Health metrics snapshot
#[derive(Debug, Clone)]
pub struct HealthMetrics {
    /// Current health state
    pub state: HealthState,
    /// Current connection quality
    pub quality: ConnectionQuality,
    /// Latest RTT in milliseconds (None if no measurements)
    pub latest_rtt_ms: Option<u64>,
    /// Average RTT in milliseconds (None if no measurements)
    pub average_rtt_ms: Option<u64>,
    /// Minimum RTT in milliseconds (None if no measurements)
    pub min_rtt_ms: Option<u64>,
    /// Maximum RTT in milliseconds (None if no measurements)
    pub max_rtt_ms: Option<u64>,
    /// Packet loss percentage (0.0 - 1.0)
    pub packet_loss: f64,
    /// Number of successful heartbeats
    pub heartbeats_sent: u64,
    /// Number of heartbeats acknowledged
    pub heartbeats_received: u64,
    /// Time since last successful heartbeat
    pub time_since_last_heartbeat: Option<Duration>,
    /// Number of consecutive failures
    pub consecutive_failures: u32,
}

impl Default for HealthMetrics {
    fn default() -> Self {
        Self {
            state: HealthState::Connecting,
            quality: ConnectionQuality::Unknown,
            latest_rtt_ms: None,
            average_rtt_ms: None,
            min_rtt_ms: None,
            max_rtt_ms: None,
            packet_loss: 0.0,
            heartbeats_sent: 0,
            heartbeats_received: 0,
            time_since_last_heartbeat: None,
            consecutive_failures: 0,
        }
    }
}

/// Connection health monitor
///
/// Tracks heartbeat RTT, packet loss, and connection quality.
/// Thread-safe for use across async tasks.
pub struct HealthMonitor {
    /// RTT samples (ring buffer)
    rtt_samples: RwLock<VecDeque<u64>>,
    /// Heartbeat sequence counter
    sequence: AtomicU64,
    /// Total heartbeats sent
    heartbeats_sent: AtomicU64,
    /// Total heartbeats acknowledged
    heartbeats_received: AtomicU64,
    /// Current health state
    state: RwLock<HealthState>,
    /// Last successful heartbeat time
    last_heartbeat: RwLock<Option<Instant>>,
    /// Consecutive failures count
    consecutive_failures: RwLock<u32>,
    /// Pending heartbeats (sequence -> send time)
    pending_heartbeats: RwLock<std::collections::HashMap<u64, Instant>>,
}

impl HealthMonitor {
    /// Create a new health monitor
    pub fn new() -> Self {
        Self {
            rtt_samples: RwLock::new(VecDeque::with_capacity(RTT_SAMPLE_COUNT)),
            sequence: AtomicU64::new(0),
            heartbeats_sent: AtomicU64::new(0),
            heartbeats_received: AtomicU64::new(0),
            state: RwLock::new(HealthState::Connecting),
            last_heartbeat: RwLock::new(None),
            consecutive_failures: RwLock::new(0),
            pending_heartbeats: RwLock::new(std::collections::HashMap::new()),
        }
    }

    /// Get current timestamp in milliseconds since epoch
    pub fn current_timestamp_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// Generate next heartbeat sequence and timestamp
    ///
    /// Returns (sequence, timestamp_ms) for use in Heartbeat message
    pub async fn prepare_heartbeat(&self) -> (u64, u64) {
        let seq = self.sequence.fetch_add(1, Ordering::SeqCst);
        let timestamp = Self::current_timestamp_ms();

        // Track pending heartbeat
        {
            let mut pending = self.pending_heartbeats.write().await;
            pending.insert(seq, Instant::now());
        }

        self.heartbeats_sent.fetch_add(1, Ordering::SeqCst);
        debug!("Prepared heartbeat: seq={}, timestamp={}", seq, timestamp);

        (seq, timestamp)
    }

    /// Process heartbeat acknowledgment
    ///
    /// Updates RTT samples and connection quality based on response.
    /// Returns the measured RTT in milliseconds if successful.
    pub async fn process_heartbeat_ack(
        &self,
        sequence: u64,
        client_timestamp_ms: u64,
        _server_timestamp_ms: u64,
    ) -> Option<u64> {
        // Calculate RTT from pending heartbeat
        let send_time = {
            let mut pending = self.pending_heartbeats.write().await;
            pending.remove(&sequence)
        };

        let rtt_ms = if let Some(send_time) = send_time {
            send_time.elapsed().as_millis() as u64
        } else {
            // Fallback to timestamp-based calculation
            let now = Self::current_timestamp_ms();
            now.saturating_sub(client_timestamp_ms)
        };

        debug!("Heartbeat ack received: seq={}, rtt={}ms", sequence, rtt_ms);

        // Update RTT samples
        {
            let mut samples = self.rtt_samples.write().await;
            if samples.len() >= RTT_SAMPLE_COUNT {
                samples.pop_front();
            }
            samples.push_back(rtt_ms);
        }

        // Update last heartbeat time
        {
            let mut last = self.last_heartbeat.write().await;
            *last = Some(Instant::now());
        }

        // Reset consecutive failures
        {
            let mut failures = self.consecutive_failures.write().await;
            *failures = 0;
        }

        self.heartbeats_received.fetch_add(1, Ordering::SeqCst);

        // Update state based on quality
        self.update_state().await;

        Some(rtt_ms)
    }

    /// Record a heartbeat failure (timeout or error)
    pub async fn record_failure(&self) {
        let failures = {
            let mut failures = self.consecutive_failures.write().await;
            *failures += 1;
            *failures
        };

        warn!(
            "Heartbeat failure recorded: consecutive_failures={}",
            failures
        );

        // Clean up old pending heartbeats
        {
            let mut pending = self.pending_heartbeats.write().await;
            let cutoff = Instant::now() - HEARTBEAT_TIMEOUT;
            pending.retain(|_, time| *time > cutoff);
        }

        // Update state
        self.update_state().await;
    }

    /// Update health state based on current metrics
    async fn update_state(&self) {
        let metrics = self.get_metrics().await;
        let new_state = self.calculate_state(&metrics);

        let mut state = self.state.write().await;
        if *state != new_state {
            info!("Health state transition: {} -> {}", *state, new_state);
            *state = new_state;
        }
    }

    /// Calculate health state from metrics
    fn calculate_state(&self, metrics: &HealthMetrics) -> HealthState {
        // Check for timeout
        if let Some(time_since) = metrics.time_since_last_heartbeat {
            if time_since > HEARTBEAT_TIMEOUT {
                return HealthState::Disconnected;
            }
        }

        // Check consecutive failures
        if metrics.consecutive_failures >= 3 {
            return HealthState::Disconnected;
        }

        // Check for no heartbeats yet
        if metrics.heartbeats_received == 0 {
            return HealthState::Connecting;
        }

        // Check for degraded state
        if metrics.quality == ConnectionQuality::Poor
            || metrics.packet_loss > PACKET_LOSS_DEGRADED_THRESHOLD
            || metrics.consecutive_failures >= 1
        {
            return HealthState::Degraded;
        }

        HealthState::Connected
    }

    /// Calculate connection quality from RTT and packet loss
    fn calculate_quality(&self, avg_rtt_ms: Option<u64>, packet_loss: f64) -> ConnectionQuality {
        if packet_loss > PACKET_LOSS_DEGRADED_THRESHOLD {
            return ConnectionQuality::Poor;
        }

        match avg_rtt_ms {
            None => ConnectionQuality::Unknown,
            Some(rtt) if rtt < RTT_GOOD_THRESHOLD_MS => ConnectionQuality::Good,
            Some(rtt) if rtt < RTT_FAIR_THRESHOLD_MS => ConnectionQuality::Fair,
            Some(_) => ConnectionQuality::Poor,
        }
    }

    /// Get current health metrics snapshot
    pub async fn get_metrics(&self) -> HealthMetrics {
        let samples = self.rtt_samples.read().await;
        let state = *self.state.read().await;
        let last_heartbeat = *self.last_heartbeat.read().await;
        let consecutive_failures = *self.consecutive_failures.read().await;

        let heartbeats_sent = self.heartbeats_sent.load(Ordering::SeqCst);
        let heartbeats_received = self.heartbeats_received.load(Ordering::SeqCst);

        // Calculate RTT statistics
        let (latest_rtt_ms, average_rtt_ms, min_rtt_ms, max_rtt_ms) = if samples.is_empty() {
            (None, None, None, None)
        } else {
            let latest = samples.back().copied();
            let sum: u64 = samples.iter().sum();
            let avg = sum / samples.len() as u64;
            let min = samples.iter().min().copied();
            let max = samples.iter().max().copied();
            (latest, Some(avg), min, max)
        };

        // Calculate packet loss
        let packet_loss = if heartbeats_sent > 0 {
            1.0 - (heartbeats_received as f64 / heartbeats_sent as f64)
        } else {
            0.0
        };

        // Calculate time since last heartbeat
        let time_since_last_heartbeat = last_heartbeat.map(|t| t.elapsed());

        // Calculate quality
        let quality = self.calculate_quality(average_rtt_ms, packet_loss);

        HealthMetrics {
            state,
            quality,
            latest_rtt_ms,
            average_rtt_ms,
            min_rtt_ms,
            max_rtt_ms,
            packet_loss,
            heartbeats_sent,
            heartbeats_received,
            time_since_last_heartbeat,
            consecutive_failures,
        }
    }

    /// Check if connection is healthy (not disconnected)
    pub async fn is_healthy(&self) -> bool {
        let state = *self.state.read().await;
        !matches!(state, HealthState::Disconnected)
    }

    /// Check if heartbeat timeout has occurred
    pub async fn is_timed_out(&self) -> bool {
        let last = self.last_heartbeat.read().await;
        match *last {
            Some(time) => time.elapsed() > HEARTBEAT_TIMEOUT,
            None => {
                // If no heartbeat received yet, check based on consecutive failures
                *self.consecutive_failures.read().await >= 3
            }
        }
    }

    /// Reset monitor state (for reconnection)
    pub async fn reset(&self) {
        {
            let mut samples = self.rtt_samples.write().await;
            samples.clear();
        }
        {
            let mut state = self.state.write().await;
            *state = HealthState::Connecting;
        }
        {
            let mut last = self.last_heartbeat.write().await;
            *last = None;
        }
        {
            let mut failures = self.consecutive_failures.write().await;
            *failures = 0;
        }
        {
            let mut pending = self.pending_heartbeats.write().await;
            pending.clear();
        }
        debug!("Health monitor reset");
    }
}

impl Default for HealthMonitor {
    fn default() -> Self {
        Self::new()
    }
}

/// Create a shared health monitor
pub fn create_health_monitor() -> Arc<HealthMonitor> {
    Arc::new(HealthMonitor::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_health_monitor_creation() {
        let monitor = HealthMonitor::new();
        let metrics = monitor.get_metrics().await;

        assert_eq!(metrics.state, HealthState::Connecting);
        assert_eq!(metrics.quality, ConnectionQuality::Unknown);
        assert!(metrics.latest_rtt_ms.is_none());
        assert_eq!(metrics.heartbeats_sent, 0);
    }

    #[tokio::test]
    async fn test_heartbeat_sequence() {
        let monitor = HealthMonitor::new();

        let (seq1, _) = monitor.prepare_heartbeat().await;
        let (seq2, _) = monitor.prepare_heartbeat().await;

        assert_eq!(seq1, 0);
        assert_eq!(seq2, 1);
    }

    #[tokio::test]
    async fn test_rtt_measurement() {
        let monitor = HealthMonitor::new();

        // Simulate heartbeat with known RTT
        let (seq, timestamp) = monitor.prepare_heartbeat().await;

        // Process ack
        let rtt = monitor
            .process_heartbeat_ack(seq, timestamp, timestamp + 5)
            .await;

        assert!(rtt.is_some());

        let metrics = monitor.get_metrics().await;
        assert!(metrics.latest_rtt_ms.is_some());
        assert_eq!(metrics.heartbeats_received, 1);
    }

    #[tokio::test]
    async fn test_quality_calculation() {
        let monitor = HealthMonitor::new();

        // Good quality (RTT < 50ms)
        let quality = monitor.calculate_quality(Some(30), 0.0);
        assert_eq!(quality, ConnectionQuality::Good);

        // Fair quality (RTT 50-150ms)
        let quality = monitor.calculate_quality(Some(100), 0.0);
        assert_eq!(quality, ConnectionQuality::Fair);

        // Poor quality (RTT > 150ms)
        let quality = monitor.calculate_quality(Some(200), 0.0);
        assert_eq!(quality, ConnectionQuality::Poor);

        // Poor quality (high packet loss)
        let quality = monitor.calculate_quality(Some(30), 0.25);
        assert_eq!(quality, ConnectionQuality::Poor);
    }

    #[tokio::test]
    async fn test_failure_tracking() {
        let monitor = HealthMonitor::new();

        monitor.record_failure().await;
        let metrics = monitor.get_metrics().await;
        assert_eq!(metrics.consecutive_failures, 1);

        monitor.record_failure().await;
        monitor.record_failure().await;
        let metrics = monitor.get_metrics().await;
        assert_eq!(metrics.consecutive_failures, 3);
        assert_eq!(metrics.state, HealthState::Disconnected);
    }

    #[tokio::test]
    async fn test_state_transitions() {
        let monitor = HealthMonitor::new();

        // Initial state
        let metrics = monitor.get_metrics().await;
        assert_eq!(metrics.state, HealthState::Connecting);

        // After successful heartbeat
        let (seq, timestamp) = monitor.prepare_heartbeat().await;
        monitor
            .process_heartbeat_ack(seq, timestamp, timestamp)
            .await;

        let metrics = monitor.get_metrics().await;
        assert_eq!(metrics.state, HealthState::Connected);
    }

    #[tokio::test]
    async fn test_reset() {
        let monitor = HealthMonitor::new();

        // Add some data
        let (seq, timestamp) = monitor.prepare_heartbeat().await;
        monitor
            .process_heartbeat_ack(seq, timestamp, timestamp)
            .await;
        monitor.record_failure().await;

        // Reset
        monitor.reset().await;

        let metrics = monitor.get_metrics().await;
        assert_eq!(metrics.state, HealthState::Connecting);
        assert!(metrics.latest_rtt_ms.is_none());
        assert_eq!(metrics.consecutive_failures, 0);
    }
}
