//! Client configuration management

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Auto-connect mode for servers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AutoConnectMode {
    /// User must manually connect to the server
    #[default]
    Manual,
    /// Auto-connect to server on startup, but don't auto-attach devices
    Auto,
    /// Auto-connect to server and auto-attach all shared devices
    #[serde(rename = "full")]
    AutoWithDevices,
}

/// Individual server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Iroh NodeId of the server
    pub node_id: String,
    /// Display name (e.g., "pi5-kim")
    #[serde(default)]
    pub name: Option<String>,
    /// Auto-connect behavior for this server
    #[serde(default)]
    pub auto_connect: AutoConnectMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientConfig {
    pub client: ClientSettings,
    pub servers: ServersSettings,
    pub iroh: IrohSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientSettings {
    /// Global auto-connect override (if set, applies to all servers)
    #[serde(default)]
    pub global_auto_connect: Option<AutoConnectMode>,
    pub log_level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServersSettings {
    /// Legacy: Simple list of approved server node IDs (for backward compatibility)
    #[serde(default)]
    pub approved_servers: Vec<String>,
    /// New: Detailed server configurations with names and auto-connect modes
    #[serde(default)]
    pub configured: Vec<ServerConfig>,
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
                global_auto_connect: None,
                log_level: "info".to_string(),
            },
            servers: ServersSettings {
                approved_servers: Vec::new(),
                configured: Vec::new(),
            },
            iroh: IrohSettings {
                relay_servers: None,
                secret_key_path: None,
            },
        }
    }
}

impl ClientConfig {
    /// Get all server configurations (merging legacy approved_servers with configured)
    ///
    /// Legacy servers from `approved_servers` are converted to ServerConfig with
    /// Manual auto-connect mode. Servers in `configured` take precedence.
    pub fn all_servers(&self) -> Vec<ServerConfig> {
        let mut servers: Vec<ServerConfig> = self.servers.configured.clone();
        let configured_ids: std::collections::HashSet<String> =
            self.servers.configured.iter().map(|s| s.node_id.clone()).collect();

        // Add legacy servers that aren't already in configured
        for node_id in &self.servers.approved_servers {
            if !configured_ids.contains(node_id) {
                servers.push(ServerConfig {
                    node_id: node_id.clone(),
                    name: None,
                    auto_connect: AutoConnectMode::Manual,
                });
            }
        }

        servers
    }

    /// Find server configuration by node ID
    pub fn find_server(&self, node_id: &str) -> Option<&ServerConfig> {
        self.servers.configured.iter().find(|s| s.node_id == node_id)
    }

    /// Get the effective auto-connect mode for a server
    pub fn effective_auto_connect(&self, server: &ServerConfig) -> AutoConnectMode {
        // Global override takes precedence if set
        self.client.global_auto_connect.unwrap_or(server.auto_connect)
    }

    /// Get servers that should auto-connect on startup
    pub fn auto_connect_servers(&self) -> Vec<&ServerConfig> {
        self.servers.configured.iter()
            .filter(|s| {
                let mode = self.effective_auto_connect(s);
                matches!(mode, AutoConnectMode::Auto | AutoConnectMode::AutoWithDevices)
            })
            .collect()
    }

