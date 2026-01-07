//! Audit logging and compliance features
//!
//! Provides structured JSON audit logging for security and compliance purposes.
//! Logs client connections, device operations, authentication events, and
//! transfer statistics to a rotatable audit log file.

#![allow(dead_code)]

use crate::config::AuditConfig;
use anyhow::{Context, Result};
use protocol::{DeviceHandle, DeviceId, DeviceInfo};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, error, warn};

/// Minimum log level for audit events
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AuditLevel {
    /// Log everything including transfer statistics
    All,
    /// Log connections, devices, auth, and config changes (default)
    #[default]
    Standard,
    /// Log only security-relevant events (auth failures, config changes)
    Security,
    /// Disable audit logging
    Off,
}

impl AuditLevel {
    /// Check if an event type should be logged at this level
    fn should_log(&self, event_type: &AuditEventType) -> bool {
        match self {
            AuditLevel::Off => false,
            AuditLevel::Security => matches!(
                event_type,
                AuditEventType::AuthenticationFailure | AuditEventType::ConfigurationChange
            ),
            AuditLevel::Standard => !matches!(event_type, AuditEventType::TransferStatistics),
            AuditLevel::All => true,
        }
    }
}

/// Types of audit events
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditEventType {
    /// Client connected to server
    ClientConnected,
    /// Client disconnected from server
    ClientDisconnected,
    /// Device attach request
    DeviceAttach,
    /// Device detach request
    DeviceDetach,
    /// Authentication failure
    AuthenticationFailure,
    /// Configuration change
    ConfigurationChange,
    /// Periodic transfer statistics
    TransferStatistics,
    /// Server started
    ServerStarted,
    /// Server stopped
    ServerStopped,
    /// Device hotplug (arrived)
    DeviceArrived,
    /// Device hotplug (removed)
    DeviceRemoved,
}

/// Result of an operation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditResult {
    /// Operation succeeded
    Success,
    /// Operation failed
    Failure,
    /// Operation was denied
    Denied,
}

/// Details for different audit event types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AuditDetails {
    /// Connection details
    Connection {
        #[serde(skip_serializing_if = "Option::is_none")]
        remote_addr: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Device operation details
    Device {
        device_id: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        handle: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        vendor_id: Option<u16>,
        #[serde(skip_serializing_if = "Option::is_none")]
        product_id: Option<u16>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Authentication details
    Auth {
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Configuration change details
    Config {
        setting: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        old_value: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        new_value: Option<String>,
    },
    /// Transfer statistics
    Statistics {
        control_transfers: u64,
        bulk_transfers: u64,
        interrupt_transfers: u64,
        bytes_in: u64,
        bytes_out: u64,
        errors: u64,
        period_seconds: u64,
    },
    /// Server lifecycle
    Server {
        #[serde(skip_serializing_if = "Option::is_none")]
        version: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Device hotplug details
    Hotplug {
        device_id: u32,
        vendor_id: u16,
        product_id: u16,
        #[serde(skip_serializing_if = "Option::is_none")]
        product_name: Option<String>,
    },
    /// Simple message
    Message { message: String },
}

/// A structured audit log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// ISO 8601 timestamp
    pub timestamp: String,
    /// Type of audit event
    pub event_type: AuditEventType,
    /// Client EndpointId (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_id: Option<String>,
    /// Device ID (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<u32>,
    /// Result of the operation
    pub result: AuditResult,
    /// Additional details
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<AuditDetails>,
}

impl AuditEntry {
    /// Create a new audit entry with the current timestamp
    pub fn new(event_type: AuditEventType, result: AuditResult) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| {
                // Format as ISO 8601
                let secs = d.as_secs();
                let datetime = time_to_iso8601(secs);
                datetime
            })
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());

        Self {
            timestamp,
            event_type,
            endpoint_id: None,
            device_id: None,
            result,
            details: None,
        }
    }

    /// Set the endpoint ID
    pub fn with_endpoint_id(mut self, endpoint_id: impl Into<String>) -> Self {
        self.endpoint_id = Some(endpoint_id.into());
        self
    }

    /// Set the device ID
    pub fn with_device_id(mut self, device_id: DeviceId) -> Self {
        self.device_id = Some(device_id.0);
        self
    }

    /// Set the device ID from a raw u32
    pub fn with_device_id_raw(mut self, device_id: u32) -> Self {
        self.device_id = Some(device_id);
        self
    }

    /// Set the details
    pub fn with_details(mut self, details: AuditDetails) -> Self {
        self.details = Some(details);
        self
    }
}

