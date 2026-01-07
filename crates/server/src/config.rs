//! Server configuration management

use crate::audit::AuditLevel;
use anyhow::{Context, Result, anyhow};
use protocol::SharingMode;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub server: ServerSettings,
    pub usb: UsbSettings,
    pub security: SecuritySettings,
    pub iroh: IrohSettings,
    /// Device passthrough policies
    #[serde(default)]
    pub device_policies: Vec<DevicePolicy>,
    /// Audit logging configuration
    #[serde(default)]
    pub audit: AuditConfig,
    /// Bandwidth limiting configuration
    #[serde(default)]
    pub bandwidth: BandwidthSettings,
    /// Quality of Service configuration
    #[serde(default)]
    pub qos: QosSettings,
    /// Device sharing configuration
    #[serde(default)]
    pub sharing: SharingSettings,
}

/// Audit logging configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditConfig {
    /// Enable audit logging
    #[serde(default)]
    pub enabled: bool,
    /// Path to audit log file
    #[serde(default = "AuditConfig::default_path")]
    pub path: PathBuf,
    /// Audit level (all, standard, security, off)
    #[serde(default)]
    pub level: AuditLevel,
    /// Maximum log file size in MB before rotation
    #[serde(default)]
    pub max_size_mb: Option<u32>,
    /// Maximum number of entries before rotation
    #[serde(default)]
    pub max_entries: Option<u64>,
    /// Maximum number of rotated files to keep
    #[serde(default)]
    pub max_files: Option<u32>,
    /// Enable syslog output (in addition to file)
    #[serde(default)]
    pub syslog: bool,
    /// Transfer statistics reporting interval in seconds (0 = disabled)
    #[serde(default = "AuditConfig::default_stats_interval")]
    pub stats_interval_secs: u64,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            path: Self::default_path(),
            level: AuditLevel::default(),
            max_size_mb: Some(10),
            max_entries: None,
            max_files: Some(5),
            syslog: false,
            stats_interval_secs: Self::default_stats_interval(),
        }
    }
}

impl AuditConfig {
    fn default_path() -> PathBuf {
        if let Some(data_dir) = dirs::data_local_dir() {
            data_dir.join("p2p-usb").join("audit.log")
        } else {
            PathBuf::from("/var/log/p2p-usb/audit.log")
        }
    }

