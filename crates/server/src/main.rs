//! rust-p2p-usb Server
//!
//! USB device sharing server that runs on a Raspberry Pi or other Linux host.
//! Provides USB devices over the network to authorized clients using Iroh P2P networking.

mod config;
mod network;
mod service;
mod tui;
mod usb;

use anyhow::{Context, Result};
use clap::Parser;
use common::{UsbBridge, UsbCommand, create_usb_bridge, setup_logging};
use network::IrohServer;
use tokio::signal;
use tracing::{error, info};
use usb::spawn_usb_worker;

#[derive(Parser, Debug)]
#[command(name = "p2p-usb-server")]
#[command(
    author,
    version,
    about = "P2P USB Server - Share USB devices over the network"
)]
#[command(long_about = "
A high-performance USB device sharing server using Iroh P2P networking.
Enables secure access to USB devices from anywhere on the internet.

EXAMPLES:
    # Run with default config
    p2p-usb-server

    # Run with custom config
    p2p-usb-server --config /path/to/config.toml

    # List USB devices without starting server
    p2p-usb-server --list-devices

    # Run as systemd service (no TUI)
    p2p-usb-server --service

    # Run with debug logging
    p2p-usb-server --log-level debug

CONFIGURATION:
    The server looks for configuration files in the following order:
    1. Path specified with --config
    2. ~/.config/p2p-usb/server.toml
    3. /etc/p2p-usb/server.toml
    4. Built-in defaults

For more information, visit: https://github.com/kimasplund/rust-p2p-usb
")]
struct Args {
    /// Path to configuration file
    #[arg(short, long, value_name = "PATH")]
    config: Option<std::path::PathBuf>,

    /// Save default configuration to default location and exit
    #[arg(long)]
    save_config: bool,

    /// Run as systemd service (no TUI)
    #[arg(long)]
    service: bool,

    /// List USB devices and exit
    #[arg(long)]
    list_devices: bool,

    /// Log level (trace, debug, info, warn, error)
    #[arg(short, long, value_name = "LEVEL")]
    log_level: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Handle --save-config flag early (before loading config)
    if args.save_config {
        let config = config::ServerConfig::default();
        let path = config::ServerConfig::default_path();
        config.save(&path).context("Failed to save configuration")?;
        println!("Configuration saved to: {}", path.display());
        return Ok(());
    }

    // Load configuration first (to get log level from config if not specified)
    let config = if let Some(ref path) = args.config {
        config::ServerConfig::load(Some(path.clone())).context("Failed to load configuration")?
    } else {
        config::ServerConfig::load_or_default()
    };

    // Use CLI log level if specified, otherwise use config value
    let log_level = args
        .log_level
        .as_deref()
        .unwrap_or(&config.server.log_level);

    // Setup logging
    setup_logging(log_level).context("Failed to setup logging")?;

    info!("rust-p2p-usb Server v{}", env!("CARGO_PKG_VERSION"));
    info!("Log level: {}", log_level);

    // Initialize USB subsystem
    let (usb_bridge, worker) = create_usb_bridge();
    // Start USB worker thread (hybrid architecture: sync USB ops in dedicated thread)
    // Pass configured filters to restrict which devices are shared
    let usb_worker_handle = spawn_usb_worker(worker, config.usb.filters.clone());

    if args.list_devices {
        let result = list_devices_mode(usb_bridge.clone()).await;
        // Cleanup: Shutdown USB worker thread
        info!("Shutting down USB subsystem...");
        if let Err(e) = shutdown_usb_worker(usb_bridge).await {
            error!("Error shutting down USB worker: {:#}", e);
        }
        // Wait for USB thread to exit
        if let Err(e) = usb_worker_handle.join() {
            error!("USB worker thread panicked: {:?}", e);
        }
        return result;
    }

    // Determine run mode from args or config
    let service_mode = args.service || config.server.service_mode;

    let result = if service_mode {
        info!("Running in service mode (headless)");
        run_service(config, usb_bridge.clone()).await
    } else {
        info!("Running in TUI mode (interactive)");
        run_tui(config, usb_bridge.clone()).await
    };

    // Cleanup: Shutdown USB worker thread
    info!("Shutting down USB subsystem...");
    if let Err(e) = shutdown_usb_worker(usb_bridge).await {
        error!("Error shutting down USB worker: {:#}", e);
    }

    // Wait for USB thread to exit
    if let Err(e) = usb_worker_handle.join() {
        error!("USB worker thread panicked: {:?}", e);
    }

    result
}

