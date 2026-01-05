//! Secret key persistence for stable EndpointIds
//!
//! This module provides functionality to load and save Iroh secret keys,
//! ensuring that server and client EndpointIds remain stable across restarts.
//!
//! Keys are stored in XDG-compliant locations:
//! - Linux: `~/.config/p2p-usb/secret_key`
//! - macOS: `~/Library/Application Support/p2p-usb/secret_key`
//! - Windows: `%APPDATA%\p2p-usb\secret_key`

use anyhow::{Context, Result, bail};
use iroh::SecretKey;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Application name for XDG directory lookup
const APP_NAME: &str = "p2p-usb";

/// Secret key filename
const SECRET_KEY_FILENAME: &str = "secret_key";

/// Length of secret key in bytes (Ed25519)
const SECRET_KEY_LENGTH: usize = 32;

/// Get the default secret key path using XDG conventions
///
/// Returns the platform-specific configuration directory path for the secret key:
/// - Linux: `~/.config/p2p-usb/secret_key`
/// - macOS: `~/Library/Application Support/p2p-usb/secret_key`
/// - Windows: `%APPDATA%\p2p-usb\secret_key`
pub fn default_secret_key_path() -> Result<PathBuf> {
    let config_dir =
        dirs::config_dir().context("Failed to determine config directory (HOME not set?)")?;

    Ok(config_dir.join(APP_NAME).join(SECRET_KEY_FILENAME))
}

/// Load or generate a secret key
///
/// This function provides the main entry point for secret key management:
/// 1. If a key exists at the specified path, it is loaded
/// 2. If no key exists, a new one is generated and saved
///
/// # Arguments
/// * `path` - Optional path to the secret key file. If None, uses the default XDG path.
///
/// # Returns
/// The loaded or newly generated secret key
///
/// # Errors
/// Returns an error if:
/// - The config directory cannot be determined
/// - The key file exists but cannot be read
/// - The key file contains invalid data
/// - A new key cannot be saved (permission denied, disk full, etc.)
///
/// # Example
/// ```no_run
/// use common::keys::load_or_generate_secret_key;
///
/// let secret_key = load_or_generate_secret_key(None)?;
/// println!("EndpointId: {}", secret_key.public());
/// # Ok::<(), anyhow::Error>(())
/// ```
pub fn load_or_generate_secret_key(path: Option<&Path>) -> Result<SecretKey> {
    let key_path = match path {
        Some(p) => p.to_path_buf(),
        None => default_secret_key_path()?,
    };

    if key_path.exists() {
        load_secret_key(&key_path)
    } else {
        let key = generate_secret_key();
        save_secret_key(&key, &key_path)?;
        info!("Generated new secret key at {}", key_path.display());
        Ok(key)
    }
}

/// Load a secret key from file
///
/// # Arguments
/// * `path` - Path to the secret key file
///
/// # Returns
/// The loaded secret key
///
/// # Errors
/// Returns an error if:
/// - The file cannot be opened or read
/// - The file contains invalid data (wrong length, corrupted)
pub fn load_secret_key(path: &Path) -> Result<SecretKey> {
    debug!("Loading secret key from {}", path.display());

    let mut file = File::open(path)
        .with_context(|| format!("Failed to open secret key file: {}", path.display()))?;

    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .with_context(|| format!("Failed to read secret key file: {}", path.display()))?;

    // Validate key length
    if bytes.len() != SECRET_KEY_LENGTH {
        bail!(
            "Invalid secret key file: expected {} bytes, got {} bytes",
            SECRET_KEY_LENGTH,
            bytes.len()
        );
    }

    // Convert to fixed-size array
    let key_bytes: [u8; SECRET_KEY_LENGTH] = bytes.try_into().expect("Length already validated");

    let key = SecretKey::from_bytes(&key_bytes);

    info!(
        "Loaded secret key from {} (EndpointId: {})",
        path.display(),
        key.public()
    );

    Ok(key)
}

/// Generate a new random secret key
///
/// Uses the operating system's cryptographically secure random number generator.
pub fn generate_secret_key() -> SecretKey {
    SecretKey::generate(&mut rand::rng())
}

