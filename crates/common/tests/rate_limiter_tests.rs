//! Integration tests for rate limiter functionality
//!
//! Tests the token bucket rate limiter implementation including:
//! - try_consume and rollback behavior
//! - Bandwidth limiting under load
//! - Atomic operations

use common::rate_limiter::{
    BandwidthLimit, BandwidthMetrics, MetricsTracker, RateLimitResult, RateLimiter,
};
use std::time::Duration;

mod bandwidth_limit {
    use super::*;

    #[test]
    fn test_bandwidth_limit_new() {
        let limit = BandwidthLimit::new(1_000_000, None);
        assert_eq!(limit.bytes_per_second, 1_000_000);
        assert_eq!(limit.burst_bytes, 1_000_000);
    }

    #[test]
    fn test_bandwidth_limit_with_burst() {
        let limit = BandwidthLimit::new(1_000_000, Some(2_000_000));
        assert_eq!(limit.bytes_per_second, 1_000_000);
        assert_eq!(limit.burst_bytes, 2_000_000);
    }

    #[test]
    fn test_bandwidth_limit_from_str_mbps() {
        let limit = BandwidthLimit::from_str("100Mbps").unwrap();
        assert_eq!(limit.bytes_per_second, 12_500_000);
    }

    #[test]
    fn test_bandwidth_limit_from_str_gbps() {
        let limit = BandwidthLimit::from_str("1Gbps").unwrap();
        assert_eq!(limit.bytes_per_second, 125_000_000);
    }

    #[test]
    fn test_bandwidth_limit_from_str_kbps() {
        let limit = BandwidthLimit::from_str("1000kbps").unwrap();
        assert_eq!(limit.bytes_per_second, 125_000);
    }

    #[test]
    fn test_bandwidth_limit_from_str_mb_per_s() {
        let limit = BandwidthLimit::from_str("50MB/s").unwrap();
        assert_eq!(limit.bytes_per_second, 50_000_000);
    }

    #[test]
    fn test_bandwidth_limit_from_str_gb_per_s() {
        let limit = BandwidthLimit::from_str("1GB/s").unwrap();
        assert_eq!(limit.bytes_per_second, 1_000_000_000);
    }

    #[test]
    fn test_bandwidth_limit_from_str_kb_per_s() {
        let limit = BandwidthLimit::from_str("100KB/s").unwrap();
        assert_eq!(limit.bytes_per_second, 100_000);
    }

    #[test]
    fn test_bandwidth_limit_from_str_bytes_per_s() {
        let limit = BandwidthLimit::from_str("1000B/s").unwrap();
        assert_eq!(limit.bytes_per_second, 1000);
    }

    #[test]
    fn test_bandwidth_limit_from_str_plain_number() {
        let limit = BandwidthLimit::from_str("50000").unwrap();
        assert_eq!(limit.bytes_per_second, 50000);
    }

    #[test]
    fn test_bandwidth_limit_from_str_case_insensitive() {
        let limit1 = BandwidthLimit::from_str("100mbps").unwrap();
        let limit2 = BandwidthLimit::from_str("100MBPS").unwrap();
        let limit3 = BandwidthLimit::from_str("100Mbps").unwrap();

        assert_eq!(limit1.bytes_per_second, limit2.bytes_per_second);
        assert_eq!(limit2.bytes_per_second, limit3.bytes_per_second);
    }

    #[test]
    fn test_bandwidth_limit_from_str_with_whitespace() {
        let limit = BandwidthLimit::from_str("  100Mbps  ").unwrap();
        assert_eq!(limit.bytes_per_second, 12_500_000);
    }

    #[test]
    fn test_bandwidth_limit_from_str_invalid() {
        assert!(BandwidthLimit::from_str("invalid").is_none());
        assert!(BandwidthLimit::from_str("").is_none());
    }
}

mod rate_limit_result {
    use super::*;

