//! Integration tests for PolicyEngine
//!
//! Tests policy enforcement including:
//! - Time window restrictions
//! - Session duration limits
//! - Client allowlists
//! - Device class restrictions

use protocol::{DeviceHandle, DeviceId, DeviceInfo, DeviceSpeed, SharingMode};
use std::time::Duration;

fn make_device_info(vid: u16, pid: u16, class: u8) -> DeviceInfo {
    DeviceInfo {
        id: DeviceId(1),
        vendor_id: vid,
        product_id: pid,
        bus_number: 1,
        device_address: 1,
        manufacturer: Some("Test".to_string()),
        product: Some("Test Device".to_string()),
        serial_number: None,
        class,
        subclass: 0,
        protocol: 0,
        speed: DeviceSpeed::High,
        num_configurations: 1,
    }
}

mod policy_decision {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum PolicyDecision {
        Allow,
        Deny(PolicyDenialReason),
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum PolicyDenialReason {
        ClientNotAllowed,
        OutsideTimeWindow {
            current_time: String,
            allowed_windows: Vec<String>,
        },
        SessionDurationExceeded {
            max_duration_secs: u64,
        },
        DeviceClassRestricted {
            device_class: u8,
        },
        NoMatchingPolicy,
    }

    #[test]
    fn test_allow_decision() {
        let decision = PolicyDecision::Allow;
        assert_eq!(decision, PolicyDecision::Allow);
    }

    #[test]
    fn test_deny_client_not_allowed() {
        let decision = PolicyDecision::Deny(PolicyDenialReason::ClientNotAllowed);
        assert!(matches!(
            decision,
            PolicyDecision::Deny(PolicyDenialReason::ClientNotAllowed)
        ));
    }

    #[test]
    fn test_deny_outside_time_window() {
        let decision = PolicyDecision::Deny(PolicyDenialReason::OutsideTimeWindow {
            current_time: "18:00".to_string(),
            allowed_windows: vec!["09:00-17:00".to_string()],
        });

        match decision {
            PolicyDecision::Deny(PolicyDenialReason::OutsideTimeWindow {
                current_time,
                allowed_windows,
            }) => {
                assert_eq!(current_time, "18:00");
                assert_eq!(allowed_windows, vec!["09:00-17:00".to_string()]);
            }
            _ => panic!("Expected OutsideTimeWindow"),
        }
    }

    #[test]
    fn test_deny_session_duration_exceeded() {
        let decision = PolicyDecision::Deny(PolicyDenialReason::SessionDurationExceeded {
            max_duration_secs: 3600,
        });

        match decision {
            PolicyDecision::Deny(PolicyDenialReason::SessionDurationExceeded { max_duration_secs }) => {
                assert_eq!(max_duration_secs, 3600);
            }
            _ => panic!("Expected SessionDurationExceeded"),
        }
    }

    #[test]
    fn test_deny_device_class_restricted() {
        let decision = PolicyDecision::Deny(PolicyDenialReason::DeviceClassRestricted {
            device_class: 8,
        });

        match decision {
            PolicyDecision::Deny(PolicyDenialReason::DeviceClassRestricted { device_class }) => {
                assert_eq!(device_class, 8);
            }
            _ => panic!("Expected DeviceClassRestricted"),
        }
    }

    #[test]
    fn test_deny_no_matching_policy() {
        let decision = PolicyDecision::Deny(PolicyDenialReason::NoMatchingPolicy);
        assert!(matches!(
            decision,
            PolicyDecision::Deny(PolicyDenialReason::NoMatchingPolicy)
        ));
    }
}

mod time_window_parsing {
    use super::*;

    fn parse_time(time: &str) -> Option<(u8, u8)> {
        let parts: Vec<&str> = time.trim().split(':').collect();
        if parts.len() != 2 {
            return None;
        }

        let hours: u8 = parts[0].parse().ok()?;
        let minutes: u8 = parts[1].parse().ok()?;

        if hours > 23 || minutes > 59 {
            return None;
        }

        Some((hours, minutes))
    }