    fn default_stats_interval() -> u64 {
        300 // 5 minutes
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerSettings {
    pub bind_addr: Option<String>,
    pub service_mode: bool,
    pub log_level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsbSettings {
    pub auto_share: bool,
    pub filters: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecuritySettings {
    pub approved_clients: Vec<String>,
    pub require_approval: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IrohSettings {
    pub relay_servers: Option<Vec<String>>,
    /// Path to the secret key file for stable EndpointId
    /// If None, uses default XDG path: ~/.config/p2p-usb/secret_key
    #[serde(default)]
    pub secret_key_path: Option<PathBuf>,
}

/// Device passthrough policy for fine-grained sharing control
///
/// Controls access to USB devices based on client identity, time windows,
/// session duration, and device class restrictions.
///
/// # Example Configuration
/// ```toml
/// [[device_policies]]
/// device_filter = "04f9:*"  # Brother devices
/// allowed_clients = ["endpoint1", "endpoint2"]
/// time_windows = ["09:00-17:00"]
/// max_session_duration = "1h"
///
/// [[device_policies]]
/// device_filter = "*"  # Default policy
/// allowed_clients = ["*"]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevicePolicy {
    /// Device filter pattern (VID:PID format, e.g., "04f9:*" or "*" for default)
    /// Can use short form like "04f9:1234" without 0x prefix
    #[serde(alias = "filter")]
    pub device_filter: String,
    /// List of allowed client EndpointIds (empty = all approved clients, "*" = any)
    #[serde(default)]
    pub allowed_clients: Vec<String>,
    /// Human-readable description
    #[serde(default)]
    pub description: Option<String>,
    /// Sharing mode for this device (exclusive, shared, read-only)
    #[serde(default)]
    pub sharing_mode: SharingMode,
    /// Lock timeout in seconds for shared/read-only modes (0 = no timeout)
    #[serde(default = "DevicePolicy::default_lock_timeout")]
    pub lock_timeout_secs: u32,
    /// Maximum clients that can attach simultaneously (for shared mode)
    #[serde(default = "DevicePolicy::default_max_clients")]
    pub max_concurrent_clients: u32,

    // Time-based access control fields
    /// Time windows when access is allowed (e.g., ["09:00-17:00"])
    /// Format: "HH:MM-HH:MM" (24-hour format)
    /// Supports overnight windows like "22:00-06:00"
    /// Empty or None means no time restriction
    #[serde(default)]
    pub time_windows: Option<Vec<String>>,

    /// Maximum session duration (parsed from string like "1h", "30m", "1h30m")
    /// None means no duration limit
    #[serde(default, with = "duration_serde")]
    pub max_session_duration: Option<Duration>,

    /// Device classes that are restricted (denied) for this policy
    /// USB device class codes: 1=Audio, 2=CDC, 3=HID, 6=Image, 7=Printer,
    /// 8=Mass Storage, 9=Hub, 10=CDC-Data, 11=Smart Card, 13=Content Security,
    /// 14=Video, 15=Personal Healthcare, 16=Audio/Video
    /// None means no class restrictions
    #[serde(default)]
    pub restricted_device_classes: Option<Vec<u8>>,
}

impl DevicePolicy {
    fn default_lock_timeout() -> u32 {
        300 // 5 minutes default
    }

    fn default_max_clients() -> u32 {
        4 // Up to 4 clients by default
    }
}

/// Custom serde module for Duration
mod duration_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Option<Duration>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match duration {
            Some(d) => {
                let s = format_duration(*d);
                s.serialize(serializer)
            }
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Duration>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: Option<String> = Option::deserialize(deserializer)?;
        match opt {
            Some(s) => parse_duration(&s)
                .map(Some)
                .map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }

    /// Parse a duration string like "1h", "30m", "1h30m"
    pub fn parse_duration(s: &str) -> Result<Duration, String> {
        let s = s.trim().to_lowercase();
        let mut total_secs: u64 = 0;
        let mut current_num = String::new();

        for c in s.chars() {
            if c.is_ascii_digit() {
                current_num.push(c);
            } else {
                if current_num.is_empty() {
                    return Err(format!("Invalid duration format: {}", s));
                }
                let num: u64 = current_num
                    .parse()
                    .map_err(|_| format!("Invalid number in duration: {}", current_num))?;
                current_num.clear();

                match c {
                    'h' => total_secs += num * 3600,
                    'm' => total_secs += num * 60,
                    's' => total_secs += num,
                    _ => return Err(format!("Invalid duration unit: {}", c)),
                }
            }
        }

        // Handle case where string ends with a number (assume seconds)
        if !current_num.is_empty() {
            let num: u64 = current_num
                .parse()
                .map_err(|_| format!("Invalid number in duration: {}", current_num))?;
            total_secs += num;
        }

        if total_secs == 0 {
            return Err("Duration must be greater than 0".to_string());
        }

        Ok(Duration::from_secs(total_secs))
    }

    fn format_duration(d: Duration) -> String {
        let secs = d.as_secs();
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        let secs = secs % 60;

        let mut result = String::new();
        if hours > 0 {
            result.push_str(&format!("{}h", hours));
        }
        if mins > 0 {
            result.push_str(&format!("{}m", mins));
        }
        if secs > 0 || result.is_empty() {
            result.push_str(&format!("{}s", secs));
        }
        result
    }
}

/// Global sharing configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharingSettings {
    /// Default sharing mode for devices without a specific policy
    #[serde(default)]
    pub default_mode: SharingMode,
    /// Default lock timeout in seconds
    #[serde(default = "SharingSettings::default_lock_timeout")]
    pub default_lock_timeout_secs: u32,
    /// Maximum queue length per device
    #[serde(default = "SharingSettings::default_max_queue")]
    pub max_queue_length: u32,
    /// Enable queue position notifications
    #[serde(default = "SharingSettings::default_notifications")]
    pub queue_notifications: bool,
}

impl Default for SharingSettings {
    fn default() -> Self {
        Self {
            default_mode: SharingMode::Exclusive,
            default_lock_timeout_secs: Self::default_lock_timeout(),
            max_queue_length: Self::default_max_queue(),
            queue_notifications: Self::default_notifications(),
        }
    }
}

impl SharingSettings {
    fn default_lock_timeout() -> u32 {
        300 // 5 minutes
    }

    fn default_max_queue() -> u32 {
        10 // Up to 10 clients in queue
    }

    fn default_notifications() -> bool {
        true
    }
}

/// Bandwidth limiting configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BandwidthSettings {
    /// Enable bandwidth limiting
    #[serde(default)]
    pub enabled: bool,
    /// Global server bandwidth limit (e.g., "100Mbps", "50MB/s", or bytes per second)
    #[serde(default)]
    pub global_limit: Option<String>,
    /// Per-client bandwidth limit (e.g., "50Mbps")
    #[serde(default)]
    pub per_client_limit: Option<String>,
    /// Per-device bandwidth limit (e.g., "25Mbps")
    #[serde(default)]
    pub per_device_limit: Option<String>,
    /// Burst size multiplier (1.0 = no burst, 2.0 = double burst capacity)
    #[serde(default = "BandwidthSettings::default_burst_multiplier")]
    pub burst_multiplier: f64,
}

impl BandwidthSettings {
    fn default_burst_multiplier() -> f64 {
        1.5
    }

    /// Parse a bandwidth string to bytes per second
    pub fn parse_limit(s: &str) -> Option<u64> {
        common::rate_limiter::BandwidthLimit::from_str(s).map(|l| l.bytes_per_second)
    }

    /// Get global limit in bytes per second
    pub fn global_limit_bps(&self) -> Option<u64> {
        self.global_limit.as_ref().and_then(|s| Self::parse_limit(s))
    }

    /// Get per-client limit in bytes per second
    pub fn per_client_limit_bps(&self) -> Option<u64> {
        self.per_client_limit.as_ref().and_then(|s| Self::parse_limit(s))
    }

    /// Get per-device limit in bytes per second
    pub fn per_device_limit_bps(&self) -> Option<u64> {
        self.per_device_limit.as_ref().and_then(|s| Self::parse_limit(s))
    }
}

/// Quality of Service configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QosSettings {
    /// Enable QoS prioritization
    #[serde(default)]
    pub enabled: bool,
    /// Priority for control transfers (0-7, higher = more priority)
    #[serde(default = "QosSettings::default_control_priority")]
    pub control_priority: u8,
    /// Priority for interrupt transfers
    #[serde(default = "QosSettings::default_interrupt_priority")]
    pub interrupt_priority: u8,
    /// Priority for bulk transfers
    #[serde(default = "QosSettings::default_bulk_priority")]
    pub bulk_priority: u8,
    /// Per-client fair quota in Mbps (for fair scheduling between clients)
    #[serde(default = "QosSettings::default_client_quota_mbps")]
    pub client_quota_mbps: u64,
    /// Enable priority aging (boost priority of waiting requests to prevent starvation)
    #[serde(default = "QosSettings::default_priority_aging")]
    pub priority_aging: bool,
}

impl Default for QosSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            control_priority: Self::default_control_priority(),
            interrupt_priority: Self::default_interrupt_priority(),
            bulk_priority: Self::default_bulk_priority(),
            client_quota_mbps: Self::default_client_quota_mbps(),
            priority_aging: Self::default_priority_aging(),
        }
    }
}