/// Convert Unix timestamp to ISO 8601 format
fn time_to_iso8601(secs: u64) -> String {
    // Simple implementation without external crate
    // This gives us a basic ISO 8601 format
    const SECONDS_PER_DAY: u64 = 86400;
    const SECONDS_PER_HOUR: u64 = 3600;
    const SECONDS_PER_MINUTE: u64 = 60;

    let days = secs / SECONDS_PER_DAY;
    let remaining = secs % SECONDS_PER_DAY;
    let hours = remaining / SECONDS_PER_HOUR;
    let remaining = remaining % SECONDS_PER_HOUR;
    let minutes = remaining / SECONDS_PER_MINUTE;
    let seconds = remaining % SECONDS_PER_MINUTE;

    // Calculate year, month, day from days since epoch (1970-01-01)
    let (year, month, day) = days_to_ymd(days);

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hours, minutes, seconds
    )
}

/// Convert days since Unix epoch to year, month, day
fn days_to_ymd(days: u64) -> (u32, u32, u32) {
    let mut remaining_days = days as i64;
    let mut year = 1970i32;

    // Advance by years
    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }

    // Find month and day
    let is_leap = is_leap_year(year);
    let days_in_months: [i64; 12] = if is_leap {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1u32;
    for &days_in_month in &days_in_months {
        if remaining_days < days_in_month {
            break;
        }
        remaining_days -= days_in_month;
        month += 1;
    }

    let day = (remaining_days + 1) as u32;

    (year as u32, month, day)
}

/// Check if a year is a leap year
fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// Message sent to the audit logger
enum AuditMessage {
    /// Log an entry
    Log(AuditEntry),
    /// Rotate the log file
    Rotate,
    /// Shutdown the logger
    Shutdown,
}

/// Async audit logger that writes to a file in the background
pub struct AuditLogger {
    /// Channel to send log entries
    sender: mpsc::UnboundedSender<AuditMessage>,
    /// Configuration
    config: AuditConfig,
}

impl AuditLogger {
    /// Create a new audit logger
    ///
    /// Returns None if audit logging is disabled
    pub fn new(config: AuditConfig) -> Option<Self> {
        if !config.enabled {
            return None;
        }

        let (sender, receiver) = mpsc::unbounded_channel();
        let writer = AuditWriter::new(config.clone());

        // Spawn background task to handle writing
        tokio::spawn(async move {
            writer.run(receiver).await;
        });

        Some(Self { sender, config })
    }

    /// Log an audit entry
    pub fn log(&self, entry: AuditEntry) {
        // Check if this event type should be logged
        if !self.config.level.should_log(&entry.event_type) {
            return;
        }

        if let Err(e) = self.sender.send(AuditMessage::Log(entry)) {
            warn!("Failed to send audit log entry: {}", e);
        }
    }

    /// Request log rotation
    pub fn rotate(&self) {
        if let Err(e) = self.sender.send(AuditMessage::Rotate) {
            warn!("Failed to send rotate request: {}", e);
        }
    }

    /// Shutdown the audit logger gracefully
    pub fn shutdown(&self) {
        let _ = self.sender.send(AuditMessage::Shutdown);
    }

    /// Log a client connection event
    pub fn log_client_connected(&self, endpoint_id: &str, remote_addr: Option<String>) {
        let entry = AuditEntry::new(AuditEventType::ClientConnected, AuditResult::Success)
            .with_endpoint_id(endpoint_id)
            .with_details(AuditDetails::Connection {
                remote_addr,
                reason: None,
            });
        self.log(entry);
    }

    /// Log a client disconnection event
    pub fn log_client_disconnected(&self, endpoint_id: &str, reason: Option<String>) {
        let entry = AuditEntry::new(AuditEventType::ClientDisconnected, AuditResult::Success)
            .with_endpoint_id(endpoint_id)
            .with_details(AuditDetails::Connection {
                remote_addr: None,
                reason,
            });
        self.log(entry);
    }

    /// Log an authentication failure
    pub fn log_auth_failure(&self, endpoint_id: &str, reason: &str) {
        let entry = AuditEntry::new(AuditEventType::AuthenticationFailure, AuditResult::Denied)
            .with_endpoint_id(endpoint_id)
            .with_details(AuditDetails::Auth {
                reason: Some(reason.to_string()),
            });
        self.log(entry);
    }