    fn parse_time_window(window: &str) -> Option<((u8, u8), (u8, u8))> {
        let parts: Vec<&str> = window.split('-').collect();
        if parts.len() != 2 {
            return None;
        }

        let start = parse_time(parts[0])?;
        let end = parse_time(parts[1])?;

        Some((start, end))
    }

    #[test]
    fn test_parse_time_valid() {
        assert_eq!(parse_time("09:00"), Some((9, 0)));
        assert_eq!(parse_time("17:30"), Some((17, 30)));
        assert_eq!(parse_time("00:00"), Some((0, 0)));
        assert_eq!(parse_time("23:59"), Some((23, 59)));
    }

    #[test]
    fn test_parse_time_invalid() {
        assert_eq!(parse_time("24:00"), None);
        assert_eq!(parse_time("12:60"), None);
        assert_eq!(parse_time("invalid"), None);
        assert_eq!(parse_time("12"), None);
        assert_eq!(parse_time("12:00:00"), None);
    }

    #[test]
    fn test_parse_time_window_valid() {
        assert_eq!(
            parse_time_window("09:00-17:00"),
            Some(((9, 0), (17, 0)))
        );
        assert_eq!(
            parse_time_window("22:00-06:00"),
            Some(((22, 0), (6, 0)))
        );
        assert_eq!(
            parse_time_window("00:00-23:59"),
            Some(((0, 0), (23, 59)))
        );
    }

    #[test]
    fn test_parse_time_window_invalid() {
        assert_eq!(parse_time_window("09:00"), None);
        assert_eq!(parse_time_window("09:00-17:00-18:00"), None);
        assert_eq!(parse_time_window("invalid"), None);
        assert_eq!(parse_time_window("24:00-25:00"), None);
    }
}

mod time_window_checking {
    use super::*;

    fn time_in_range(time: (u8, u8), start: (u8, u8), end: (u8, u8)) -> bool {
        let time_mins = time.0 as u16 * 60 + time.1 as u16;
        let start_mins = start.0 as u16 * 60 + start.1 as u16;
        let end_mins = end.0 as u16 * 60 + end.1 as u16;

        if start_mins <= end_mins {
            time_mins >= start_mins && time_mins < end_mins
        } else {
            time_mins >= start_mins || time_mins < end_mins
        }
    }

    #[test]
    fn test_time_in_normal_range() {
        assert!(time_in_range((10, 0), (9, 0), (17, 0)));
        assert!(time_in_range((9, 0), (9, 0), (17, 0)));
        assert!(time_in_range((16, 59), (9, 0), (17, 0)));
    }

    #[test]
    fn test_time_outside_normal_range() {
        assert!(!time_in_range((17, 0), (9, 0), (17, 0)));
        assert!(!time_in_range((8, 59), (9, 0), (17, 0)));
        assert!(!time_in_range((18, 0), (9, 0), (17, 0)));
        assert!(!time_in_range((6, 0), (9, 0), (17, 0)));
    }

    #[test]
    fn test_time_in_overnight_range() {
        assert!(time_in_range((23, 0), (22, 0), (6, 0)));
        assert!(time_in_range((0, 0), (22, 0), (6, 0)));
        assert!(time_in_range((5, 59), (22, 0), (6, 0)));
        assert!(time_in_range((22, 0), (22, 0), (6, 0)));
    }

    #[test]
    fn test_time_outside_overnight_range() {
        assert!(!time_in_range((6, 0), (22, 0), (6, 0)));
        assert!(!time_in_range((10, 0), (22, 0), (6, 0)));
        assert!(!time_in_range((21, 59), (22, 0), (6, 0)));
    }