impl QosSettings {
    fn default_control_priority() -> u8 {
        7 // Highest priority
    }

    fn default_interrupt_priority() -> u8 {
        5
    }

    fn default_bulk_priority() -> u8 {
        3
    }

    fn default_client_quota_mbps() -> u64 {
        100 // 100 Mbps per client for fair scheduling
    }

    fn default_priority_aging() -> bool {
        true // Enable by default to prevent starvation
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            server: ServerSettings {
                bind_addr: Some("127.0.0.1:8080".to_string()),
                service_mode: false,
                log_level: "info".to_string(),
            },
            usb: UsbSettings {
                auto_share: false,
                filters: Vec::new(),
            },
            security: SecuritySettings {
                approved_clients: Vec::new(),
                require_approval: true,
            },
            iroh: IrohSettings {
                relay_servers: None,
                secret_key_path: None,
            },
            device_policies: Vec::new(),
            audit: AuditConfig::default(),
            bandwidth: BandwidthSettings::default(),
            qos: QosSettings::default(),
            sharing: SharingSettings::default(),
        }
    }
}

impl ServerConfig {
    /// Load configuration from the specified path
    pub fn load(path: Option<PathBuf>) -> Result<Self> {
        let config_path = if let Some(p) = path {
            p
        } else {
            // Try standard locations in order
            let candidates = vec![
                Self::default_path(),
                PathBuf::from("/etc/p2p-usb/server.toml"),
            ];

            candidates
                .into_iter()
                .find(|p| p.exists())
                .ok_or_else(|| anyhow!("No configuration file found, using defaults"))?
        };

        let content = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read config file: {}", config_path.display()))?;

        let config: ServerConfig = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", config_path.display()))?;

        config.validate()?;

        tracing::info!("Loaded configuration from: {}", config_path.display());
        Ok(config)
    }

    /// Load configuration or return defaults if not found
    pub fn load_or_default() -> Self {
        match Self::load(None) {
            Ok(config) => config,
            Err(e) => {
                tracing::warn!("Failed to load config: {}, using defaults", e);
                Self::default()
            }
        }
    }

