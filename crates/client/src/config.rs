//! Client configuration management

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientConfig {
    pub client: ClientSettings,
    pub servers: ServersSettings,
    pub iroh: IrohSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientSettings {
    pub auto_connect: bool,
    pub log_level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServersSettings {
    pub approved_servers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IrohSettings {
    pub relay_servers: Option<Vec<String>>,
    /// Path to the secret key file for stable EndpointId
    /// If None, uses default XDG path: ~/.config/p2p-usb/secret_key
    #[serde(default)]
    pub secret_key_path: Option<PathBuf>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            client: ClientSettings {
                auto_connect: true,
                log_level: "info".to_string(),
            },
            servers: ServersSettings {
                approved_servers: Vec::new(),
            },
            iroh: IrohSettings {
                relay_servers: None,
                secret_key_path: None,
            },
        }
    }
}

impl ClientConfig {
    /// Load configuration from the specified path
    pub fn load(path: Option<PathBuf>) -> Result<Self> {
        let config_path = if let Some(p) = path {
            p
        } else {
            // Try standard locations in order
            let candidates = vec![
                Self::default_path(),
                PathBuf::from("/etc/p2p-usb/client.toml"),
            ];

            candidates
                .into_iter()
                .find(|p| p.exists())
                .ok_or_else(|| anyhow!("No configuration file found, using defaults"))?
        };

        let content = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read config file: {}", config_path.display()))?;

        let config: ClientConfig = toml::from_str(&content)
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
            config_dir.join("rust-p2p-usb").join("client.toml")
        } else {
            PathBuf::from(".config/rust-p2p-usb/client.toml")
        }
    }

    /// Validate configuration values
    fn validate(&self) -> Result<()> {
        // Validate log level
        let valid_levels = ["trace", "debug", "info", "warn", "error"];
        if !valid_levels.contains(&self.client.log_level.as_str()) {
            return Err(anyhow!(
                "Invalid log level '{}', must be one of: {}",
                self.client.log_level,
                valid_levels.join(", ")
            ));
        }

        // Validate approved server node IDs (basic format check)
        for server_id in &self.servers.approved_servers {
            if server_id.is_empty() {
                return Err(anyhow!("Empty server node ID in approved_servers list"));
            }
            // Note: Full NodeId validation would require iroh types, done at runtime
        }

        Ok(())
    }
}

/// Legacy load_config function for backward compatibility
#[allow(dead_code)]
pub fn load_config(path: &str) -> Result<ClientConfig> {
    let path_buf = PathBuf::from(shellexpand::tilde(path).as_ref());
    ClientConfig::load(Some(path_buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ClientConfig::default();
        assert_eq!(config.client.log_level, "info");
        assert!(config.client.auto_connect);
        assert!(config.servers.approved_servers.is_empty());
    }

    #[test]
    fn test_config_serialization() {
        let config = ClientConfig::default();
        let toml_str = toml::to_string(&config).unwrap();
        let parsed: ClientConfig = toml::from_str(&toml_str).unwrap();

        assert_eq!(config.client.log_level, parsed.client.log_level);
        assert_eq!(config.client.auto_connect, parsed.client.auto_connect);
    }

    #[test]
    fn test_validate_log_level() {
        let mut config = ClientConfig::default();
        assert!(config.validate().is_ok());

        config.client.log_level = "invalid".to_string();
        assert!(config.validate().is_err());

        config.client.log_level = "trace".to_string();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_empty_server_id() {
        let mut config = ClientConfig::default();
        config.servers.approved_servers.push("".to_string());
        assert!(config.validate().is_err());
    }
}