    #[test]
    fn test_time_in_multiple_windows() {
        let windows = vec![((9, 0), (12, 0)), ((13, 0), (17, 0))];

        let in_any_window = |time: (u8, u8)| -> bool {
            windows.iter().any(|(start, end)| time_in_range(time, *start, *end))
        };

        assert!(in_any_window((10, 0)));
        assert!(in_any_window((14, 0)));
        assert!(!in_any_window((12, 30)));
        assert!(!in_any_window((18, 0)));
    }
}

mod client_allowlist {
    use super::*;

    fn is_client_allowed(allowed_clients: &[String], client_id: &str) -> bool {
        if allowed_clients.is_empty() {
            return true;
        }

        if allowed_clients.iter().any(|c| c == "*") {
            return true;
        }

        allowed_clients
            .iter()
            .any(|c| c.eq_ignore_ascii_case(client_id))
    }

    #[test]
    fn test_empty_allowlist_allows_all() {
        let allowed: Vec<String> = Vec::new();
        assert!(is_client_allowed(&allowed, "any_client"));
        assert!(is_client_allowed(&allowed, "other_client"));
    }

    #[test]
    fn test_wildcard_allows_all() {
        let allowed = vec!["*".to_string()];
        assert!(is_client_allowed(&allowed, "any_client"));
        assert!(is_client_allowed(&allowed, "other_client"));
    }

    #[test]
    fn test_specific_client_allowed() {
        let allowed = vec!["client1".to_string(), "client2".to_string()];
        assert!(is_client_allowed(&allowed, "client1"));
        assert!(is_client_allowed(&allowed, "client2"));
        assert!(!is_client_allowed(&allowed, "client3"));
    }

    #[test]
    fn test_case_insensitive_match() {
        let allowed = vec!["ClientA".to_string()];
        assert!(is_client_allowed(&allowed, "clienta"));
        assert!(is_client_allowed(&allowed, "CLIENTA"));
        assert!(is_client_allowed(&allowed, "ClientA"));
    }

    #[test]
    fn test_mixed_wildcard_and_specific() {
        let allowed = vec!["client1".to_string(), "*".to_string()];
        assert!(is_client_allowed(&allowed, "any_client"));
    }
}

mod device_class_restrictions {
    use super::*;

    fn is_device_class_allowed(restricted_classes: &Option<Vec<u8>>, device_class: u8) -> bool {
        match restricted_classes {
            None => true,
            Some(classes) => !classes.contains(&device_class),
        }
    }

    #[test]
    fn test_no_restrictions_allows_all() {
        let restricted: Option<Vec<u8>> = None;
        assert!(is_device_class_allowed(&restricted, 1));
        assert!(is_device_class_allowed(&restricted, 8));
        assert!(is_device_class_allowed(&restricted, 255));
    }

    #[test]
    fn test_empty_restrictions_allows_all() {
        let restricted = Some(Vec::new());
        assert!(is_device_class_allowed(&restricted, 1));
        assert!(is_device_class_allowed(&restricted, 8));
    }

    #[test]
    fn test_restricted_class_denied() {
        let restricted = Some(vec![8u8]);
        assert!(is_device_class_allowed(&restricted, 1));
        assert!(is_device_class_allowed(&restricted, 7));
        assert!(!is_device_class_allowed(&restricted, 8));
        assert!(is_device_class_allowed(&restricted, 9));
    }

    #[test]
    fn test_multiple_restricted_classes() {
        let restricted = Some(vec![1u8, 8, 14]);
        assert!(!is_device_class_allowed(&restricted, 1));
        assert!(is_device_class_allowed(&restricted, 3));
        assert!(!is_device_class_allowed(&restricted, 8));
        assert!(is_device_class_allowed(&restricted, 9));
        assert!(!is_device_class_allowed(&restricted, 14));
    }

