//! Server configuration management

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub server: ServerSettings,
    pub usb: UsbSettings,
    pub security: SecuritySettings,
    pub iroh: IrohSettings,
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
