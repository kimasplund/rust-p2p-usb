//! Systemd service integration
//!
//! This module provides integration with systemd's sd-notify protocol,
//! enabling proper service lifecycle management, watchdog support, and
//! status notifications.

use anyhow::{Context, Result};
use std::env;
use std::os::unix::net::UnixDatagram;
use tracing::{debug, error, info};

/// Notify systemd that the service is ready
///
/// This should be called once the service has completed initialization
/// and is ready to accept connections. Only has effect when running
/// under systemd with Type=notify.
pub fn notify_ready() -> Result<()> {
    if let Ok(socket_path) = env::var("NOTIFY_SOCKET") {
        let socket = UnixDatagram::unbound().context("Failed to create Unix socket")?;
        socket
            .send_to(b"READY=1", &socket_path)
            .context("Failed to send READY notification to systemd")?;
        info!("Notified systemd: service ready");
        Ok(())
    } else {
        debug!("NOTIFY_SOCKET not set, skipping systemd notification");
        Ok(())
    }
}

/// Notify systemd that the service is stopping
///
/// This should be called when the service begins its shutdown sequence.
/// Helps systemd track service lifecycle accurately.
pub fn notify_stopping() -> Result<()> {
    if let Ok(socket_path) = env::var("NOTIFY_SOCKET") {
        let socket = UnixDatagram::unbound().context("Failed to create Unix socket")?;
        socket
            .send_to(b"STOPPING=1", &socket_path)
            .context("Failed to send STOPPING notification to systemd")?;
        info!("Notified systemd: service stopping");
        Ok(())
    } else {
        debug!("NOTIFY_SOCKET not set, skipping systemd notification");
        Ok(())
    }
}

/// Notify systemd that the service is reloading configuration
///
/// Use this when implementing configuration reload functionality.
pub fn notify_reloading() -> Result<()> {
    if let Ok(socket_path) = env::var("NOTIFY_SOCKET") {
        let socket = UnixDatagram::unbound().context("Failed to create Unix socket")?;
        socket
            .send_to(b"RELOADING=1", &socket_path)
            .context("Failed to send RELOADING notification to systemd")?;
        info!("Notified systemd: service reloading");
        Ok(())
    } else {
        debug!("NOTIFY_SOCKET not set, skipping systemd notification");
        Ok(())
    }
}

/// Send watchdog keepalive to systemd
///
/// This should be called periodically (at least every WatchdogSec/2 interval)
/// when systemd watchdog is enabled. If the service fails to send keepalives,
/// systemd will restart it.
pub fn notify_watchdog() -> Result<()> {
    if let Ok(socket_path) = env::var("NOTIFY_SOCKET") {
        let socket = UnixDatagram::unbound().context("Failed to create Unix socket")?;
        socket
            .send_to(b"WATCHDOG=1", &socket_path)
            .context("Failed to send WATCHDOG notification to systemd")?;
        debug!("Notified systemd: watchdog keepalive");
        Ok(())
    } else {
        // Silently skip if not running under systemd with watchdog
        Ok(())
    }
}

/// Send a custom status message to systemd
///
/// The status will be visible in `systemctl status` output.
pub fn notify_status(status: &str) -> Result<()> {
    if let Ok(socket_path) = env::var("NOTIFY_SOCKET") {
        let socket = UnixDatagram::unbound().context("Failed to create Unix socket")?;
        let message = format!("STATUS={}", status);
        socket
            .send_to(message.as_bytes(), &socket_path)
            .context("Failed to send STATUS notification to systemd")?;
        debug!("Notified systemd: status = {}", status);
        Ok(())
    } else {
        debug!("NOTIFY_SOCKET not set, skipping systemd notification");
        Ok(())
    }
}

/// Get the watchdog timeout configured by systemd (in microseconds)
///
/// Returns None if watchdog is not enabled or not running under systemd.
pub fn get_watchdog_timeout() -> Option<u64> {
    env::var("WATCHDOG_USEC").ok().and_then(|s| s.parse().ok())
}

/// Check if running under systemd
pub fn is_systemd() -> bool {
    env::var("NOTIFY_SOCKET").is_ok()
}

/// Watchdog task that sends periodic keepalives to systemd
///
/// Spawns a background tokio task that sends WATCHDOG=1 notifications
/// at half the configured watchdog interval. Returns immediately if
/// watchdog is not enabled.
///
/// # Example
///
/// ```no_run
/// # use anyhow::Result;
/// # async fn example() -> Result<()> {
/// let watchdog_handle = server::service::spawn_watchdog_task().await?;
/// // Server runs...
/// watchdog_handle.abort(); // Stop watchdog when shutting down
/// # Ok(())
/// # }
/// ```
pub async fn spawn_watchdog_task() -> Result<tokio::task::JoinHandle<()>> {
    if let Some(timeout_usec) = get_watchdog_timeout() {
        let interval_secs = (timeout_usec / 1_000_000) / 2; // Half of watchdog timeout
        let interval = std::time::Duration::from_secs(interval_secs.max(1));

        info!(
            "Systemd watchdog enabled, interval: {}s (timeout: {}s)",
            interval.as_secs(),
            timeout_usec / 1_000_000
        );

        let handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                ticker.tick().await;
                if let Err(e) = notify_watchdog() {
                    error!("Failed to send watchdog keepalive: {:#}", e);
                    // Continue trying despite errors
                }
            }
        });

        Ok(handle)
    } else {
        debug!("Systemd watchdog not enabled, skipping watchdog task");
        // Return a completed task that does nothing
        Ok(tokio::spawn(async {}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_systemd_without_socket() {
        // When NOTIFY_SOCKET is not set, should return false
        unsafe {
            env::remove_var("NOTIFY_SOCKET");
        }
        assert!(!is_systemd());
    }

    #[test]
    fn test_notify_functions_without_socket() {
        // When NOTIFY_SOCKET is not set, functions should succeed but do nothing
        unsafe {
            env::remove_var("NOTIFY_SOCKET");
        }

        assert!(notify_ready().is_ok());
        assert!(notify_stopping().is_ok());
        assert!(notify_watchdog().is_ok());
        assert!(notify_status("test").is_ok());
    }

    #[test]
    fn test_get_watchdog_timeout() {
        // Without WATCHDOG_USEC, should return None
        unsafe {
            env::remove_var("WATCHDOG_USEC");
        }
        assert!(get_watchdog_timeout().is_none());

        // With valid timeout, should parse it
        unsafe {
            env::set_var("WATCHDOG_USEC", "30000000");
        }
        assert_eq!(get_watchdog_timeout(), Some(30_000_000));

        // With invalid timeout, should return None
        unsafe {
            env::set_var("WATCHDOG_USEC", "invalid");
        }
        assert!(get_watchdog_timeout().is_none());

        // Cleanup
        unsafe {
            env::remove_var("WATCHDOG_USEC");
        }
    }
}