    #[test]
    fn test_common_device_classes() {
        let restricted = Some(vec![8u8]);

        let audio_class = 1;
        let cdc_class = 2;
        let hid_class = 3;
        let printer_class = 7;
        let mass_storage_class = 8;
        let hub_class = 9;
        let video_class = 14;

        assert!(is_device_class_allowed(&restricted, audio_class));
        assert!(is_device_class_allowed(&restricted, cdc_class));
        assert!(is_device_class_allowed(&restricted, hid_class));
        assert!(is_device_class_allowed(&restricted, printer_class));
        assert!(!is_device_class_allowed(&restricted, mass_storage_class));
        assert!(is_device_class_allowed(&restricted, hub_class));
        assert!(is_device_class_allowed(&restricted, video_class));
    }
}

mod device_filter_matching {
    use super::*;

    fn filter_matches_device(filter: &str, vendor_id: u16, product_id: u16) -> bool {
        let device_str = format!("{:04x}:{:04x}", vendor_id, product_id);

        if filter == "*" {
            return true;
        }

        if filter.ends_with(":*") {
            let vid_part = &filter[..filter.len() - 2];
            let device_vid = format!("{:04x}", vendor_id);
            return vid_part.to_lowercase() == device_vid;
        }

        filter.to_lowercase() == device_str.to_lowercase()
    }

    #[test]
    fn test_wildcard_matches_all() {
        assert!(filter_matches_device("*", 0x1234, 0x5678));
        assert!(filter_matches_device("*", 0x04f9, 0x0042));
        assert!(filter_matches_device("*", 0x0000, 0x0000));
    }

    #[test]
    fn test_exact_match() {
        assert!(filter_matches_device("1234:5678", 0x1234, 0x5678));
        assert!(filter_matches_device("04f9:0042", 0x04f9, 0x0042));
        assert!(!filter_matches_device("1234:5678", 0x1234, 0x5679));
        assert!(!filter_matches_device("1234:5678", 0x1235, 0x5678));
    }

    #[test]
    fn test_vendor_wildcard() {
        assert!(filter_matches_device("04f9:*", 0x04f9, 0x0042));
        assert!(filter_matches_device("04f9:*", 0x04f9, 0x1234));
        assert!(!filter_matches_device("04f9:*", 0x04f8, 0x0042));
    }

    #[test]
    fn test_case_insensitive() {
        assert!(filter_matches_device("04F9:0042", 0x04f9, 0x0042));
        assert!(filter_matches_device("04f9:0042", 0x04F9, 0x0042));
        assert!(filter_matches_device("ABCD:EF01", 0xabcd, 0xef01));
    }

    #[test]
    fn test_leading_zeros() {
        assert!(filter_matches_device("0001:0002", 0x0001, 0x0002));
        assert!(filter_matches_device("04f9:*", 0x04f9, 0x0001));
    }
}

mod policy_matching_priority {
    use super::*;

    #[derive(Debug, Clone)]
    struct MockPolicy {
        device_filter: String,
        allowed_clients: Vec<String>,
        time_windows: Option<Vec<String>>,
        max_session_duration: Option<Duration>,
        restricted_device_classes: Option<Vec<u8>>,
    }

    fn find_matching_policy<'a>(
        policies: &'a [MockPolicy],
        device_info: &DeviceInfo,
    ) -> Option<&'a MockPolicy> {
        let device_str = format!(
            "{:04x}:{:04x}",
            device_info.vendor_id, device_info.product_id
        );

        for policy in policies {
            if policy.device_filter != "*" && policy.device_filter != format!("{:04x}:*", device_info.vendor_id) {
                if policy.device_filter.to_lowercase() == device_str.to_lowercase() {
                    return Some(policy);
                }
            }
        }

        let vid_wildcard = format!("{:04x}:*", device_info.vendor_id);
        for policy in policies {
            if policy.device_filter == vid_wildcard {
                return Some(policy);
            }
        }