    /// Save configuration to the specified path
    pub fn save(&self, path: &Path) -> Result<()> {
        let content = toml::to_string_pretty(self).context("Failed to serialize configuration")?;

        // Create parent directories if they don't exist
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create config directory: {}", parent.display())
            })?;
        }

        fs::write(path, content)
            .with_context(|| format!("Failed to write config file: {}", path.display()))?;

        tracing::info!("Saved configuration to: {}", path.display());
        Ok(())
    }

    /// Get the default configuration file path
    pub fn default_path() -> PathBuf {
        if let Some(config_dir) = dirs::config_dir() {
            config_dir.join("rust-p2p-usb").join("server.toml")
        } else {
            PathBuf::from(".config/rust-p2p-usb/server.toml")
        }
    }

    /// Validate configuration values
    fn validate(&self) -> Result<()> {
        // Validate log level
        let valid_levels = ["trace", "debug", "info", "warn", "error"];
        if !valid_levels.contains(&self.server.log_level.as_str()) {
            return Err(anyhow!(
                "Invalid log level '{}', must be one of: {}",
                self.server.log_level,
                valid_levels.join(", ")
            ));
        }

        // Validate USB filters (VID:PID format)
        for filter in &self.usb.filters {
            Self::validate_filter(filter)?;
        }

        // Validate approved client node IDs (basic format check)
        for client_id in &self.security.approved_clients {
            if client_id.is_empty() {
                return Err(anyhow!("Empty client node ID in approved_clients list"));
            }
            // Note: Full NodeId validation would require iroh types, done at runtime
        }

        Ok(())
    }

    /// Validate a USB device filter pattern (VID:PID)
    fn validate_filter(filter: &str) -> Result<()> {
        let parts: Vec<&str> = filter.split(':').collect();
        if parts.len() != 2 {
            return Err(anyhow!(
                "Invalid filter format '{}', expected VID:PID (e.g., '0x1234:0x5678' or '0x1234:*')",
                filter
            ));
        }

        let (vid, pid) = (parts[0], parts[1]);

        // Validate VID
        if vid != "*" {
            Self::validate_hex_id(vid, "VID")?;
        }

        // Validate PID
        if pid != "*" {
            Self::validate_hex_id(pid, "PID")?;
        }

        Ok(())
    }

    /// Validate a hex ID (VID or PID)
    fn validate_hex_id(id: &str, name: &str) -> Result<()> {
        if !id.starts_with("0x") && !id.starts_with("0X") {
            return Err(anyhow!(
                "Invalid {} '{}', must start with '0x' (e.g., '0x1234')",
                name,
                id
            ));
        }

        let hex_part = &id[2..];
        if hex_part.is_empty() || hex_part.len() > 4 {
            return Err(anyhow!(
                "Invalid {} '{}', hex part must be 1-4 digits",
                name,
                id
            ));
        }

        u16::from_str_radix(hex_part, 16)
            .map_err(|_| anyhow!("Invalid {} '{}', not a valid hex number", name, id))?;

        Ok(())
    }
}

/// Legacy load_config function for backward compatibility
#[allow(dead_code)]
pub fn load_config(path: &str) -> Result<ServerConfig> {
    let path_buf = PathBuf::from(shellexpand::tilde(path).as_ref());
    ServerConfig::load(Some(path_buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ServerConfig::default();
        assert_eq!(config.server.log_level, "info");
        assert!(!config.usb.auto_share);
        assert!(config.security.require_approval);
    }

    #[test]
    fn test_validate_filter_valid() {
        assert!(ServerConfig::validate_filter("0x1234:0x5678").is_ok());
        assert!(ServerConfig::validate_filter("0x1234:*").is_ok());
        assert!(ServerConfig::validate_filter("*:0x5678").is_ok());
        assert!(ServerConfig::validate_filter("*:*").is_ok());
        assert!(ServerConfig::validate_filter("0xABCD:0xEF01").is_ok());
    }

    #[test]
    fn test_validate_filter_invalid() {
        assert!(ServerConfig::validate_filter("1234:5678").is_err());
        assert!(ServerConfig::validate_filter("0x1234").is_err());
        assert!(ServerConfig::validate_filter("0x1234:0x5678:0x9abc").is_err());
        assert!(ServerConfig::validate_filter("0xGHIJ:0x5678").is_err());
        assert!(ServerConfig::validate_filter("0x12345:0x5678").is_err());
    }

    #[test]
    fn test_config_serialization() {
        let config = ServerConfig::default();
        let toml_str = toml::to_string(&config).unwrap();
        let parsed: ServerConfig = toml::from_str(&toml_str).unwrap();

        assert_eq!(config.server.log_level, parsed.server.log_level);
        assert_eq!(config.usb.auto_share, parsed.usb.auto_share);
    }

    #[test]
    fn test_validate_log_level() {
        let mut config = ServerConfig::default();
        assert!(config.validate().is_ok());

        config.server.log_level = "invalid".to_string();
        assert!(config.validate().is_err());

        config.server.log_level = "debug".to_string();
        assert!(config.validate().is_ok());
    }
}