    #[test]
    fn test_allowed_is_allowed() {
        let result = RateLimitResult::Allowed;
        assert!(result.is_allowed());
        assert!(result.wait_duration().is_none());
    }

    #[test]
    fn test_wait_is_not_allowed() {
        let result = RateLimitResult::Wait(Duration::from_millis(100));
        assert!(!result.is_allowed());
        assert_eq!(result.wait_duration(), Some(Duration::from_millis(100)));
    }
}

mod rate_limiter_basic {
    use super::*;

    #[tokio::test]
    async fn test_rate_limiter_no_limits() {
        let limiter = RateLimiter::new(None, None, None);

        let result = limiter.check(Some("client1"), Some(1), 1_000_000).await;
        assert!(result.is_allowed());
    }

    #[tokio::test]
    async fn test_rate_limiter_global_limit_allows() {
        let global = BandwidthLimit::new(10_000_000, Some(10_000_000));
        let limiter = RateLimiter::new(Some(global), None, None);

        let result = limiter.check(None, None, 1_000_000).await;
        assert!(result.is_allowed());
    }

    #[tokio::test]
    async fn test_rate_limiter_client_limit_allows() {
        let client_limit = BandwidthLimit::new(5_000_000, Some(5_000_000));
        let limiter = RateLimiter::new(None, Some(client_limit), None);

        let result = limiter.check(Some("client1"), None, 1_000_000).await;
        assert!(result.is_allowed());
    }

    #[tokio::test]
    async fn test_rate_limiter_device_limit_allows() {
        let device_limit = BandwidthLimit::new(5_000_000, Some(5_000_000));
        let limiter = RateLimiter::new(None, None, Some(device_limit));

        let result = limiter.check(None, Some(1), 1_000_000).await;
        assert!(result.is_allowed());
    }

    #[tokio::test]
    async fn test_rate_limiter_all_limits() {
        let global = BandwidthLimit::new(100_000_000, Some(100_000_000));
        let client = BandwidthLimit::new(50_000_000, Some(50_000_000));
        let device = BandwidthLimit::new(25_000_000, Some(25_000_000));

        let limiter = RateLimiter::new(Some(global), Some(client), Some(device));

        let result = limiter.check(Some("client1"), Some(1), 1_000_000).await;
        assert!(result.is_allowed());
    }
}

mod rate_limiter_exhaustion {
    use super::*;

    #[tokio::test]
    async fn test_rate_limiter_global_exhaustion() {
        let global = BandwidthLimit::new(1000, Some(1000));
        let limiter = RateLimiter::new(Some(global), None, None);

        limiter.record(None, None, 1000).await;

        let result = limiter.check(None, None, 100).await;
        assert!(!result.is_allowed());
        assert!(result.wait_duration().is_some());
    }

    #[tokio::test]
    async fn test_rate_limiter_client_exhaustion() {
        let client_limit = BandwidthLimit::new(1000, Some(1000));
        let limiter = RateLimiter::new(None, Some(client_limit), None);

        limiter.record(Some("client1"), None, 1000).await;

        let result = limiter.check(Some("client1"), None, 100).await;
        assert!(!result.is_allowed());

        let result = limiter.check(Some("client2"), None, 100).await;
        assert!(result.is_allowed());
    }

    #[tokio::test]
    async fn test_rate_limiter_device_exhaustion() {
        let device_limit = BandwidthLimit::new(1000, Some(1000));
        let limiter = RateLimiter::new(None, None, Some(device_limit));

        limiter.record(None, Some(1), 1000).await;

        let result = limiter.check(None, Some(1), 100).await;
        assert!(!result.is_allowed());

        let result = limiter.check(None, Some(2), 100).await;
        assert!(result.is_allowed());
    }