        policies.iter().find(|p| p.device_filter == "*")
    }

    #[test]
    fn test_exact_match_priority() {
        let policies = vec![
            MockPolicy {
                device_filter: "*".to_string(),
                allowed_clients: vec!["*".to_string()],
                time_windows: None,
                max_session_duration: None,
                restricted_device_classes: None,
            },
            MockPolicy {
                device_filter: "04f9:*".to_string(),
                allowed_clients: vec!["vendor_client".to_string()],
                time_windows: None,
                max_session_duration: None,
                restricted_device_classes: None,
            },
            MockPolicy {
                device_filter: "04f9:0042".to_string(),
                allowed_clients: vec!["specific_client".to_string()],
                time_windows: None,
                max_session_duration: None,
                restricted_device_classes: None,
            },
        ];

        let device = make_device_info(0x04f9, 0x0042, 0);
        let matched = find_matching_policy(&policies, &device);

        assert!(matched.is_some());
        assert_eq!(matched.unwrap().device_filter, "04f9:0042");
    }

    #[test]
    fn test_vendor_wildcard_priority() {
        let policies = vec![
            MockPolicy {
                device_filter: "*".to_string(),
                allowed_clients: vec!["*".to_string()],
                time_windows: None,
                max_session_duration: None,
                restricted_device_classes: None,
            },
            MockPolicy {
                device_filter: "04f9:*".to_string(),
                allowed_clients: vec!["vendor_client".to_string()],
                time_windows: None,
                max_session_duration: None,
                restricted_device_classes: None,
            },
        ];

        let device = make_device_info(0x04f9, 0x9999, 0);
        let matched = find_matching_policy(&policies, &device);

        assert!(matched.is_some());
        assert_eq!(matched.unwrap().device_filter, "04f9:*");
    }

    #[test]
    fn test_default_policy_fallback() {
        let policies = vec![
            MockPolicy {
                device_filter: "04f9:*".to_string(),
                allowed_clients: vec!["vendor_client".to_string()],
                time_windows: None,
                max_session_duration: None,
                restricted_device_classes: None,
            },
            MockPolicy {
                device_filter: "*".to_string(),
                allowed_clients: vec!["*".to_string()],
                time_windows: None,
                max_session_duration: None,
                restricted_device_classes: None,
            },
        ];

        let device = make_device_info(0x1234, 0x5678, 0);
        let matched = find_matching_policy(&policies, &device);

        assert!(matched.is_some());
        assert_eq!(matched.unwrap().device_filter, "*");
    }

    #[test]
    fn test_no_matching_policy() {
        let policies = vec![MockPolicy {
            device_filter: "04f9:*".to_string(),
            allowed_clients: vec!["vendor_client".to_string()],
            time_windows: None,
            max_session_duration: None,
            restricted_device_classes: None,
        }];

        let device = make_device_info(0x1234, 0x5678, 0);
        let matched = find_matching_policy(&policies, &device);

        assert!(matched.is_none());
    }
}

mod session_duration_limits {
    use super::*;
    use std::time::Instant;

    #[derive(Debug, Clone)]
    struct ActiveSession {
        handle: DeviceHandle,
        device_id: DeviceId,
        started_at: Instant,
        max_duration: Option<Duration>,
    }

    impl ActiveSession {
        fn is_expired(&self) -> bool {
            if let Some(max_duration) = self.max_duration {
                self.started_at.elapsed() >= max_duration
            } else {
                false
            }
        }

        fn time_remaining(&self) -> Option<Duration> {
            self.max_duration.map(|max| {
                let elapsed = self.started_at.elapsed();
                if elapsed >= max {
                    Duration::ZERO
                } else {
                    max - elapsed
                }
            })
        }
    }

    #[test]
    fn test_session_no_duration_limit() {
        let session = ActiveSession {
            handle: DeviceHandle(1),
            device_id: DeviceId(1),
            started_at: Instant::now() - Duration::from_secs(86400),
            max_duration: None,
        };

        assert!(!session.is_expired());
        assert!(session.time_remaining().is_none());
    }