    /// Log a device attach request
    pub fn log_device_attach(
        &self,
        endpoint_id: &str,
        device_id: DeviceId,
        handle: Option<DeviceHandle>,
        device_info: Option<&DeviceInfo>,
        result: AuditResult,
        error: Option<String>,
    ) {
        let entry = AuditEntry::new(AuditEventType::DeviceAttach, result)
            .with_endpoint_id(endpoint_id)
            .with_device_id(device_id)
            .with_details(AuditDetails::Device {
                device_id: device_id.0,
                handle: handle.map(|h| h.0),
                vendor_id: device_info.map(|d| d.vendor_id),
                product_id: device_info.map(|d| d.product_id),
                error,
            });
        self.log(entry);
    }

    /// Log a device detach request
    pub fn log_device_detach(
        &self,
        endpoint_id: &str,
        handle: DeviceHandle,
        device_id: Option<DeviceId>,
        result: AuditResult,
        error: Option<String>,
    ) {
        let mut entry = AuditEntry::new(AuditEventType::DeviceDetach, result)
            .with_endpoint_id(endpoint_id)
            .with_details(AuditDetails::Device {
                device_id: device_id.map(|d| d.0).unwrap_or(0),
                handle: Some(handle.0),
                vendor_id: None,
                product_id: None,
                error,
            });

        if let Some(id) = device_id {
            entry = entry.with_device_id(id);
        }

        self.log(entry);
    }

    /// Log a device hotplug arrival
    pub fn log_device_arrived(&self, device_info: &DeviceInfo) {
        let entry = AuditEntry::new(AuditEventType::DeviceArrived, AuditResult::Success)
            .with_device_id(device_info.id)
            .with_details(AuditDetails::Hotplug {
                device_id: device_info.id.0,
                vendor_id: device_info.vendor_id,
                product_id: device_info.product_id,
                product_name: device_info.product.clone(),
            });
        self.log(entry);
    }

    /// Log a device hotplug removal
    pub fn log_device_removed(&self, device_id: DeviceId) {
        let entry = AuditEntry::new(AuditEventType::DeviceRemoved, AuditResult::Success)
            .with_device_id(device_id)
            .with_details(AuditDetails::Hotplug {
                device_id: device_id.0,
                vendor_id: 0,
                product_id: 0,
                product_name: None,
            });
        self.log(entry);
    }

    /// Log a configuration change
    pub fn log_config_change(
        &self,
        setting: &str,
        old_value: Option<String>,
        new_value: Option<String>,
    ) {
        let entry = AuditEntry::new(AuditEventType::ConfigurationChange, AuditResult::Success)
            .with_details(AuditDetails::Config {
                setting: setting.to_string(),
                old_value,
                new_value,
            });
        self.log(entry);
    }

    /// Log server start
    pub fn log_server_started(&self, version: &str) {
        let entry = AuditEntry::new(AuditEventType::ServerStarted, AuditResult::Success)
            .with_details(AuditDetails::Server {
                version: Some(version.to_string()),
                reason: None,
            });
        self.log(entry);
    }

    /// Log server stop
    pub fn log_server_stopped(&self, reason: Option<String>) {
        let entry = AuditEntry::new(AuditEventType::ServerStopped, AuditResult::Success)
            .with_details(AuditDetails::Server {
                version: None,
                reason,
            });
        self.log(entry);
    }

    /// Log transfer statistics
    pub fn log_transfer_statistics(&self, stats: TransferStatistics) {
        let entry = AuditEntry::new(AuditEventType::TransferStatistics, AuditResult::Success)
            .with_details(AuditDetails::Statistics {
                control_transfers: stats.control_transfers,
                bulk_transfers: stats.bulk_transfers,
                interrupt_transfers: stats.interrupt_transfers,
                bytes_in: stats.bytes_in,
                bytes_out: stats.bytes_out,
                errors: stats.errors,
                period_seconds: stats.period_seconds,
            });
        self.log(entry);
    }
}

/// Transfer statistics for periodic logging
#[derive(Debug, Clone, Default)]
pub struct TransferStatistics {
    pub control_transfers: u64,
    pub bulk_transfers: u64,
    pub interrupt_transfers: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub errors: u64,
    pub period_seconds: u64,
}

/// Background writer for audit log entries
struct AuditWriter {
    config: AuditConfig,
    file: Option<BufWriter<File>>,
    entries_written: u64,
    current_file_size: u64,
}