    #[tokio::test]
    async fn test_rate_limiter_bottleneck_detection() {
        let global = BandwidthLimit::new(100_000, Some(100_000));
        let client = BandwidthLimit::new(10_000, Some(10_000));
        let device = BandwidthLimit::new(1_000, Some(1_000));

        let limiter = RateLimiter::new(Some(global), Some(client), Some(device));

        limiter.record(Some("client1"), Some(1), 1000).await;

        let result = limiter.check(Some("client1"), Some(1), 100).await;
        assert!(!result.is_allowed());

        let result = limiter.check(Some("client1"), Some(2), 100).await;
        assert!(result.is_allowed());
    }
}

mod try_acquire {
    use super::*;

    #[tokio::test]
    async fn test_try_acquire_success() {
        let global = BandwidthLimit::new(10_000, Some(10_000));
        let limiter = RateLimiter::new(Some(global), None, None);

        let success = limiter.try_acquire(None, None, 1000).await;
        assert!(success);
    }

    #[tokio::test]
    async fn test_try_acquire_failure() {
        let global = BandwidthLimit::new(1000, Some(1000));
        let limiter = RateLimiter::new(Some(global), None, None);

        let success = limiter.try_acquire(None, None, 1000).await;
        assert!(success);

        let success = limiter.try_acquire(None, None, 100).await;
        assert!(!success);
    }

    #[tokio::test]
    async fn test_try_acquire_consumes_tokens() {
        let global = BandwidthLimit::new(10_000, Some(10_000));
        let limiter = RateLimiter::new(Some(global), None, None);

        let success1 = limiter.try_acquire(None, None, 5000).await;
        assert!(success1);

        let success2 = limiter.try_acquire(None, None, 5000).await;
        assert!(success2);

        let success3 = limiter.try_acquire(None, None, 100).await;
        assert!(!success3);
    }

    #[tokio::test]
    async fn test_try_acquire_rollback_on_client_failure() {
        let global = BandwidthLimit::new(10_000, Some(10_000));
        let client = BandwidthLimit::new(1000, Some(1000));

        let limiter = RateLimiter::new(Some(global), Some(client), None);

        limiter.try_acquire(Some("client1"), None, 1000).await;

        let success = limiter.try_acquire(Some("client1"), None, 500).await;
        assert!(!success);

        let result = limiter.check(None, None, 9000).await;
        assert!(result.is_allowed());
    }

    #[tokio::test]
    async fn test_try_acquire_rollback_on_device_failure() {
        let global = BandwidthLimit::new(10_000, Some(10_000));
        let client = BandwidthLimit::new(5_000, Some(5_000));
        let device = BandwidthLimit::new(1000, Some(1000));

        let limiter = RateLimiter::new(Some(global), Some(client), Some(device));

        limiter.try_acquire(Some("client1"), Some(1), 1000).await;

        let success = limiter.try_acquire(Some("client1"), Some(1), 500).await;
        assert!(!success);

        let result = limiter.check(None, None, 9000).await;
        assert!(result.is_allowed());

        let result = limiter.check(Some("client1"), None, 4000).await;
        assert!(result.is_allowed());
    }
}

mod record_and_refill {
    use super::*;

    #[tokio::test]
    async fn test_record_consumes_tokens() {
        let global = BandwidthLimit::new(1000, Some(1000));
        let limiter = RateLimiter::new(Some(global), None, None);

        limiter.record(None, None, 1000).await;

        let result = limiter.check(None, None, 500).await;
        assert!(!result.is_allowed());
    }

    #[tokio::test]
    async fn test_record_all_buckets() {
        let global = BandwidthLimit::new(10_000, Some(10_000));
        let client = BandwidthLimit::new(5_000, Some(5_000));
        let device = BandwidthLimit::new(2_000, Some(2_000));

        let limiter = RateLimiter::new(Some(global), Some(client), Some(device));

        limiter.record(Some("client1"), Some(1), 2000).await;

        let result = limiter.check(Some("client1"), Some(1), 100).await;
        assert!(!result.is_allowed());
    }
}