    #[test]
    fn test_session_not_expired() {
        let session = ActiveSession {
            handle: DeviceHandle(1),
            device_id: DeviceId(1),
            started_at: Instant::now() - Duration::from_secs(1800),
            max_duration: Some(Duration::from_secs(3600)),
        };

        assert!(!session.is_expired());

        let remaining = session.time_remaining().unwrap();
        assert!(remaining > Duration::from_secs(1700));
        assert!(remaining <= Duration::from_secs(1800));
    }

    #[test]
    fn test_session_expired() {
        let session = ActiveSession {
            handle: DeviceHandle(1),
            device_id: DeviceId(1),
            started_at: Instant::now() - Duration::from_secs(3601),
            max_duration: Some(Duration::from_secs(3600)),
        };

        assert!(session.is_expired());
        assert_eq!(session.time_remaining(), Some(Duration::ZERO));
    }

    #[test]
    fn test_session_at_exact_limit() {
        let max = Duration::from_secs(3600);
        let session = ActiveSession {
            handle: DeviceHandle(1),
            device_id: DeviceId(1),
            started_at: Instant::now() - max,
            max_duration: Some(max),
        };

        assert!(session.is_expired());
    }
}

mod session_expiration_events {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum SessionExpiredReason {
        DurationLimitReached,
        TimeWindowExpired,
    }

    #[derive(Debug, Clone)]
    struct SessionExpiredEvent {
        handle: DeviceHandle,
        device_id: DeviceId,
        reason: SessionExpiredReason,
    }

    #[test]
    fn test_duration_limit_reached_event() {
        let event = SessionExpiredEvent {
            handle: DeviceHandle(1),
            device_id: DeviceId(1),
            reason: SessionExpiredReason::DurationLimitReached,
        };

        assert_eq!(event.handle, DeviceHandle(1));
        assert_eq!(event.device_id, DeviceId(1));
        assert_eq!(event.reason, SessionExpiredReason::DurationLimitReached);
    }

    #[test]
    fn test_time_window_expired_event() {
        let event = SessionExpiredEvent {
            handle: DeviceHandle(2),
            device_id: DeviceId(2),
            reason: SessionExpiredReason::TimeWindowExpired,
        };

        assert_eq!(event.handle, DeviceHandle(2));
        assert_eq!(event.device_id, DeviceId(2));
        assert_eq!(event.reason, SessionExpiredReason::TimeWindowExpired);
    }

    #[test]
    fn test_multiple_expiration_events() {
        let events = vec![
            SessionExpiredEvent {
                handle: DeviceHandle(1),
                device_id: DeviceId(1),
                reason: SessionExpiredReason::DurationLimitReached,
            },
            SessionExpiredEvent {
                handle: DeviceHandle(2),
                device_id: DeviceId(2),
                reason: SessionExpiredReason::TimeWindowExpired,
            },
        ];

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].reason, SessionExpiredReason::DurationLimitReached);
        assert_eq!(events[1].reason, SessionExpiredReason::TimeWindowExpired);
    }
}

mod timezone_handling {
    use super::*;

    fn get_time_with_offset(utc_hours: u8, utc_minutes: u8, offset_hours: i32) -> (u8, u8) {
        let total_minutes = utc_hours as i32 * 60 + utc_minutes as i32 + offset_hours * 60;
        let adjusted_minutes = total_minutes.rem_euclid(24 * 60);
        let hours = (adjusted_minutes / 60) as u8;
        let minutes = (adjusted_minutes % 60) as u8;
        (hours, minutes)
    }

    #[test]
    fn test_positive_offset() {
        assert_eq!(get_time_with_offset(10, 0, 2), (12, 0));
        assert_eq!(get_time_with_offset(10, 30, 2), (12, 30));
    }

    #[test]
    fn test_negative_offset() {
        assert_eq!(get_time_with_offset(10, 0, -5), (5, 0));
        assert_eq!(get_time_with_offset(10, 30, -5), (5, 30));
    }

    #[test]
    fn test_offset_crossing_midnight_forward() {
        assert_eq!(get_time_with_offset(23, 0, 2), (1, 0));
        assert_eq!(get_time_with_offset(22, 30, 3), (1, 30));
    }

