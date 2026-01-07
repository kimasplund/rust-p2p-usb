//! Rate limiter implementation using token bucket algorithm
//!
//! Provides non-blocking rate limiting for bandwidth control. Supports:
//! - Per-client bandwidth limits
//! - Per-device bandwidth limits
//! - Global server bandwidth limit
//!
//! Uses a token bucket algorithm with configurable bucket size and refill rate.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Bandwidth limit specification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BandwidthLimit {
    /// Maximum bytes per second
    pub bytes_per_second: u64,
    /// Burst capacity (bucket size) in bytes
    pub burst_bytes: u64,
}

impl BandwidthLimit {
    /// Create a new bandwidth limit
    ///
    /// # Arguments
    /// * `bytes_per_second` - Rate limit in bytes per second
    /// * `burst_bytes` - Maximum burst size (defaults to 1 second of traffic if None)
    pub fn new(bytes_per_second: u64, burst_bytes: Option<u64>) -> Self {
        let burst = burst_bytes.unwrap_or(bytes_per_second);
        Self {
            bytes_per_second,
            burst_bytes: burst,
        }
    }

    /// Create from a human-readable string like "100Mbps", "50MB/s"
    pub fn from_str(s: &str) -> Option<Self> {
        parse_bandwidth(s).map(|bps| Self::new(bps, None))
    }
}

/// Parse bandwidth string like "100Mbps", "50MB/s", "1Gbps"
fn parse_bandwidth(s: &str) -> Option<u64> {
    let s = s.trim().to_lowercase();

    // Try parsing with various suffixes
    if let Some(num) = s.strip_suffix("gbps") {
        return num.trim().parse::<u64>().ok().map(|n| n * 1_000_000_000 / 8);
    }
    if let Some(num) = s.strip_suffix("mbps") {
        return num.trim().parse::<u64>().ok().map(|n| n * 1_000_000 / 8);
    }
    if let Some(num) = s.strip_suffix("kbps") {
        return num.trim().parse::<u64>().ok().map(|n| n * 1_000 / 8);
    }
    if let Some(num) = s.strip_suffix("bps") {
        return num.trim().parse::<u64>().ok().map(|n| n / 8);
    }
    if let Some(num) = s.strip_suffix("gb/s") {
        return num.trim().parse::<u64>().ok().map(|n| n * 1_000_000_000);
    }
    if let Some(num) = s.strip_suffix("mb/s") {
        return num.trim().parse::<u64>().ok().map(|n| n * 1_000_000);
    }
    if let Some(num) = s.strip_suffix("kb/s") {
        return num.trim().parse::<u64>().ok().map(|n| n * 1_000);
    }
    if let Some(num) = s.strip_suffix("b/s") {
        return num.trim().parse::<u64>().ok();
    }

    // Plain number treated as bytes per second
    s.parse::<u64>().ok()
}

/// Token bucket state for rate limiting
#[derive(Debug, Clone)]
struct TokenBucket {
    /// Current number of tokens (bytes) available
    tokens: f64,
    /// Maximum tokens (burst capacity)
    max_tokens: f64,
    /// Tokens added per second
    refill_rate: f64,
    /// Last time tokens were refilled
    last_refill: Instant,
}

impl TokenBucket {
    fn new(limit: &BandwidthLimit) -> Self {
        Self {
            tokens: limit.burst_bytes as f64,
            max_tokens: limit.burst_bytes as f64,
            refill_rate: limit.bytes_per_second as f64,
            last_refill: Instant::now(),
        }
    }

    /// Refill tokens based on elapsed time
    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();

        if elapsed > 0.0 {
            self.tokens = (self.tokens + self.refill_rate * elapsed).min(self.max_tokens);
            self.last_refill = now;
        }
    }

    /// Try to consume tokens, returns true if successful
    fn try_consume(&mut self, bytes: u64) -> bool {
        self.refill();

        let bytes_f64 = bytes as f64;
        if self.tokens >= bytes_f64 {
            self.tokens -= bytes_f64;
            true
        } else {
            false
        }
    }

    /// Get the wait time until enough tokens are available
    fn wait_time(&mut self, bytes: u64) -> Duration {
        self.refill();

        let bytes_f64 = bytes as f64;
        if self.tokens >= bytes_f64 {
            Duration::ZERO
        } else {
            let deficit = bytes_f64 - self.tokens;
            let seconds = deficit / self.refill_rate;
            Duration::from_secs_f64(seconds)
        }
    }

    /// Record bytes transferred (for metrics, without limiting)
    fn record(&mut self, bytes: u64) {
        self.refill();
        self.tokens = (self.tokens - bytes as f64).max(0.0);
    }
}