impl AuditWriter {
    fn new(config: AuditConfig) -> Self {
        Self {
            config,
            file: None,
            entries_written: 0,
            current_file_size: 0,
        }
    }

    /// Open or reopen the audit log file
    fn open_file(&mut self) -> Result<()> {
        let path = &self.config.path;

        // Create parent directory if it doesn't exist
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create audit log directory: {:?}", parent))?;
        }

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("Failed to open audit log: {:?}", path))?;

        self.current_file_size = file.metadata().map(|m| m.len()).unwrap_or(0);
        self.file = Some(BufWriter::new(file));
        self.entries_written = 0;

        debug!("Opened audit log: {:?}", path);
        Ok(())
    }

    /// Write an entry to the log file
    fn write_entry(&mut self, entry: &AuditEntry) -> Result<()> {
        if self.file.is_none() {
            self.open_file()?;
        }

        let json = serde_json::to_string(entry).context("Failed to serialize audit entry")?;
        let line = format!("{}\n", json);
        let line_bytes = line.as_bytes();

        if let Some(ref mut writer) = self.file {
            writer
                .write_all(line_bytes)
                .context("Failed to write audit entry")?;
            writer.flush().context("Failed to flush audit log")?;

            self.entries_written += 1;
            self.current_file_size += line_bytes.len() as u64;

            // Check if rotation is needed
            if self.should_rotate() {
                self.rotate()?;
            }
        }

        Ok(())
    }

    /// Check if log rotation is needed
    fn should_rotate(&self) -> bool {
        if let Some(max_size) = self.config.max_size_mb {
            let max_bytes = max_size as u64 * 1024 * 1024;
            if self.current_file_size >= max_bytes {
                return true;
            }
        }

        if let Some(max_entries) = self.config.max_entries {
            if self.entries_written >= max_entries {
                return true;
            }
        }

        false
    }

    /// Rotate the log file
    fn rotate(&mut self) -> Result<()> {
        // Close current file
        self.file = None;

        let path = &self.config.path;
        let max_files = self.config.max_files.unwrap_or(5);

        // Rotate existing files
        for i in (1..max_files).rev() {
            let old_path = Self::rotated_path(path, i);
            let new_path = Self::rotated_path(path, i + 1);

            if old_path.exists() {
                if i + 1 >= max_files {
                    // Delete oldest file
                    std::fs::remove_file(&old_path).ok();
                } else {
                    std::fs::rename(&old_path, &new_path).ok();
                }
            }
        }

        // Rename current file to .1
        if path.exists() {
            let rotated = Self::rotated_path(path, 1);
            std::fs::rename(path, &rotated).ok();
        }

        debug!("Rotated audit log: {:?}", path);

        // Open new file
        self.open_file()?;

        Ok(())
    }

    /// Get the path for a rotated log file
    fn rotated_path(base: &PathBuf, index: u32) -> PathBuf {
        let file_name = base
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("audit.log");

        let new_name = format!("{}.{}", file_name, index);

        base.with_file_name(new_name)
    }

    /// Run the writer loop
    async fn run(mut self, mut receiver: mpsc::UnboundedReceiver<AuditMessage>) {
        while let Some(message) = receiver.recv().await {
            match message {
                AuditMessage::Log(entry) => {
                    if let Err(e) = self.write_entry(&entry) {
                        error!("Failed to write audit log entry: {:#}", e);
                    }
                }
                AuditMessage::Rotate => {
                    if let Err(e) = self.rotate() {
                        error!("Failed to rotate audit log: {:#}", e);
                    }
                }
                AuditMessage::Shutdown => {
                    debug!("Audit logger shutting down");
                    break;
                }
            }
        }

        // Ensure final flush
        if let Some(ref mut writer) = self.file {
            let _ = writer.flush();
        }
    }
}

/// Shared audit logger handle
pub type SharedAuditLogger = Arc<Option<AuditLogger>>;

/// Create a shared audit logger from configuration
pub fn create_audit_logger(config: AuditConfig) -> SharedAuditLogger {
    Arc::new(AuditLogger::new(config))
}

/// Statistics collector for periodic transfer statistics logging
pub struct StatisticsCollector {
    /// Current statistics
    stats: Mutex<TransferStatistics>,
    /// Last report time
    last_report: Mutex<std::time::Instant>,
    /// Report interval
    interval: Duration,
    /// Audit logger
    logger: SharedAuditLogger,
}