mod client_device_management {
    use super::*;

    #[tokio::test]
    async fn test_remove_client() {
        let client_limit = BandwidthLimit::new(1000, Some(1000));
        let limiter = RateLimiter::new(None, Some(client_limit), None);

        limiter.record(Some("client1"), None, 1000).await;

        let result = limiter.check(Some("client1"), None, 100).await;
        assert!(!result.is_allowed());

        limiter.remove_client("client1").await;

        let result = limiter.check(Some("client1"), None, 100).await;
        assert!(result.is_allowed());
    }

    #[tokio::test]
    async fn test_remove_device() {
        let device_limit = BandwidthLimit::new(1000, Some(1000));
        let limiter = RateLimiter::new(None, None, Some(device_limit));

        limiter.record(None, Some(1), 1000).await;

        let result = limiter.check(None, Some(1), 100).await;
        assert!(!result.is_allowed());

        limiter.remove_device(1).await;

        let result = limiter.check(None, Some(1), 100).await;
        assert!(result.is_allowed());
    }

    #[tokio::test]
    async fn test_multiple_clients_independent() {
        let client_limit = BandwidthLimit::new(1000, Some(1000));
        let limiter = RateLimiter::new(None, Some(client_limit), None);

        limiter.record(Some("client1"), None, 1000).await;

        let result1 = limiter.check(Some("client1"), None, 100).await;
        assert!(!result1.is_allowed());

        let result2 = limiter.check(Some("client2"), None, 500).await;
        assert!(result2.is_allowed());

        let result3 = limiter.check(Some("client3"), None, 1000).await;
        assert!(result3.is_allowed());
    }

    #[tokio::test]
    async fn test_multiple_devices_independent() {
        let device_limit = BandwidthLimit::new(1000, Some(1000));
        let limiter = RateLimiter::new(None, None, Some(device_limit));

        limiter.record(None, Some(1), 1000).await;
        limiter.record(None, Some(2), 500).await;

        let result1 = limiter.check(None, Some(1), 100).await;
        assert!(!result1.is_allowed());

        let result2 = limiter.check(None, Some(2), 500).await;
        assert!(result2.is_allowed());

        let result3 = limiter.check(None, Some(3), 1000).await;
        assert!(result3.is_allowed());
    }
}

mod bandwidth_metrics {
    use super::*;

    #[test]
    fn test_metrics_record() {
        let mut metrics = BandwidthMetrics::default();

        metrics.record(1000);
        assert_eq!(metrics.total_bytes, 1000);
        assert_eq!(metrics.bytes_per_second, 1000);

        metrics.record(500);
        assert_eq!(metrics.total_bytes, 1500);
    }

    #[test]
    fn test_metrics_record_limited() {
        let mut metrics = BandwidthMetrics::default();

        metrics.record_limited();
        assert_eq!(metrics.rate_limited_count, 1);

        metrics.record_limited();
        metrics.record_limited();
        assert_eq!(metrics.rate_limited_count, 3);
    }

    #[test]
    fn test_metrics_last_update() {
        let mut metrics = BandwidthMetrics::default();
        assert!(metrics.last_update.is_none());

        metrics.record(100);
        assert!(metrics.last_update.is_some());
    }
}

mod metrics_tracker {
    use super::*;

    #[tokio::test]
    async fn test_metrics_tracker_global() {
        let tracker = MetricsTracker::new();

        tracker.record(None, None, 1000).await;
        tracker.record(None, None, 500).await;

        let global = tracker.global_metrics().await;
        assert_eq!(global.total_bytes, 1500);
    }

    #[tokio::test]
    async fn test_metrics_tracker_client() {
        let tracker = MetricsTracker::new();

        tracker.record(Some("client1"), None, 1000).await;
        tracker.record(Some("client1"), None, 500).await;
        tracker.record(Some("client2"), None, 2000).await;

        let client1 = tracker.client_metrics("client1").await.unwrap();
        assert_eq!(client1.total_bytes, 1500);

        let client2 = tracker.client_metrics("client2").await.unwrap();
        assert_eq!(client2.total_bytes, 2000);

        let client3 = tracker.client_metrics("client3").await;
        assert!(client3.is_none());
    }

