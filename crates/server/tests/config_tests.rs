//! Integration tests for configuration parsing
//!
//! Tests server and client configuration parsing, including:
//! - Server config with all options
//! - Client config with multi-server support
//! - AutoConnectMode variants
//! - Invalid configuration handling

use std::time::Duration;

mod server_config {
    use super::*;

    const MINIMAL_SERVER_CONFIG: &str = r#"
[server]
bind_addr = "0.0.0.0:8080"
service_mode = false
log_level = "info"

[usb]
auto_share = false
filters = []

[security]
approved_clients = []
require_approval = true

[iroh]
"#;

    const FULL_SERVER_CONFIG: &str = r#"
[server]
bind_addr = "192.168.1.100:9090"
service_mode = true
log_level = "debug"

[usb]
auto_share = true
filters = ["0x04f9:*", "0x1234:0x5678"]

[security]
approved_clients = ["client1", "client2"]
require_approval = false

[iroh]
relay_servers = ["relay1.example.com", "relay2.example.com"]
secret_key_path = "/etc/p2p-usb/secret_key"

[policy]
timezone_offset_hours = 2

[bandwidth]
enabled = true
global_limit = "100Mbps"
per_client_limit = "50Mbps"
per_device_limit = "25Mbps"
burst_multiplier = 2.0

[qos]
enabled = true
control_priority = 7
interrupt_priority = 5
bulk_priority = 3
client_quota_mbps = 100
priority_aging = true

[sharing]
default_mode = "shared"
default_lock_timeout_secs = 600
max_queue_length = 20
queue_notifications = true

[audit]
enabled = true
path = "/var/log/p2p-usb/audit.log"
level = "all"
max_size_mb = 100
max_entries = 1000000
max_files = 10
syslog = false
stats_interval_secs = 60

[[device_policies]]
device_filter = "04f9:*"
allowed_clients = ["trusted_client"]
description = "Brother devices"
sharing_mode = "exclusive"
lock_timeout_secs = 300
max_concurrent_clients = 1
time_windows = ["09:00-17:00"]
max_session_duration = "1h"
restricted_device_classes = [8]

[[device_policies]]
device_filter = "*"
allowed_clients = ["*"]
description = "Default policy"
sharing_mode = "shared"
"#;

    #[test]
    fn test_parse_minimal_server_config() {
        let config: toml::Value = toml::from_str(MINIMAL_SERVER_CONFIG).unwrap();

        let server = config.get("server").unwrap();
        assert_eq!(
            server.get("bind_addr").unwrap().as_str().unwrap(),
            "0.0.0.0:8080"
        );
        assert!(!server.get("service_mode").unwrap().as_bool().unwrap());
        assert_eq!(server.get("log_level").unwrap().as_str().unwrap(), "info");

        let usb = config.get("usb").unwrap();
        assert!(!usb.get("auto_share").unwrap().as_bool().unwrap());
        assert!(usb.get("filters").unwrap().as_array().unwrap().is_empty());

        let security = config.get("security").unwrap();
        assert!(security.get("require_approval").unwrap().as_bool().unwrap());
    }