    /// Get display name for a server (falls back to truncated node ID)
    pub fn server_display_name<'a>(&'a self, node_id: &str) -> String {
        self.find_server(node_id)
            .and_then(|s| s.name.as_ref())
            .map(|n| n.to_string())
            .unwrap_or_else(|| {
                // Truncate node ID for display
                if node_id.len() > 12 {
                    format!("{}...", &node_id[..12])
                } else {
                    node_id.to_string()
                }
            })
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
        assert!(config.client.global_auto_connect.is_none());
        assert!(config.servers.approved_servers.is_empty());
        assert!(config.servers.configured.is_empty());
    }

    #[test]
    fn test_config_serialization() {
        let config = ClientConfig::default();
        let toml_str = toml::to_string(&config).unwrap();
        let parsed: ClientConfig = toml::from_str(&toml_str).unwrap();

        assert_eq!(config.client.log_level, parsed.client.log_level);
        assert_eq!(config.client.global_auto_connect, parsed.client.global_auto_connect);
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

    #[test]
    fn test_all_servers_merges_legacy_and_configured() {
        let mut config = ClientConfig::default();

        // Add legacy server
        config.servers.approved_servers.push("legacy-server-id".to_string());

        // Add configured server
        config.servers.configured.push(ServerConfig {
            node_id: "configured-server-id".to_string(),
            name: Some("pi5-kim".to_string()),
            auto_connect: AutoConnectMode::Auto,
        });

        let all = config.all_servers();
        assert_eq!(all.len(), 2);

        // Check configured server preserves settings
        let configured = all.iter().find(|s| s.node_id == "configured-server-id").unwrap();
        assert_eq!(configured.name, Some("pi5-kim".to_string()));
        assert_eq!(configured.auto_connect, AutoConnectMode::Auto);

        // Check legacy server gets default settings
        let legacy = all.iter().find(|s| s.node_id == "legacy-server-id").unwrap();
        assert!(legacy.name.is_none());
        assert_eq!(legacy.auto_connect, AutoConnectMode::Manual);
    }

    #[test]
    fn test_configured_server_takes_precedence() {
        let mut config = ClientConfig::default();

        // Same ID in both legacy and configured
        config.servers.approved_servers.push("same-id".to_string());
        config.servers.configured.push(ServerConfig {
            node_id: "same-id".to_string(),
            name: Some("Named Server".to_string()),
            auto_connect: AutoConnectMode::AutoWithDevices,
        });

        let all = config.all_servers();
        assert_eq!(all.len(), 1); // No duplicate
        assert_eq!(all[0].name, Some("Named Server".to_string()));
    }

    #[test]
    fn test_global_auto_connect_override() {
        let mut config = ClientConfig::default();
        config.servers.configured.push(ServerConfig {
            node_id: "server1".to_string(),
            name: None,
            auto_connect: AutoConnectMode::Manual,
        });

        // Without global override, uses per-server setting
        assert_eq!(config.effective_auto_connect(&config.servers.configured[0]), AutoConnectMode::Manual);

        // With global override, overrides per-server setting
        config.client.global_auto_connect = Some(AutoConnectMode::AutoWithDevices);
        assert_eq!(config.effective_auto_connect(&config.servers.configured[0]), AutoConnectMode::AutoWithDevices);
    }

    #[test]
    fn test_auto_connect_mode_serialization() {
        // Test that modes serialize correctly in a TOML context
        let mut config = ClientConfig::default();
        config.servers.configured.push(ServerConfig {
            node_id: "test".to_string(),
            name: None,
            auto_connect: AutoConnectMode::Auto,
        });

        let toml_str = toml::to_string(&config).unwrap();
        assert!(toml_str.contains("auto_connect = \"auto\""));

        config.servers.configured[0].auto_connect = AutoConnectMode::AutoWithDevices;
        let toml_str = toml::to_string(&config).unwrap();
        assert!(toml_str.contains("auto_connect = \"full\""));

        config.servers.configured[0].auto_connect = AutoConnectMode::Manual;
        let toml_str = toml::to_string(&config).unwrap();
        assert!(toml_str.contains("auto_connect = \"manual\""));
    }

    #[test]
    fn test_server_display_name() {
        let mut config = ClientConfig::default();

        // Server with name
        config.servers.configured.push(ServerConfig {
            node_id: "abcdefghijklmnopqrstuvwxyz123456".to_string(),
            name: Some("pi5-kim".to_string()),
            auto_connect: AutoConnectMode::Manual,
        });

        assert_eq!(config.server_display_name("abcdefghijklmnopqrstuvwxyz123456"), "pi5-kim");

        // Unknown server with long ID gets truncated
        assert_eq!(config.server_display_name("unknownserver123456789"), "unknownserve...");

        // Short unknown server ID shown in full
        assert_eq!(config.server_display_name("short"), "short");
    }
}