    #[tokio::test]
    async fn test_metrics_tracker_device() {
        let tracker = MetricsTracker::new();

        tracker.record(None, Some(1), 1000).await;
        tracker.record(None, Some(1), 500).await;
        tracker.record(None, Some(2), 2000).await;

        let device1 = tracker.device_metrics(1).await.unwrap();
        assert_eq!(device1.total_bytes, 1500);

        let device2 = tracker.device_metrics(2).await.unwrap();
        assert_eq!(device2.total_bytes, 2000);

        let device3 = tracker.device_metrics(3).await;
        assert!(device3.is_none());
    }

    #[tokio::test]
    async fn test_metrics_tracker_combined() {
        let tracker = MetricsTracker::new();

        tracker.record(Some("client1"), Some(1), 1000).await;

        let global = tracker.global_metrics().await;
        assert_eq!(global.total_bytes, 1000);

        let client = tracker.client_metrics("client1").await.unwrap();
        assert_eq!(client.total_bytes, 1000);

        let device = tracker.device_metrics(1).await.unwrap();
        assert_eq!(device.total_bytes, 1000);
    }
}

mod wait_time_calculation {
    use super::*;

    #[tokio::test]
    async fn test_wait_time_proportional_to_deficit() {
        let global = BandwidthLimit::new(1000, Some(1000));
        let limiter = RateLimiter::new(Some(global), None, None);

        limiter.record(None, None, 1000).await;

        let result = limiter.check(None, None, 100).await;
        match result {
            RateLimitResult::Wait(duration) => {
                assert!(duration.as_millis() >= 90);
                assert!(duration.as_millis() <= 200);
            }
            _ => panic!("Expected Wait result"),
        }
    }

    #[tokio::test]
    async fn test_wait_time_larger_for_larger_request() {
        let global = BandwidthLimit::new(1000, Some(1000));
        let limiter = RateLimiter::new(Some(global), None, None);

        limiter.record(None, None, 1000).await;

        let result1 = limiter.check(None, None, 100).await;
        let result2 = limiter.check(None, None, 500).await;

        let wait1 = match result1 {
            RateLimitResult::Wait(d) => d,
            _ => panic!("Expected Wait"),
        };

        let wait2 = match result2 {
            RateLimitResult::Wait(d) => d,
            _ => panic!("Expected Wait"),
        };

        assert!(wait2 > wait1);
    }

    #[tokio::test]
    async fn test_wait_time_max_from_all_buckets() {
        let global = BandwidthLimit::new(10_000, Some(10_000));
        let client = BandwidthLimit::new(5_000, Some(5_000));
        let device = BandwidthLimit::new(1_000, Some(1_000));

        let limiter = RateLimiter::new(Some(global), Some(client), Some(device));

        limiter.record(Some("client1"), Some(1), 1000).await;

        let result = limiter.check(Some("client1"), Some(1), 500).await;

        assert!(!result.is_allowed());
    }
}

mod concurrent_access {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_concurrent_check() {
        let global = BandwidthLimit::new(100_000, Some(100_000));
        let limiter = Arc::new(RateLimiter::new(Some(global), None, None));

        let mut handles = Vec::new();

        for i in 0..10 {
            let limiter_clone = limiter.clone();
            let handle = tokio::spawn(async move {
                let result = limiter_clone.check(None, None, 1000).await;
                (i, result.is_allowed())
            });
            handles.push(handle);
        }

        let mut results = Vec::new();
        for handle in handles {
            results.push(handle.await.unwrap());
        }

        let allowed_count = results.iter().filter(|(_, allowed)| *allowed).count();
        assert_eq!(allowed_count, 10);
    }