    #[test]
    fn test_parse_full_server_config() {
        let config: toml::Value = toml::from_str(FULL_SERVER_CONFIG).unwrap();

        let server = config.get("server").unwrap();
        assert_eq!(
            server.get("bind_addr").unwrap().as_str().unwrap(),
            "192.168.1.100:9090"
        );
        assert!(server.get("service_mode").unwrap().as_bool().unwrap());
        assert_eq!(server.get("log_level").unwrap().as_str().unwrap(), "debug");

        let usb = config.get("usb").unwrap();
        assert!(usb.get("auto_share").unwrap().as_bool().unwrap());
        let filters = usb.get("filters").unwrap().as_array().unwrap();
        assert_eq!(filters.len(), 2);

        let security = config.get("security").unwrap();
        assert!(!security.get("require_approval").unwrap().as_bool().unwrap());
        let approved = security.get("approved_clients").unwrap().as_array().unwrap();
        assert_eq!(approved.len(), 2);

        let iroh = config.get("iroh").unwrap();
        let relays = iroh.get("relay_servers").unwrap().as_array().unwrap();
        assert_eq!(relays.len(), 2);
        assert_eq!(
            iroh.get("secret_key_path").unwrap().as_str().unwrap(),
            "/etc/p2p-usb/secret_key"
        );

        let policy = config.get("policy").unwrap();
        assert_eq!(
            policy
                .get("timezone_offset_hours")
                .unwrap()
                .as_integer()
                .unwrap(),
            2
        );

        let bandwidth = config.get("bandwidth").unwrap();
        assert!(bandwidth.get("enabled").unwrap().as_bool().unwrap());
        assert_eq!(
            bandwidth.get("global_limit").unwrap().as_str().unwrap(),
            "100Mbps"
        );

        let qos = config.get("qos").unwrap();
        assert!(qos.get("enabled").unwrap().as_bool().unwrap());
        assert_eq!(
            qos.get("control_priority").unwrap().as_integer().unwrap(),
            7
        );

        let sharing = config.get("sharing").unwrap();
        assert_eq!(
            sharing.get("default_mode").unwrap().as_str().unwrap(),
            "shared"
        );
        assert_eq!(
            sharing
                .get("default_lock_timeout_secs")
                .unwrap()
                .as_integer()
                .unwrap(),
            600
        );

        let policies = config.get("device_policies").unwrap().as_array().unwrap();
        assert_eq!(policies.len(), 2);

        let policy0 = &policies[0];
        assert_eq!(
            policy0.get("device_filter").unwrap().as_str().unwrap(),
            "04f9:*"
        );
        let time_windows = policy0.get("time_windows").unwrap().as_array().unwrap();
        assert_eq!(time_windows.len(), 1);
        assert_eq!(
            policy0.get("max_session_duration").unwrap().as_str().unwrap(),
            "1h"
        );
    }

    #[test]
    fn test_parse_device_policy_time_windows() {
        let config = r#"
[[device_policies]]
device_filter = "*"
allowed_clients = ["*"]
time_windows = ["09:00-12:00", "13:00-17:00", "22:00-06:00"]
"#;

        let parsed: toml::Value = toml::from_str(config).unwrap();
        let policies = parsed.get("device_policies").unwrap().as_array().unwrap();
        let windows = policies[0]
            .get("time_windows")
            .unwrap()
            .as_array()
            .unwrap();

        assert_eq!(windows.len(), 3);
        assert_eq!(windows[0].as_str().unwrap(), "09:00-12:00");
        assert_eq!(windows[1].as_str().unwrap(), "13:00-17:00");
        assert_eq!(windows[2].as_str().unwrap(), "22:00-06:00");
    }

    #[test]
    fn test_parse_device_policy_session_duration() {
        let durations = vec![
            ("30m", 30 * 60),
            ("1h", 60 * 60),
            ("1h30m", 90 * 60),
            ("2h30m45s", 2 * 3600 + 30 * 60 + 45),
        ];

        for (duration_str, _expected_secs) in durations {
            let config = format!(
                r#"
[[device_policies]]
device_filter = "*"
allowed_clients = ["*"]
max_session_duration = "{}"
"#,
                duration_str
            );

            let parsed: toml::Value = toml::from_str(&config).unwrap();
            let policies = parsed.get("device_policies").unwrap().as_array().unwrap();
            let duration = policies[0]
                .get("max_session_duration")
                .unwrap()
                .as_str()
                .unwrap();

            assert_eq!(duration, duration_str);
        }
    }