/// Rate limiting result
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitResult {
    /// Transfer is allowed to proceed
    Allowed,
    /// Transfer should wait for the specified duration
    Wait(Duration),
}

impl RateLimitResult {
    /// Returns true if the transfer is allowed
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed)
    }

    /// Get the wait duration, if any
    pub fn wait_duration(&self) -> Option<Duration> {
        match self {
            Self::Allowed => None,
            Self::Wait(d) => Some(*d),
        }
    }
}

/// Rate limiter for bandwidth control
///
/// Manages token buckets for global, per-client, and per-device limits.
/// Thread-safe and non-blocking (returns wait times instead of blocking).
#[derive(Debug)]
pub struct RateLimiter {
    /// Global bandwidth limit (optional)
    global_bucket: Option<Mutex<TokenBucket>>,
    /// Per-client limits
    client_buckets: Mutex<HashMap<String, TokenBucket>>,
    /// Per-device limits
    device_buckets: Mutex<HashMap<u32, TokenBucket>>,
    /// Default per-client limit
    default_client_limit: Option<BandwidthLimit>,
    /// Default per-device limit
    default_device_limit: Option<BandwidthLimit>,
}

impl RateLimiter {
    /// Create a new rate limiter with the specified limits
    pub fn new(
        global_limit: Option<BandwidthLimit>,
        default_client_limit: Option<BandwidthLimit>,
        default_device_limit: Option<BandwidthLimit>,
    ) -> Self {
        Self {
            global_bucket: global_limit.map(|l| Mutex::new(TokenBucket::new(&l))),
            client_buckets: Mutex::new(HashMap::new()),
            device_buckets: Mutex::new(HashMap::new()),
            default_client_limit,
            default_device_limit,
        }
    }

    /// Check if a transfer of the given size is allowed
    ///
    /// Returns `RateLimitResult::Allowed` if the transfer can proceed,
    /// or `RateLimitResult::Wait(duration)` if the caller should wait.
    ///
    /// # Arguments
    /// * `client_id` - Optional client identifier for per-client limiting
    /// * `device_id` - Optional device ID for per-device limiting
    /// * `bytes` - Number of bytes to transfer
    pub async fn check(
        &self,
        client_id: Option<&str>,
        device_id: Option<u32>,
        bytes: u64,
    ) -> RateLimitResult {
        let mut max_wait = Duration::ZERO;

        // Check global limit
        if let Some(ref global) = self.global_bucket {
            let mut bucket = global.lock().await;
            let wait = bucket.wait_time(bytes);
            if wait > max_wait {
                max_wait = wait;
            }
        }

        // Check client limit
        if let Some(client_id) = client_id {
            if let Some(limit) = &self.default_client_limit {
                let mut buckets = self.client_buckets.lock().await;
                let bucket = buckets
                    .entry(client_id.to_string())
                    .or_insert_with(|| TokenBucket::new(limit));
                let wait = bucket.wait_time(bytes);
                if wait > max_wait {
                    max_wait = wait;
                }
            }
        }

        // Check device limit
        if let Some(device_id) = device_id {
            if let Some(limit) = &self.default_device_limit {
                let mut buckets = self.device_buckets.lock().await;
                let bucket = buckets
                    .entry(device_id)
                    .or_insert_with(|| TokenBucket::new(limit));
                let wait = bucket.wait_time(bytes);
                if wait > max_wait {
                    max_wait = wait;
                }
            }
        }

        if max_wait.is_zero() {
            RateLimitResult::Allowed
        } else {
            RateLimitResult::Wait(max_wait)
        }
    }