    #[test]
    fn test_offset_crossing_midnight_backward() {
        assert_eq!(get_time_with_offset(1, 0, -3), (22, 0));
        assert_eq!(get_time_with_offset(0, 30, -1), (23, 30));
    }

    #[test]
    fn test_zero_offset() {
        assert_eq!(get_time_with_offset(10, 30, 0), (10, 30));
        assert_eq!(get_time_with_offset(0, 0, 0), (0, 0));
        assert_eq!(get_time_with_offset(23, 59, 0), (23, 59));
    }

    #[test]
    fn test_extreme_offsets() {
        assert_eq!(get_time_with_offset(12, 0, 12), (0, 0));
        assert_eq!(get_time_with_offset(12, 0, -12), (0, 0));
    }
}

mod window_expiry_calculation {
    use super::*;
    use std::time::Instant;

    fn calculate_window_expiry(
        current_time: (u8, u8),
        window_end: (u8, u8),
    ) -> Duration {
        let current_mins = current_time.0 as i32 * 60 + current_time.1 as i32;
        let end_mins = window_end.0 as i32 * 60 + window_end.1 as i32;

        let mins_until_end = if end_mins > current_mins {
            end_mins - current_mins
        } else {
            (24 * 60 - current_mins) + end_mins
        };

        Duration::from_secs(mins_until_end as u64 * 60)
    }

    #[test]
    fn test_expiry_same_day() {
        let expiry = calculate_window_expiry((10, 0), (17, 0));
        assert_eq!(expiry, Duration::from_secs(7 * 60 * 60));
    }

    #[test]
    fn test_expiry_minutes_precision() {
        let expiry = calculate_window_expiry((10, 30), (17, 45));
        let expected = Duration::from_secs(7 * 60 * 60 + 15 * 60);
        assert_eq!(expiry, expected);
    }

    #[test]
    fn test_expiry_overnight_current_before_midnight() {
        let expiry = calculate_window_expiry((23, 0), (6, 0));
        assert_eq!(expiry, Duration::from_secs(7 * 60 * 60));
    }

    #[test]
    fn test_expiry_overnight_current_after_midnight() {
        let expiry = calculate_window_expiry((2, 0), (6, 0));
        assert_eq!(expiry, Duration::from_secs(4 * 60 * 60));
    }

    #[test]
    fn test_expiry_almost_at_end() {
        let expiry = calculate_window_expiry((16, 59), (17, 0));
        assert_eq!(expiry, Duration::from_secs(60));
    }
}

mod policy_evaluation_integration {
    use super::*;

    #[derive(Debug, Clone)]
    struct TestPolicy {
        device_filter: String,
        allowed_clients: Vec<String>,
        time_windows: Option<Vec<String>>,
        restricted_device_classes: Option<Vec<u8>>,
    }

    #[derive(Debug, PartialEq)]
    enum TestDecision {
        Allow,
        DenyClient,
        DenyTimeWindow,
        DenyDeviceClass,
    }

    fn evaluate_policy(
        policy: &TestPolicy,
        client_id: &str,
        device_class: u8,
        current_time: (u8, u8),
    ) -> TestDecision {
        if !policy.allowed_clients.is_empty()
            && !policy.allowed_clients.iter().any(|c| c == "*")
            && !policy.allowed_clients.iter().any(|c| c.eq_ignore_ascii_case(client_id))
        {
            return TestDecision::DenyClient;
        }

        if let Some(ref classes) = policy.restricted_device_classes {
            if classes.contains(&device_class) {
                return TestDecision::DenyDeviceClass;
            }
        }

        if let Some(ref windows) = policy.time_windows {
            if !windows.is_empty() {
                let in_window = windows.iter().any(|w| {
                    if let Some((start, end)) = parse_window(w) {
                        time_in_range(current_time, start, end)
                    } else {
                        false
                    }
                });

                if !in_window {
                    return TestDecision::DenyTimeWindow;
                }
            }
        }

        TestDecision::Allow
    }