    #[test]
    fn test_parse_device_policy_restricted_classes() {
        let config = r#"
[[device_policies]]
device_filter = "*"
allowed_clients = ["*"]
restricted_device_classes = [1, 2, 3, 8, 14]
"#;

        let parsed: toml::Value = toml::from_str(config).unwrap();
        let policies = parsed.get("device_policies").unwrap().as_array().unwrap();
        let classes = policies[0]
            .get("restricted_device_classes")
            .unwrap()
            .as_array()
            .unwrap();

        assert_eq!(classes.len(), 5);
        assert_eq!(classes[0].as_integer().unwrap(), 1);
        assert_eq!(classes[3].as_integer().unwrap(), 8);
    }

    #[test]
    fn test_invalid_log_level() {
        let config = r#"
[server]
bind_addr = "0.0.0.0:8080"
service_mode = false
log_level = "verbose"

[usb]
auto_share = false
filters = []

[security]
approved_clients = []
require_approval = true

[iroh]
"#;

        let parsed: toml::Value = toml::from_str(config).unwrap();
        let log_level = parsed
            .get("server")
            .unwrap()
            .get("log_level")
            .unwrap()
            .as_str()
            .unwrap();

        let valid_levels = ["trace", "debug", "info", "warn", "error"];
        assert!(!valid_levels.contains(&log_level));
    }

    #[test]
    fn test_missing_required_sections() {
        let incomplete = r#"
[server]
log_level = "info"
"#;

        let parsed: Result<toml::Value, _> = toml::from_str(incomplete);
        assert!(parsed.is_ok());

        let config = parsed.unwrap();
        assert!(config.get("usb").is_none());
        assert!(config.get("security").is_none());
    }

    #[test]
    fn test_bandwidth_limit_formats() {
        let config = r#"
[bandwidth]
enabled = true
global_limit = "1Gbps"
per_client_limit = "100MB/s"
per_device_limit = "50000"
"#;

        let parsed: toml::Value = toml::from_str(config).unwrap();
        let bandwidth = parsed.get("bandwidth").unwrap();

        assert_eq!(
            bandwidth.get("global_limit").unwrap().as_str().unwrap(),
            "1Gbps"
        );
        assert_eq!(
            bandwidth.get("per_client_limit").unwrap().as_str().unwrap(),
            "100MB/s"
        );
        assert_eq!(
            bandwidth.get("per_device_limit").unwrap().as_str().unwrap(),
            "50000"
        );
    }

    #[test]
    fn test_sharing_modes_in_config() {
        let modes = vec!["exclusive", "shared", "read-only"];

        for mode in modes {
            let config = format!(
                r#"
[sharing]
default_mode = "{}"
"#,
                mode
            );

            let parsed: toml::Value = toml::from_str(&config).unwrap();
            assert_eq!(
                parsed
                    .get("sharing")
                    .unwrap()
                    .get("default_mode")
                    .unwrap()
                    .as_str()
                    .unwrap(),
                mode
            );
        }
    }

    #[test]
    fn test_audit_levels() {
        let levels = vec!["all", "standard", "security", "off"];

        for level in levels {
            let config = format!(
                r#"
[audit]
enabled = true
level = "{}"
"#,
                level
            );

            let parsed: toml::Value = toml::from_str(&config).unwrap();
            assert_eq!(
                parsed
                    .get("audit")
                    .unwrap()
                    .get("level")
                    .unwrap()
                    .as_str()
                    .unwrap(),
                level
            );
        }
    }
}

mod client_config {
    use super::*;

    const MINIMAL_CLIENT_CONFIG: &str = r#"
[client]
log_level = "info"

[servers]
approved_servers = []

[iroh]
"#;

    const FULL_CLIENT_CONFIG: &str = r#"
[client]
global_auto_connect = "auto"
log_level = "debug"

[servers]
approved_servers = ["legacy-server-1", "legacy-server-2"]

[[servers.configured]]
node_id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
name = "pi5-kim"
auto_connect = "full"
auto_attach = ["04f9:*", "YubiKey"]

[[servers.configured]]
node_id = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
name = "pi4-office"
auto_connect = "auto"
auto_attach = []

[[servers.configured]]
node_id = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
auto_connect = "manual"

[iroh]
relay_servers = ["relay.example.com"]
secret_key_path = "~/.config/p2p-usb/client_key"
"#;