    /// Record bytes transferred (consumes tokens from all applicable buckets)
    ///
    /// Call this after a transfer completes to update the rate limiter state.
    pub async fn record(
        &self,
        client_id: Option<&str>,
        device_id: Option<u32>,
        bytes: u64,
    ) {
        // Record in global bucket
        if let Some(ref global) = self.global_bucket {
            let mut bucket = global.lock().await;
            bucket.record(bytes);
        }

        // Record in client bucket
        if let Some(client_id) = client_id {
            if let Some(limit) = &self.default_client_limit {
                let mut buckets = self.client_buckets.lock().await;
                let bucket = buckets
                    .entry(client_id.to_string())
                    .or_insert_with(|| TokenBucket::new(limit));
                bucket.record(bytes);
            }
        }

        // Record in device bucket
        if let Some(device_id) = device_id {
            if let Some(limit) = &self.default_device_limit {
                let mut buckets = self.device_buckets.lock().await;
                let bucket = buckets
                    .entry(device_id)
                    .or_insert_with(|| TokenBucket::new(limit));
                bucket.record(bytes);
            }
        }
    }

    /// Try to acquire permission to transfer bytes
    ///
    /// Returns true if allowed, false if rate limited.
    /// Unlike `check`, this actually consumes the tokens if allowed.
    pub async fn try_acquire(
        &self,
        client_id: Option<&str>,
        device_id: Option<u32>,
        bytes: u64,
    ) -> bool {
        // First check if allowed
        let result = self.check(client_id, device_id, bytes).await;

        if result.is_allowed() {
            // Consume tokens
            self.record(client_id, device_id, bytes).await;
            true
        } else {
            false
        }
    }

    /// Remove a client's bucket (call when client disconnects)
    pub async fn remove_client(&self, client_id: &str) {
        let mut buckets = self.client_buckets.lock().await;
        buckets.remove(client_id);
    }

    /// Remove a device's bucket (call when device detaches)
    pub async fn remove_device(&self, device_id: u32) {
        let mut buckets = self.device_buckets.lock().await;
        buckets.remove(&device_id);
    }
}

/// Shared rate limiter handle
pub type SharedRateLimiter = Arc<RateLimiter>;

/// Bandwidth usage metrics
#[derive(Debug, Clone, Default)]
pub struct BandwidthMetrics {
    /// Total bytes transferred
    pub total_bytes: u64,
    /// Bytes transferred in the last second
    pub bytes_per_second: u64,
    /// Number of rate-limited events
    pub rate_limited_count: u64,
    /// Last update timestamp
    pub last_update: Option<Instant>,
}

impl BandwidthMetrics {
    /// Update metrics with a transfer
    pub fn record(&mut self, bytes: u64) {
        let now = Instant::now();
        self.total_bytes += bytes;

        // Reset bytes per second counter every second
        if let Some(last) = self.last_update {
            if now.duration_since(last) >= Duration::from_secs(1) {
                self.bytes_per_second = bytes;
                self.last_update = Some(now);
            } else {
                self.bytes_per_second += bytes;
            }
        } else {
            self.bytes_per_second = bytes;
            self.last_update = Some(now);
        }
    }

    /// Record a rate-limited event
    pub fn record_limited(&mut self) {
        self.rate_limited_count += 1;
    }
}

/// Metrics tracker for bandwidth usage
#[derive(Debug)]
pub struct MetricsTracker {
    /// Global metrics
    global: Mutex<BandwidthMetrics>,
    /// Per-client metrics
    clients: Mutex<HashMap<String, BandwidthMetrics>>,
    /// Per-device metrics
    devices: Mutex<HashMap<u32, BandwidthMetrics>>,
}

impl MetricsTracker {
    /// Create a new metrics tracker
    pub fn new() -> Self {
        Self {
            global: Mutex::new(BandwidthMetrics::default()),
            clients: Mutex::new(HashMap::new()),
            devices: Mutex::new(HashMap::new()),
        }
    }