    #[tokio::test]
    async fn test_concurrent_record() {
        let global = BandwidthLimit::new(10_000, Some(10_000));
        let limiter = Arc::new(RateLimiter::new(Some(global), None, None));

        let mut handles = Vec::new();

        for _ in 0..10 {
            let limiter_clone = limiter.clone();
            let handle = tokio::spawn(async move {
                limiter_clone.record(None, None, 1000).await;
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.await.unwrap();
        }

        let result = limiter.check(None, None, 1).await;
        assert!(!result.is_allowed());
    }

    #[tokio::test]
    async fn test_concurrent_different_clients() {
        let client_limit = BandwidthLimit::new(10_000, Some(10_000));
        let limiter = Arc::new(RateLimiter::new(None, Some(client_limit), None));

        let mut handles = Vec::new();

        for i in 0..10 {
            let limiter_clone = limiter.clone();
            let client_id = format!("client{}", i);
            let handle = tokio::spawn(async move {
                limiter_clone.record(Some(&client_id), None, 5000).await;
                let result = limiter_clone.check(Some(&client_id), None, 5000).await;
                (client_id, result.is_allowed())
            });
            handles.push(handle);
        }

        let mut results = Vec::new();
        for handle in handles {
            results.push(handle.await.unwrap());
        }

        for (client_id, allowed) in results {
            assert!(
                allowed,
                "Client {} should have independent bucket",
                client_id
            );
        }
    }
}

mod edge_cases {
    use super::*;

    #[tokio::test]
    async fn test_zero_byte_check() {
        let global = BandwidthLimit::new(1000, Some(1000));
        let limiter = RateLimiter::new(Some(global), None, None);

        let result = limiter.check(None, None, 0).await;
        assert!(result.is_allowed());
    }

    #[tokio::test]
    async fn test_zero_byte_record() {
        let global = BandwidthLimit::new(1000, Some(1000));
        let limiter = RateLimiter::new(Some(global), None, None);

        limiter.record(None, None, 0).await;

        let result = limiter.check(None, None, 1000).await;
        assert!(result.is_allowed());
    }

    #[tokio::test]
    async fn test_very_large_request() {
        let global = BandwidthLimit::new(1000, Some(1000));
        let limiter = RateLimiter::new(Some(global), None, None);

        let result = limiter.check(None, None, 1_000_000).await;
        assert!(!result.is_allowed());
    }

    #[tokio::test]
    async fn test_request_equal_to_burst() {
        let global = BandwidthLimit::new(1000, Some(1000));
        let limiter = RateLimiter::new(Some(global), None, None);

        let result = limiter.check(None, None, 1000).await;
        assert!(result.is_allowed());
    }

    #[tokio::test]
    async fn test_request_just_over_burst() {
        let global = BandwidthLimit::new(1000, Some(1000));
        let limiter = RateLimiter::new(Some(global), None, None);

        let result = limiter.check(None, None, 1001).await;
        assert!(!result.is_allowed());
    }

    #[tokio::test]
    async fn test_none_client_with_client_limit() {
        let client_limit = BandwidthLimit::new(1000, Some(1000));
        let limiter = RateLimiter::new(None, Some(client_limit), None);

        let result = limiter.check(None, None, 500).await;
        assert!(result.is_allowed());

        limiter.record(None, None, 500).await;

        let result = limiter.check(None, None, 500).await;
        assert!(result.is_allowed());
    }

    #[tokio::test]
    async fn test_none_device_with_device_limit() {
        let device_limit = BandwidthLimit::new(1000, Some(1000));
        let limiter = RateLimiter::new(None, None, Some(device_limit));

        let result = limiter.check(None, None, 500).await;
        assert!(result.is_allowed());

        limiter.record(None, None, 500).await;

        let result = limiter.check(None, None, 500).await;
        assert!(result.is_allowed());
    }
}