/// List USB devices and exit
async fn list_devices_mode(usb_bridge: UsbBridge) -> Result<()> {
    info!("Listing USB devices...");

    let (tx, rx) = tokio::sync::oneshot::channel();
    usb_bridge
        .send_command(UsbCommand::ListDevices { response: tx })
        .await
        .context("Failed to send ListDevices command")?;

    let devices = rx.await.context("Failed to receive device list")?;

    if devices.is_empty() {
        println!("No USB devices found.");
    } else {
        println!("Found {} USB device(s):\n", devices.len());
        for device in devices {
            println!(
                "  [{}] {:04x}:{:04x} - {} {}",
                device.id.0,
                device.vendor_id,
                device.product_id,
                device
                    .manufacturer
                    .as_deref()
                    .unwrap_or("Unknown Manufacturer"),
                device.product.as_deref().unwrap_or("Unknown Product")
            );
            println!(
                "      Bus {:03} Device {:03} Speed: {:?}",
                device.bus_number, device.device_address, device.speed
            );
            if let Some(serial) = &device.serial_number {
                println!("      Serial: {}", serial);
            }
            println!();
        }
    }

    Ok(())
}

/// Run in service mode (headless, systemd-compatible)
async fn run_service(config: config::ServerConfig, usb_bridge: UsbBridge) -> Result<()> {
    info!("Starting P2P USB Server in service mode");

    if service::is_systemd() {
        info!("Running under systemd");
    }

    // Initialize Iroh server
    let server = IrohServer::new(config.clone(), usb_bridge.clone())
        .await
        .context("Failed to initialize Iroh server")?;

    info!("Server EndpointId: {}", server.endpoint_id());
    info!("Listening on: {:?}", server.local_addrs());

    // Start watchdog task if enabled
    let watchdog_handle = service::spawn_watchdog_task()
        .await
        .context("Failed to spawn watchdog task")?;

    // Notify systemd that we're ready
    service::notify_ready().context("Failed to notify systemd ready")?;
    service::notify_status("Running - waiting for connections")
        .context("Failed to send status to systemd")?;

    info!("Press Ctrl+C to shutdown");

    // Setup Ctrl+C handler
    let server_handle = tokio::spawn(async move {
        if let Err(e) = server.run().await {
            error!("Server error: {:#}", e);
        }
    });

    // Wait for shutdown signal
    match signal::ctrl_c().await {
        Ok(()) => {
            info!("Received Ctrl+C, shutting down gracefully...");
        }
        Err(e) => {
            error!("Error waiting for Ctrl+C: {}", e);
        }
    }

    // Notify systemd we're stopping
    service::notify_stopping().context("Failed to notify systemd stopping")?;

    // Stop watchdog
    watchdog_handle.abort();

    // Abort server task (will drop endpoint and close connections)
    server_handle.abort();

    info!("Server shutdown complete");
    Ok(())
}

/// Run in TUI mode (interactive terminal UI)
async fn run_tui(config: config::ServerConfig, usb_bridge: UsbBridge) -> Result<()> {
    // Initialize Iroh server
    let server = IrohServer::new(config.clone(), usb_bridge.clone())
        .await
        .context("Failed to initialize Iroh server")?;

    let endpoint_id = server.endpoint_id();
    info!("Server EndpointId: {}", endpoint_id);
    info!("Listening on: {:?}", server.local_addrs());

    // Create channel for network events to TUI
    // Note: network_tx will be used by the server to send events when network layer is integrated
    let (_network_tx, network_rx) = tokio::sync::mpsc::unbounded_channel();

    // Spawn server task in background
    // Note: In a full implementation, the server would send NetworkEvents through network_tx
    // when clients connect/disconnect and attach/detach from devices.
    // For now, the channel exists but won't receive events until the network layer is integrated.
    let _server_handle = tokio::spawn(async move {
        if let Err(e) = server.run().await {
            error!("Server error: {:#}", e);
        }
    });

    // Run the TUI (this blocks until user quits)
    let tui_result = tui::run(endpoint_id, usb_bridge, network_rx, config.usb.auto_share).await;

    // Note: Server task will be cleaned up when we return (dropped)
    // The Iroh endpoint's Drop implementation will close connections gracefully

    tui_result
}

/// Shutdown USB worker thread gracefully
async fn shutdown_usb_worker(usb_bridge: UsbBridge) -> Result<()> {
    usb_bridge
        .send_command(UsbCommand::Shutdown)
        .await
        .context("Failed to send Shutdown command")?;
    Ok(())
}
