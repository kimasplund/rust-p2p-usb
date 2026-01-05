//! rust-p2p-usb Client
//!
//! USB device sharing client that connects to remote servers and creates
//! virtual USB devices for remote access.

mod config;
mod network;
mod tui;
mod virtual_usb;

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use common::setup_logging;
use iroh::PublicKey as EndpointId;
use network::{ClientConfig as NetworkClientConfig, IrohClient};
use std::sync::Arc;
use tokio::signal;
use tracing::{error, info, warn};
use virtual_usb::VirtualUsbManager;

#[derive(Parser, Debug)]
#[command(name = "p2p-usb-client")]
#[command(author, version, about = "P2P USB Client - Access remote USB devices")]
#[command(long_about = "
A high-performance USB device sharing client using Iroh P2P networking.
Connect to remote USB servers and access USB devices as if they were local.

EXAMPLES:
    # Run with default config (interactive TUI)
    p2p-usb-client

    # Connect to specific server immediately
    p2p-usb-client --connect <server-node-id>

    # Run with custom config
    p2p-usb-client --config /path/to/config.toml

    # Run with debug logging
    p2p-usb-client --log-level debug

CONFIGURATION:
    The client looks for configuration files in the following order:
    1. Path specified with --config
    2. ~/.config/p2p-usb/client.toml
    3. /etc/p2p-usb/client.toml
    4. Built-in defaults

For more information, visit: https://github.com/yourusername/rust-p2p-usb
")]
struct Args {
    /// Path to configuration file
    #[arg(short, long, value_name = "PATH")]
    config: Option<std::path::PathBuf>,

    /// Save default configuration to default location and exit
    #[arg(long)]
    save_config: bool,

    /// Connect to specific server by node ID
    #[arg(long, value_name = "NODE_ID")]
    connect: Option<String>,

    /// Log level (trace, debug, info, warn, error)
    #[arg(short, long, value_name = "LEVEL")]
    log_level: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Handle --save-config flag early (before loading config)
    if args.save_config {
        let config = config::ClientConfig::default();
        let path = config::ClientConfig::default_path();
        config.save(&path).context("Failed to save configuration")?;
        println!("Configuration saved to: {}", path.display());
        return Ok(());
    }

    // Load configuration first (to get log level from config if not specified)
    let config = if let Some(ref path) = args.config {
        config::ClientConfig::load(Some(path.clone())).context("Failed to load configuration")?
    } else {
        config::ClientConfig::load_or_default()
    };

    // Use CLI log level if specified, otherwise use config value
    let log_level = args
        .log_level
        .as_deref()
        .unwrap_or(&config.client.log_level);

    // Setup logging
    setup_logging(log_level).context("Failed to setup logging")?;

    info!("rust-p2p-usb Client v{}", env!("CARGO_PKG_VERSION"));
    info!("Log level: {}", log_level);

    // Initialize Iroh client
    let client = Arc::new(
        create_iroh_client(&config)
            .await
            .context("Failed to initialize Iroh client")?,
    );

    info!("Client EndpointId: {}", client.endpoint_id());

    // Initialize Virtual USB Manager
    let virtual_usb = Arc::new(
        VirtualUsbManager::new()
            .await
            .context("Failed to initialize Virtual USB Manager")?,
    );
    info!("Virtual USB Manager initialized");

    // Handle specific connection request or run TUI
    let result = if let Some(server_id_str) = args.connect {
        connect_and_run(client, virtual_usb.clone(), &server_id_str, &config).await
    } else {
        run_tui_mode(client, virtual_usb.clone(), &config).await
    };

    info!("Client shutting down...");
    result
}

/// Create Iroh client with configuration
async fn create_iroh_client(config: &config::ClientConfig) -> Result<IrohClient> {
    // Parse approved servers from config
    let mut allowed_servers = std::collections::HashSet::new();
    for server_str in &config.servers.approved_servers {
        if !server_str.is_empty() {
            match server_str.parse::<EndpointId>() {
                Ok(endpoint_id) => {
                    allowed_servers.insert(endpoint_id);
                }
                Err(e) => {
                    warn!("Failed to parse server EndpointId '{}': {}", server_str, e);
                }
            }
        }
    }

    let network_config = NetworkClientConfig {
        allowed_servers,
        alpn: common::ALPN_PROTOCOL.to_vec(),
        secret_key_path: config.iroh.secret_key_path.clone(),
    };

    IrohClient::new(network_config).await
}

/// Connect to specific server and run in connected mode
async fn connect_and_run(
    client: Arc<IrohClient>,
    virtual_usb: Arc<VirtualUsbManager>,
    server_id_str: &str,
    config: &config::ClientConfig,
) -> Result<()> {
    info!("Connecting to server: {}", server_id_str);

    // Parse server EndpointId
    let server_id = server_id_str
        .parse::<EndpointId>()
        .context("Invalid server EndpointId format")?;

    // Connect to server
    client
        .connect_to_server(server_id, None)
        .await
        .context("Failed to connect to server")?;

    info!("Successfully connected to server");

    // List available devices and attach them as virtual USB devices
    match client.list_remote_devices(server_id).await {
        Ok(devices) => {
            if devices.is_empty() {
                info!("No devices available on server");
            } else {
                info!("Available devices on server:");
                for device in &devices {
                    info!(
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

                    // Create device proxy and attach as virtual USB device
                    match IrohClient::create_device_proxy(client.clone(), server_id, device.clone())
                        .await
                    {
                        Ok(device_proxy) => match virtual_usb.attach_device(device_proxy).await {
                            Ok(handle) => {
                                info!("  ✓ Attached as virtual USB device (handle: {})", handle.0);
                            }
                            Err(e) => {
                                warn!("  ✗ Failed to attach virtual device: {:#}", e);
                            }
                        },
                        Err(e) => {
                            warn!("  ✗ Failed to create device proxy: {:#}", e);
                        }
                    }
                }
            }
        }
        Err(e) => {
            error!("Failed to list devices: {:#}", e);
        }
    }

    // If auto-connect is enabled, fall back to TUI
    if config.client.auto_connect {
        info!("Auto-connect enabled, running in interactive mode");
        warn!("TUI not yet implemented, staying connected in headless mode");
        wait_for_shutdown().await?;
    } else {
        info!("Connected successfully. Use TUI mode for device management.");
    }

    // Cleanup: detach all virtual USB devices
    info!("Detaching virtual USB devices...");
    let attached_devices = virtual_usb.list_devices().await;
    for device_handle in attached_devices {
        if let Err(e) = virtual_usb.detach_device(device_handle).await {
            warn!("Failed to detach device {}: {:#}", device_handle.0, e);
        }
    }

    // Disconnect gracefully
    if let Err(e) = client.disconnect_from_server(server_id).await {
        error!("Error disconnecting from server: {:#}", e);
    }

    Ok(())
}

/// Run in TUI mode (interactive)
async fn run_tui_mode(
    client: Arc<IrohClient>,
    virtual_usb: Arc<VirtualUsbManager>,
    config: &config::ClientConfig,
) -> Result<()> {
    info!("Starting TUI mode");

    // Run the TUI - it handles all the cleanup internally
    tui::run(client, virtual_usb, config).await
}

/// Wait for Ctrl+C signal
async fn wait_for_shutdown() -> Result<()> {
    match signal::ctrl_c().await {
        Ok(()) => {
            info!("Received Ctrl+C, shutting down...");
            Ok(())
        }
        Err(e) => Err(anyhow!("Error waiting for Ctrl+C: {}", e)),
    }
}