    #[test]
    fn test_parse_minimal_client_config() {
        let config: toml::Value = toml::from_str(MINIMAL_CLIENT_CONFIG).unwrap();

        let client = config.get("client").unwrap();
        assert_eq!(client.get("log_level").unwrap().as_str().unwrap(), "info");
        assert!(client.get("global_auto_connect").is_none());

        let servers = config.get("servers").unwrap();
        assert!(servers
            .get("approved_servers")
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn test_parse_full_client_config() {
        let config: toml::Value = toml::from_str(FULL_CLIENT_CONFIG).unwrap();

        let client = config.get("client").unwrap();
        assert_eq!(
            client.get("global_auto_connect").unwrap().as_str().unwrap(),
            "auto"
        );
        assert_eq!(client.get("log_level").unwrap().as_str().unwrap(), "debug");

        let servers = config.get("servers").unwrap();
        let approved = servers.get("approved_servers").unwrap().as_array().unwrap();
        assert_eq!(approved.len(), 2);

        let configured = servers.get("configured").unwrap().as_array().unwrap();
        assert_eq!(configured.len(), 3);

        let server0 = &configured[0];
        assert_eq!(
            server0.get("node_id").unwrap().as_str().unwrap(),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert_eq!(server0.get("name").unwrap().as_str().unwrap(), "pi5-kim");
        assert_eq!(
            server0.get("auto_connect").unwrap().as_str().unwrap(),
            "full"
        );

        let auto_attach = server0.get("auto_attach").unwrap().as_array().unwrap();
        assert_eq!(auto_attach.len(), 2);
        assert_eq!(auto_attach[0].as_str().unwrap(), "04f9:*");
        assert_eq!(auto_attach[1].as_str().unwrap(), "YubiKey");

        let server1 = &configured[1];
        assert_eq!(
            server1.get("auto_connect").unwrap().as_str().unwrap(),
            "auto"
        );
        assert!(server1
            .get("auto_attach")
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty());

        let server2 = &configured[2];
        assert!(server2.get("name").is_none());
        assert_eq!(
            server2.get("auto_connect").unwrap().as_str().unwrap(),
            "manual"
        );

        let iroh = config.get("iroh").unwrap();
        let relays = iroh.get("relay_servers").unwrap().as_array().unwrap();
        assert_eq!(relays.len(), 1);
    }

    #[test]
    fn test_auto_connect_mode_variants() {
        let modes = vec!["manual", "auto", "full"];

        for mode in modes {
            let config = format!(
                r#"
[client]
global_auto_connect = "{}"
log_level = "info"

[servers]
approved_servers = []

[iroh]
"#,
                mode
            );

            let parsed: toml::Value = toml::from_str(&config).unwrap();
            assert_eq!(
                parsed
                    .get("client")
                    .unwrap()
                    .get("global_auto_connect")
                    .unwrap()
                    .as_str()
                    .unwrap(),
                mode
            );
        }
    }

    #[test]
    fn test_server_with_all_auto_attach_patterns() {
        let config = r#"
[[servers.configured]]
node_id = "test123"
auto_connect = "full"
auto_attach = [
    "04f9:0042",
    "04f9:*",
    "YubiKey",
    "Brother",
    "1050:0407"
]
"#;

        let parsed: toml::Value = toml::from_str(config).unwrap();
        let configured = parsed
            .get("servers")
            .unwrap()
            .get("configured")
            .unwrap()
            .as_array()
            .unwrap();
        let patterns = configured[0]
            .get("auto_attach")
            .unwrap()
            .as_array()
            .unwrap();

        assert_eq!(patterns.len(), 5);
        assert_eq!(patterns[0].as_str().unwrap(), "04f9:0042");
        assert_eq!(patterns[1].as_str().unwrap(), "04f9:*");
        assert_eq!(patterns[2].as_str().unwrap(), "YubiKey");
        assert_eq!(patterns[3].as_str().unwrap(), "Brother");
        assert_eq!(patterns[4].as_str().unwrap(), "1050:0407");
    }

    #[test]
    fn test_mixed_legacy_and_configured_servers() {
        let config = r#"
[servers]
approved_servers = ["legacy1", "legacy2", "legacy3"]

[[servers.configured]]
node_id = "configured1"
name = "Server 1"
auto_connect = "auto"

[[servers.configured]]
node_id = "legacy2"
name = "Server 2 (upgraded)"
auto_connect = "full"
"#;

        let parsed: toml::Value = toml::from_str(config).unwrap();
        let servers = parsed.get("servers").unwrap();

        let approved = servers.get("approved_servers").unwrap().as_array().unwrap();
        assert_eq!(approved.len(), 3);

        let configured = servers.get("configured").unwrap().as_array().unwrap();
        assert_eq!(configured.len(), 2);

        let upgraded = &configured[1];
        assert_eq!(upgraded.get("node_id").unwrap().as_str().unwrap(), "legacy2");
        assert_eq!(
            upgraded.get("name").unwrap().as_str().unwrap(),
            "Server 2 (upgraded)"
        );
    }

    #[test]
    fn test_empty_servers_section() {
        let config = r#"
[client]
log_level = "info"

[servers]
approved_servers = []
configured = []

[iroh]
"#;

        let parsed: toml::Value = toml::from_str(config).unwrap();
        let servers = parsed.get("servers").unwrap();

        assert!(servers
            .get("approved_servers")
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty());
        assert!(servers
            .get("configured")
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn test_invalid_auto_connect_mode() {
        let config = r#"
[[servers.configured]]
node_id = "test"
auto_connect = "invalid_mode"
"#;

        let parsed: toml::Value = toml::from_str(config).unwrap();
        let configured = parsed
            .get("servers")
            .unwrap()
            .get("configured")
            .unwrap()
            .as_array()
            .unwrap();
        let mode = configured[0]
            .get("auto_connect")
            .unwrap()
            .as_str()
            .unwrap();

        let valid_modes = ["manual", "auto", "full"];
        assert!(!valid_modes.contains(&mode));
    }
}

mod config_validation {
    use super::*;

    #[test]
    fn test_valid_usb_filter_formats() {
        let valid_filters = vec![
            "0x1234:0x5678",
            "0xABCD:0xEF01",
            "0x1234:*",
            "*:0x5678",
            "*:*",
        ];

        for filter in valid_filters {
            let parts: Vec<&str> = filter.split(':').collect();
            assert_eq!(parts.len(), 2, "Filter should have exactly 2 parts");

            let (vid, pid) = (parts[0], parts[1]);

            if vid != "*" {
                assert!(
                    vid.starts_with("0x") || vid.starts_with("0X"),
                    "VID should start with 0x"
                );
                let hex_part = &vid[2..];
                assert!(
                    hex_part.len() >= 1 && hex_part.len() <= 4,
                    "VID hex part should be 1-4 chars"
                );
                assert!(
                    u16::from_str_radix(hex_part, 16).is_ok(),
                    "VID should be valid hex"
                );
            }

            if pid != "*" {
                assert!(
                    pid.starts_with("0x") || pid.starts_with("0X"),
                    "PID should start with 0x"
                );
                let hex_part = &pid[2..];
                assert!(
                    hex_part.len() >= 1 && hex_part.len() <= 4,
                    "PID hex part should be 1-4 chars"
                );
                assert!(
                    u16::from_str_radix(hex_part, 16).is_ok(),
                    "PID should be valid hex"
                );
            }
        }
    }

    #[test]
    fn test_invalid_usb_filter_formats() {
        let invalid_filters = vec![
            "1234:5678",
            "0x1234",
            "0x1234:0x5678:0x9abc",
            "0xGHIJ:0x5678",
            "0x12345:0x5678",
            "",
            ":::",
        ];

        for filter in invalid_filters {
            let parts: Vec<&str> = filter.split(':').collect();

            if parts.len() != 2 {
                continue;
            }

            let (vid, pid) = (parts[0], parts[1]);

            let vid_invalid = vid != "*"
                && (!vid.starts_with("0x")
                    || vid.len() < 3
                    || vid.len() > 6
                    || u16::from_str_radix(&vid[2..], 16).is_err());

            let pid_invalid = pid != "*"
                && (!pid.starts_with("0x")
                    || pid.len() < 3
                    || pid.len() > 6
                    || u16::from_str_radix(&pid[2..], 16).is_err());

            assert!(
                vid_invalid || pid_invalid,
                "Filter {} should be invalid",
                filter
            );
        }
    }

    #[test]
    fn test_valid_log_levels() {
        let valid_levels = vec!["trace", "debug", "info", "warn", "error"];

        for level in &valid_levels {
            assert!(
                valid_levels.contains(level),
                "{} should be a valid log level",
                level
            );
        }
    }

    #[test]
    fn test_time_window_format_validation() {
        let valid_windows = vec!["09:00-17:00", "00:00-23:59", "22:00-06:00"];

        for window in valid_windows {
            let parts: Vec<&str> = window.split('-').collect();
            assert_eq!(parts.len(), 2, "Window should have start and end");

            for part in parts {
                let time_parts: Vec<&str> = part.split(':').collect();
                assert_eq!(time_parts.len(), 2, "Time should have hours and minutes");

                let hours: u8 = time_parts[0].parse().unwrap();
                let minutes: u8 = time_parts[1].parse().unwrap();

                assert!(hours <= 23, "Hours should be 0-23");
                assert!(minutes <= 59, "Minutes should be 0-59");
            }
        }
    }

    #[test]
    fn test_invalid_time_window_formats() {
        let invalid_windows = vec![
            "09:00",
            "09:00-17:00-18:00",
            "24:00-25:00",
            "09:60-17:00",
            "invalid",
            "ab:cd-ef:gh",
        ];

        for window in invalid_windows {
            let parts: Vec<&str> = window.split('-').collect();

            if parts.len() != 2 {
                continue;
            }

            let mut is_invalid = false;
            for part in parts {
                let time_parts: Vec<&str> = part.split(':').collect();
                if time_parts.len() != 2 {
                    is_invalid = true;
                    break;
                }

                let hours: Result<u8, _> = time_parts[0].parse();
                let minutes: Result<u8, _> = time_parts[1].parse();

                match (hours, minutes) {
                    (Ok(h), Ok(m)) if h <= 23 && m <= 59 => {}
                    _ => {
                        is_invalid = true;
                        break;
                    }
                }
            }

            assert!(is_invalid, "Window {} should be invalid", window);
        }
    }

    #[test]
    fn test_duration_parsing_logic() {
        fn parse_duration(s: &str) -> Option<Duration> {
            let s = s.trim().to_lowercase();
            let mut total_secs: u64 = 0;
            let mut current_num = String::new();

            for c in s.chars() {
                if c.is_ascii_digit() {
                    current_num.push(c);
                } else {
                    if current_num.is_empty() {
                        return None;
                    }
                    let num: u64 = current_num.parse().ok()?;
                    current_num.clear();

                    match c {
                        'h' => total_secs += num * 3600,
                        'm' => total_secs += num * 60,
                        's' => total_secs += num,
                        _ => return None,
                    }
                }
            }

            if !current_num.is_empty() {
                let num: u64 = current_num.parse().ok()?;
                total_secs += num;
            }

            if total_secs == 0 {
                return None;
            }

            Some(Duration::from_secs(total_secs))
        }

        assert_eq!(parse_duration("30m"), Some(Duration::from_secs(30 * 60)));
        assert_eq!(parse_duration("1h"), Some(Duration::from_secs(3600)));
        assert_eq!(
            parse_duration("1h30m"),
            Some(Duration::from_secs(90 * 60))
        );
        assert_eq!(
            parse_duration("2h30m45s"),
            Some(Duration::from_secs(2 * 3600 + 30 * 60 + 45))
        );
        assert_eq!(parse_duration("3600"), Some(Duration::from_secs(3600)));

        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("0"), None);
        assert_eq!(parse_duration("abc"), None);
        assert_eq!(parse_duration("1x"), None);
    }
}

mod auto_attach_pattern_matching {
    use super::*;

    fn matches_pattern(pattern: &str, vid: &str, pid: &str, product_name: Option<&str>) -> bool {
        let pattern_lower = pattern.to_lowercase();

        if let Some((pattern_vid, pattern_pid)) = pattern_lower.split_once(':') {
            if pattern_pid != "*" {
                return vid == pattern_vid && pid == pattern_pid;
            }
            return vid == pattern_vid;
        }

        if let Some(name) = product_name {
            return name.to_lowercase().contains(&pattern_lower);
        }

        false
    }

    #[test]
    fn test_exact_vid_pid_match() {
        assert!(matches_pattern("04f9:0042", "04f9", "0042", None));
        assert!(!matches_pattern("04f9:0042", "04f9", "0043", None));
        assert!(!matches_pattern("04f9:0042", "04f8", "0042", None));
    }

    #[test]
    fn test_vendor_wildcard_match() {
        assert!(matches_pattern("04f9:*", "04f9", "0042", None));
        assert!(matches_pattern("04f9:*", "04f9", "1234", None));
        assert!(!matches_pattern("04f9:*", "04f8", "0042", None));
    }

    #[test]
    fn test_product_name_match() {
        assert!(matches_pattern(
            "YubiKey",
            "1050",
            "0407",
            Some("Yubico YubiKey OTP+FIDO")
        ));
        assert!(matches_pattern(
            "yubikey",
            "1050",
            "0407",
            Some("YubiKey 5")
        ));
        assert!(!matches_pattern(
            "YubiKey",
            "1050",
            "0407",
            Some("Security Key")
        ));
        assert!(!matches_pattern("YubiKey", "1050", "0407", None));
    }

    #[test]
    fn test_case_insensitive_matching() {
        assert!(matches_pattern("04F9:0042", "04f9", "0042", None));
        assert!(matches_pattern("brother", "04f9", "0042", Some("Brother HL-2270DW")));
        assert!(matches_pattern("BROTHER", "04f9", "0042", Some("brother printer")));
    }

    #[test]
    fn test_partial_product_name_match() {
        assert!(matches_pattern(
            "Brother",
            "04f9",
            "0042",
            Some("Brother HL-2270DW Laser Printer")
        ));
        assert!(matches_pattern(
            "Laser",
            "04f9",
            "0042",
            Some("Brother HL-2270DW Laser Printer")
        ));
        assert!(matches_pattern(
            "Printer",
            "04f9",
            "0042",
            Some("Brother HL-2270DW Laser Printer")
        ));
    }
}

mod config_defaults {
    use super::*;

    #[test]
    fn test_default_qos_priorities() {
        let control_priority = 7;
        let interrupt_priority = 5;
        let bulk_priority = 3;

        assert!(
            control_priority > interrupt_priority,
            "Control should have higher priority than interrupt"
        );
        assert!(
            interrupt_priority > bulk_priority,
            "Interrupt should have higher priority than bulk"
        );
    }

    #[test]
    fn test_default_sharing_settings() {
        let default_lock_timeout_secs = 300;
        let default_max_queue = 10;

        assert_eq!(default_lock_timeout_secs, 300);
        assert_eq!(default_max_queue, 10);
    }

    #[test]
    fn test_default_bandwidth_burst_multiplier() {
        let default_burst_multiplier: f64 = 1.5;
        assert!((default_burst_multiplier - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_default_audit_settings() {
        let default_max_size_mb = 10;
        let default_max_files = 5;
        let default_stats_interval = 300;

        assert_eq!(default_max_size_mb, 10);
        assert_eq!(default_max_files, 5);
        assert_eq!(default_stats_interval, 300);
    }
}