impl StatisticsCollector {
    /// Create a new statistics collector
    pub fn new(logger: SharedAuditLogger, interval_secs: u64) -> Self {
        Self {
            stats: Mutex::new(TransferStatistics::default()),
            last_report: Mutex::new(std::time::Instant::now()),
            interval: Duration::from_secs(interval_secs),
            logger,
        }
    }

    /// Record a transfer
    pub async fn record_transfer(
        &self,
        transfer_type: TransferType,
        bytes_in: u64,
        bytes_out: u64,
        is_error: bool,
    ) {
        let mut stats = self.stats.lock().await;

        match transfer_type {
            TransferType::Control => stats.control_transfers += 1,
            TransferType::Bulk => stats.bulk_transfers += 1,
            TransferType::Interrupt => stats.interrupt_transfers += 1,
        }

        stats.bytes_in += bytes_in;
        stats.bytes_out += bytes_out;

        if is_error {
            stats.errors += 1;
        }

        // Check if we should report
        let mut last_report = self.last_report.lock().await;
        if last_report.elapsed() >= self.interval {
            stats.period_seconds = self.interval.as_secs();

            if let Some(ref logger) = *self.logger {
                logger.log_transfer_statistics(stats.clone());
            }

            // Reset statistics
            *stats = TransferStatistics::default();
            *last_report = std::time::Instant::now();
        }
    }
}

/// Transfer type for statistics
#[derive(Debug, Clone, Copy)]
pub enum TransferType {
    Control,
    Bulk,
    Interrupt,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_entry_creation() {
        let entry = AuditEntry::new(AuditEventType::ClientConnected, AuditResult::Success)
            .with_endpoint_id("test_endpoint");

        assert!(entry.timestamp.contains("T"));
        assert!(entry.endpoint_id.is_some());
        assert_eq!(entry.endpoint_id.unwrap(), "test_endpoint");
    }

    #[test]
    fn test_audit_level_filtering() {
        assert!(AuditLevel::All.should_log(&AuditEventType::TransferStatistics));
        assert!(!AuditLevel::Standard.should_log(&AuditEventType::TransferStatistics));
        assert!(AuditLevel::Standard.should_log(&AuditEventType::ClientConnected));
        assert!(!AuditLevel::Security.should_log(&AuditEventType::ClientConnected));
        assert!(AuditLevel::Security.should_log(&AuditEventType::AuthenticationFailure));
        assert!(!AuditLevel::Off.should_log(&AuditEventType::AuthenticationFailure));
    }

    #[test]
    fn test_time_to_iso8601() {
        // Test epoch
        assert_eq!(time_to_iso8601(0), "1970-01-01T00:00:00Z");

        // Test a known date (2024-01-01 00:00:00 UTC = 1704067200)
        assert_eq!(time_to_iso8601(1704067200), "2024-01-01T00:00:00Z");
    }

    #[test]
    fn test_days_to_ymd() {
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
        assert_eq!(days_to_ymd(365), (1971, 1, 1)); // Non-leap year
        assert_eq!(days_to_ymd(366), (1971, 1, 2));
    }

    #[test]
    fn test_is_leap_year() {
        assert!(!is_leap_year(1970));
        assert!(!is_leap_year(1900)); // Divisible by 100 but not 400
        assert!(is_leap_year(2000)); // Divisible by 400
        assert!(is_leap_year(2024)); // Divisible by 4
        assert!(!is_leap_year(2023));
    }

    #[test]
    fn test_rotated_path() {
        let base = PathBuf::from("/var/log/audit.log");
        assert_eq!(
            AuditWriter::rotated_path(&base, 1),
            PathBuf::from("/var/log/audit.log.1")
        );
        assert_eq!(
            AuditWriter::rotated_path(&base, 5),
            PathBuf::from("/var/log/audit.log.5")
        );
    }

    #[test]
    fn test_audit_entry_serialization() {
        let entry = AuditEntry::new(AuditEventType::DeviceAttach, AuditResult::Success)
            .with_endpoint_id("abc123")
            .with_device_id_raw(42)
            .with_details(AuditDetails::Device {
                device_id: 42,
                handle: Some(1),
                vendor_id: Some(0x1234),
                product_id: Some(0x5678),
                error: None,
            });

        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("device_attach"));
        assert!(json.contains("abc123"));
        assert!(json.contains("42"));
    }
}