    fn parse_window(w: &str) -> Option<((u8, u8), (u8, u8))> {
        let parts: Vec<&str> = w.split('-').collect();
        if parts.len() != 2 {
            return None;
        }

        let start_parts: Vec<&str> = parts[0].split(':').collect();
        let end_parts: Vec<&str> = parts[1].split(':').collect();

        if start_parts.len() != 2 || end_parts.len() != 2 {
            return None;
        }

        let start = (
            start_parts[0].parse().ok()?,
            start_parts[1].parse().ok()?,
        );
        let end = (
            end_parts[0].parse().ok()?,
            end_parts[1].parse().ok()?,
        );

        Some((start, end))
    }

    fn time_in_range(time: (u8, u8), start: (u8, u8), end: (u8, u8)) -> bool {
        let time_mins = time.0 as u16 * 60 + time.1 as u16;
        let start_mins = start.0 as u16 * 60 + start.1 as u16;
        let end_mins = end.0 as u16 * 60 + end.1 as u16;

        if start_mins <= end_mins {
            time_mins >= start_mins && time_mins < end_mins
        } else {
            time_mins >= start_mins || time_mins < end_mins
        }
    }

    #[test]
    fn test_allow_all_conditions_pass() {
        let policy = TestPolicy {
            device_filter: "*".to_string(),
            allowed_clients: vec!["*".to_string()],
            time_windows: Some(vec!["09:00-17:00".to_string()]),
            restricted_device_classes: Some(vec![8]),
        };

        let result = evaluate_policy(&policy, "any_client", 3, (10, 0));
        assert_eq!(result, TestDecision::Allow);
    }

    #[test]
    fn test_deny_client_not_in_list() {
        let policy = TestPolicy {
            device_filter: "*".to_string(),
            allowed_clients: vec!["allowed_client".to_string()],
            time_windows: None,
            restricted_device_classes: None,
        };

        let result = evaluate_policy(&policy, "other_client", 0, (10, 0));
        assert_eq!(result, TestDecision::DenyClient);
    }

    #[test]
    fn test_deny_outside_time_window() {
        let policy = TestPolicy {
            device_filter: "*".to_string(),
            allowed_clients: vec!["*".to_string()],
            time_windows: Some(vec!["09:00-17:00".to_string()]),
            restricted_device_classes: None,
        };

        let result = evaluate_policy(&policy, "any_client", 0, (18, 0));
        assert_eq!(result, TestDecision::DenyTimeWindow);
    }

    #[test]
    fn test_deny_restricted_device_class() {
        let policy = TestPolicy {
            device_filter: "*".to_string(),
            allowed_clients: vec!["*".to_string()],
            time_windows: None,
            restricted_device_classes: Some(vec![8]),
        };

        let result = evaluate_policy(&policy, "any_client", 8, (10, 0));
        assert_eq!(result, TestDecision::DenyDeviceClass);
    }

    #[test]
    fn test_client_check_before_time_check() {
        let policy = TestPolicy {
            device_filter: "*".to_string(),
            allowed_clients: vec!["allowed_only".to_string()],
            time_windows: Some(vec!["09:00-17:00".to_string()]),
            restricted_device_classes: None,
        };

        let result = evaluate_policy(&policy, "other_client", 0, (18, 0));
        assert_eq!(result, TestDecision::DenyClient);
    }

    #[test]
    fn test_device_class_check_before_time_check() {
        let policy = TestPolicy {
            device_filter: "*".to_string(),
            allowed_clients: vec!["*".to_string()],
            time_windows: Some(vec!["09:00-17:00".to_string()]),
            restricted_device_classes: Some(vec![8]),
        };

        let result = evaluate_policy(&policy, "any_client", 8, (18, 0));
        assert_eq!(result, TestDecision::DenyDeviceClass);
    }
}