/// Save a secret key to file with secure permissions
///
/// Creates the parent directory if it doesn't exist.
/// On Unix systems, sets file permissions to 0600 (owner read/write only).
///
/// # Arguments
/// * `key` - The secret key to save
/// * `path` - Path where the key should be saved
///
/// # Errors
/// Returns an error if:
/// - The parent directory cannot be created
/// - The file cannot be written
/// - Permissions cannot be set (Unix only, non-fatal warning)
pub fn save_secret_key(key: &SecretKey, path: &Path) -> Result<()> {
    debug!("Saving secret key to {}", path.display());

    // Create parent directory if it doesn't exist
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config directory: {}", parent.display()))?;
    }

    // Write key bytes to file
    let key_bytes = key.to_bytes();
    let mut file = File::create(path)
        .with_context(|| format!("Failed to create secret key file: {}", path.display()))?;

    file.write_all(&key_bytes)
        .with_context(|| format!("Failed to write secret key file: {}", path.display()))?;

    file.sync_all()
        .with_context(|| format!("Failed to sync secret key file: {}", path.display()))?;

    // Set secure permissions (Unix only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = fs::Permissions::from_mode(0o600);
        if let Err(e) = fs::set_permissions(path, permissions) {
            warn!(
                "Failed to set secure permissions on {}: {}",
                path.display(),
                e
            );
        } else {
            debug!("Set permissions 0600 on {}", path.display());
        }
    }

    info!("Saved secret key to {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_generate_secret_key() {
        let key1 = generate_secret_key();
        let key2 = generate_secret_key();

        // Keys should be different
        assert_ne!(key1.to_bytes(), key2.to_bytes());

        // Public keys should also be different
        assert_ne!(key1.public(), key2.public());
    }

    #[test]
    fn test_save_and_load_secret_key() {
        let temp_dir = TempDir::new().unwrap();
        let key_path = temp_dir.path().join("test_key");

        // Generate and save a key
        let original_key = generate_secret_key();
        save_secret_key(&original_key, &key_path).unwrap();

        // Verify file exists
        assert!(key_path.exists());

        // Load the key
        let loaded_key = load_secret_key(&key_path).unwrap();

        // Keys should match
        assert_eq!(original_key.to_bytes(), loaded_key.to_bytes());
        assert_eq!(original_key.public(), loaded_key.public());
    }

    #[test]
    fn test_load_or_generate_creates_new_key() {
        let temp_dir = TempDir::new().unwrap();
        let key_path = temp_dir.path().join("new_key");

        // Key shouldn't exist yet
        assert!(!key_path.exists());

        // Load or generate should create a new key
        let key = load_or_generate_secret_key(Some(&key_path)).unwrap();

        // Key file should now exist
        assert!(key_path.exists());

        // Loading again should return the same key
        let loaded_key = load_or_generate_secret_key(Some(&key_path)).unwrap();
        assert_eq!(key.to_bytes(), loaded_key.to_bytes());
    }

    #[test]
    fn test_load_invalid_key_file() {
        let temp_dir = TempDir::new().unwrap();
        let key_path = temp_dir.path().join("invalid_key");

        // Write invalid data (wrong length)
        fs::write(&key_path, b"too short").unwrap();

        // Loading should fail
        let result = load_secret_key(&key_path);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Invalid secret key file")
        );
    }

    #[test]
    fn test_load_nonexistent_key_file() {
        let temp_dir = TempDir::new().unwrap();
        let key_path = temp_dir.path().join("nonexistent");

        let result = load_secret_key(&key_path);
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn test_key_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = TempDir::new().unwrap();
        let key_path = temp_dir.path().join("secure_key");

        let key = generate_secret_key();
        save_secret_key(&key, &key_path).unwrap();

        let metadata = fs::metadata(&key_path).unwrap();
        let mode = metadata.permissions().mode();

        // Check that only owner has read/write permissions (0600)
        // The mode includes file type bits, so we mask with 0o777
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn test_save_creates_parent_directories() {
        let temp_dir = TempDir::new().unwrap();
        let key_path = temp_dir.path().join("nested").join("dirs").join("key");

        let key = generate_secret_key();
        save_secret_key(&key, &key_path).unwrap();

        assert!(key_path.exists());
    }

    #[test]
    fn test_default_secret_key_path() {
        // This should not panic and should return a valid path
        let result = default_secret_key_path();
        assert!(result.is_ok());

        let path = result.unwrap();
        assert!(path.to_string_lossy().contains(APP_NAME));
        assert!(path.to_string_lossy().contains(SECRET_KEY_FILENAME));
    }

    #[test]
    fn test_endpoint_id_stability() {
        let temp_dir = TempDir::new().unwrap();
        let key_path = temp_dir.path().join("stable_key");

        // First run: generate key
        let key1 = load_or_generate_secret_key(Some(&key_path)).unwrap();
        let endpoint_id1 = key1.public();

        // Simulate restart: load key again
        let key2 = load_or_generate_secret_key(Some(&key_path)).unwrap();
        let endpoint_id2 = key2.public();

        // EndpointId should be stable across "restarts"
        assert_eq!(endpoint_id1, endpoint_id2);
    }
}