    /// Record a transfer
    pub async fn record(
        &self,
        client_id: Option<&str>,
        device_id: Option<u32>,
        bytes: u64,
    ) {
        // Update global metrics
        {
            let mut global = self.global.lock().await;
            global.record(bytes);
        }

        // Update client metrics
        if let Some(client_id) = client_id {
            let mut clients = self.clients.lock().await;
            clients
                .entry(client_id.to_string())
                .or_default()
                .record(bytes);
        }

        // Update device metrics
        if let Some(device_id) = device_id {
            let mut devices = self.devices.lock().await;
            devices.entry(device_id).or_default().record(bytes);
        }
    }

    /// Get global metrics
    pub async fn global_metrics(&self) -> BandwidthMetrics {
        self.global.lock().await.clone()
    }

    /// Get client metrics
    pub async fn client_metrics(&self, client_id: &str) -> Option<BandwidthMetrics> {
        self.clients.lock().await.get(client_id).cloned()
    }

    /// Get device metrics
    pub async fn device_metrics(&self, device_id: u32) -> Option<BandwidthMetrics> {
        self.devices.lock().await.get(&device_id).cloned()
    }
}

impl Default for MetricsTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_bandwidth() {
        assert_eq!(parse_bandwidth("100Mbps"), Some(12_500_000)); // 100 megabits = 12.5 MB
        assert_eq!(parse_bandwidth("1Gbps"), Some(125_000_000)); // 1 gigabit = 125 MB
        assert_eq!(parse_bandwidth("50MB/s"), Some(50_000_000));
        assert_eq!(parse_bandwidth("1GB/s"), Some(1_000_000_000));
        assert_eq!(parse_bandwidth("1000"), Some(1000));
    }

    #[test]
    fn test_bandwidth_limit() {
        let limit = BandwidthLimit::new(1_000_000, None);
        assert_eq!(limit.bytes_per_second, 1_000_000);
        assert_eq!(limit.burst_bytes, 1_000_000);

        let limit = BandwidthLimit::new(1_000_000, Some(2_000_000));
        assert_eq!(limit.burst_bytes, 2_000_000);

        let limit = BandwidthLimit::from_str("100Mbps").unwrap();
        assert_eq!(limit.bytes_per_second, 12_500_000);
    }

    #[test]
    fn test_token_bucket() {
        let limit = BandwidthLimit::new(1000, Some(1000));
        let mut bucket = TokenBucket::new(&limit);

        // Initially full
        assert!(bucket.try_consume(500));
        assert!(bucket.try_consume(500));
        assert!(!bucket.try_consume(100)); // Empty

        // Wait time should be non-zero
        let wait = bucket.wait_time(100);
        assert!(wait > Duration::ZERO);
    }

    #[tokio::test]
    async fn test_rate_limiter() {
        let global = BandwidthLimit::new(10_000, Some(10_000));
        let client = BandwidthLimit::new(5_000, Some(5_000));

        let limiter = RateLimiter::new(Some(global), Some(client), None);

        // First check should be allowed
        let result = limiter.check(Some("client1"), None, 1000).await;
        assert!(result.is_allowed());

        // Record the transfer
        limiter.record(Some("client1"), None, 1000).await;

        // Should still have capacity
        let result = limiter.check(Some("client1"), None, 1000).await;
        assert!(result.is_allowed());
    }

    #[tokio::test]
    async fn test_rate_limiter_overflow() {
        let limit = BandwidthLimit::new(1000, Some(1000));
        let limiter = RateLimiter::new(Some(limit), None, None);

        // Consume all tokens
        limiter.record(None, None, 1000).await;

        // Should need to wait
        let result = limiter.check(None, None, 100).await;
        assert!(!result.is_allowed());
        assert!(result.wait_duration().is_some());
    }

    #[tokio::test]
    async fn test_metrics_tracker() {
        let tracker = MetricsTracker::new();

        tracker.record(Some("client1"), Some(1), 1000).await;
        tracker.record(Some("client1"), Some(1), 500).await;

        let global = tracker.global_metrics().await;
        assert_eq!(global.total_bytes, 1500);

        let client = tracker.client_metrics("client1").await.unwrap();
        assert_eq!(client.total_bytes, 1500);

        let device = tracker.device_metrics(1).await.unwrap();
        assert_eq!(device.total_bytes, 1500);
    }
}
