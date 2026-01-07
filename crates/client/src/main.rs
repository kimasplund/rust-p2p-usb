//! rust-p2p-usb Client
//!
//! USB device sharing client that connects to remote servers and creates
//! virtual USB devices for remote access.

mod config;
mod network;
mod tui;
mod virtual_usb;

use anyhow::{Context, Result};
use clap::Parser;
use common::setup_logging;
use iroh::PublicKey as EndpointId;
use network::{
    ClientConfig as NetworkClientConfig, DeviceNotification, IrohClient, ReconciliationResult,
};
use protocol::DeviceId;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::signal;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
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

For more information, visit: https://github.com/kimasplund/rust-p2p-usb
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

    /// Run in headless mode (no TUI, stay connected until Ctrl+C)
    #[arg(long)]
    headless: bool,
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

    // Set up reconciliation callback for handling reconnection
    setup_reconciliation_callback(&client, virtual_usb.clone()).await;

    // Handle specific connection request or run TUI
    let result = if let Some(server_id_str) = args.connect {
        connect_and_run(
            client,
            virtual_usb.clone(),
            &server_id_str,
            &config,
            args.headless,
        )
        .await
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

/// Resolve a server identifier to an EndpointId
///
/// Accepts either:
/// - A full EndpointId hex string (64 chars)
/// - A server name from config (e.g., "pi5-kim")
///
/// This allows users to connect with friendly names:
///   `--connect pi5-kim` instead of `--connect e8f5a338d37c...`
fn resolve_server_id(server_str: &str, config: &config::ClientConfig) -> Result<EndpointId> {
    // First, try parsing as EndpointId directly
    if let Ok(endpoint_id) = server_str.parse::<EndpointId>() {
        return Ok(endpoint_id);
    }

    debug!(
        "Looking up server '{}' in {} configured servers",
        server_str,
        config.servers.configured.len()
    );

    // Look up by name in configured servers
    for server in &config.servers.configured {
        if let Some(name) = &server.name {
            if name.eq_ignore_ascii_case(server_str) {
                return server.node_id.parse::<EndpointId>().context(format!(
                    "Server '{}' has invalid node_id in config: {}",
                    name, server.node_id
                ));
            }
        }
    }

    // Not found - provide helpful error
    let configured_names: Vec<&str> = config
        .servers
        .configured
        .iter()
        .filter_map(|s| s.name.as_deref())
        .collect();

    if configured_names.is_empty() {
        anyhow::bail!(
            "Unknown server '{}'. No named servers in config. \
            Use full EndpointId or add servers to config with [servers.configured]",
            server_str
        );
    } else {
        anyhow::bail!(
            "Unknown server '{}'. Available: {}. \
            Or use full EndpointId.",
            server_str,
            configured_names.join(", ")
        );
    }
}

/// Connect to specific server and run in connected mode
async fn connect_and_run(
    client: Arc<IrohClient>,
    virtual_usb: Arc<VirtualUsbManager>,
    server_id_str: &str,
    config: &config::ClientConfig,
    headless: bool,
) -> Result<()> {
    // Resolve server name or EndpointId
    let server_id = resolve_server_id(server_id_str, config)?;
    let display_name = config.server_display_name(&server_id.to_string());
    info!("Connecting to server: {} ({})", display_name, server_id);

    // Connect to server
    client
        .connect_to_server(server_id, None)
        .await
        .context("Failed to connect to server")?;

    info!("Successfully connected to server");

    // Track previously attached devices for auto-reattach
    let previously_attached: Arc<RwLock<HashSet<DeviceId>>> = Arc::new(RwLock::new(HashSet::new()));

    // Get server config for auto-attach filtering
    let server_config = config.find_server(&server_id.to_string());
    let effective_mode = server_config
        .map(|s| config.effective_auto_connect(s))
        .unwrap_or(config::AutoConnectMode::Manual);

    // List available devices and attach them as virtual USB devices
    match client.list_remote_devices(server_id).await {
        Ok(devices) => {
            if devices.is_empty() {
                info!("No devices available on server");
            } else {
                info!("Available devices on server:");
                for device in &devices {
                    let product_name = device.product.as_deref();

                    // Check if this device should be auto-attached
                    let should_attach = server_config
                        .map(|s| {
                            s.should_auto_attach(device.vendor_id, device.product_id, product_name)
                        })
                        .unwrap_or(matches!(
                            effective_mode,
                            config::AutoConnectMode::AutoWithDevices
                        ));

                    let status_prefix = if should_attach { "[auto]" } else { "[skip]" };
                    info!(
                        "  {} {:04x}:{:04x} - {} {}",
                        status_prefix,
                        device.vendor_id,
                        device.product_id,
                        device
                            .manufacturer
                            .as_deref()
                            .unwrap_or("Unknown Manufacturer"),
                        product_name.unwrap_or("Unknown Product")
                    );

                    if !should_attach {
                        continue;
                    }

                    // Create device proxy and attach as virtual USB device
                    match IrohClient::create_device_proxy(client.clone(), server_id, device.clone())
                        .await
                    {
                        Ok(device_proxy) => match virtual_usb.attach_device(device_proxy).await {
                            Ok(global_id) => {
                                info!("  [ok] Attached as virtual USB device ({})", global_id);
                                // Track this device for auto-reattach
                                previously_attached.write().await.insert(device.id);
                            }
                            Err(e) => {
                                warn!("  [error] Failed to attach virtual device: {:#}", e);
                            }
                        },
                        Err(e) => {
                            warn!("  [error] Failed to create device proxy: {:#}", e);
                        }
                    }
                }
            }
        }
        Err(e) => {
            error!("Failed to list devices: {:#}", e);
        }
    }

    // Subscribe to device notifications from server
    if let Some(notification_rx) = client.subscribe_notifications(server_id).await {
        let virtual_usb_clone = virtual_usb.clone();
        let client_clone = client.clone();
        let previously_attached_clone = previously_attached.clone();
        let server_config_clone = server_config.cloned();
        tokio::spawn(handle_notifications(
            notification_rx,
            virtual_usb_clone,
            client_clone,
            server_id,
            previously_attached_clone,
            server_config_clone,
        ));
        info!("Subscribed to device notifications from server");
    } else {
        warn!("Failed to subscribe to device notifications (connection may be closed)");
    }

    // If headless mode, wait for Ctrl+C; otherwise launch TUI
    if headless {
        info!("Running in headless mode. Press Ctrl+C to shutdown.");
        signal::ctrl_c()
            .await
            .context("Failed to wait for Ctrl+C")?;
        info!("Received Ctrl+C, shutting down...");
    } else {
        // Check if TUI mode should be launched
        let launch_tui = config
            .client
            .global_auto_connect
            .map(|mode| {
                matches!(
                    mode,
                    config::AutoConnectMode::Auto | config::AutoConnectMode::AutoWithDevices
                )
            })
            .unwrap_or(true);

        if launch_tui {
            info!("Launching TUI for interactive management");
            // Run TUI - it handles cleanup internally
            return tui::run(client, virtual_usb, config).await;
        } else {
            info!("Connected successfully. Use TUI mode for device management.");
        }
    }

    // Cleanup: detach all virtual USB devices
    info!("Detaching virtual USB devices...");
    let attached_devices = virtual_usb.list_devices().await;
    for global_id in attached_devices {
        if let Err(e) = virtual_usb.detach_device(global_id).await {
            warn!("Failed to detach device {}: {:#}", global_id, e);
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

/// Handle device notifications from server
async fn handle_notifications(
    mut notification_rx: tokio::sync::broadcast::Receiver<DeviceNotification>,
    virtual_usb: Arc<VirtualUsbManager>,
    client: Arc<IrohClient>,
    server_id: EndpointId,
    previously_attached: Arc<RwLock<HashSet<DeviceId>>>,
    server_config: Option<config::ServerConfig>,
) {
    loop {
        match notification_rx.recv().await {
            Ok(DeviceNotification::DeviceArrived { device }) => {
                info!(
                    "Device arrived on server: {:?} ({:04x}:{:04x})",
                    device.id, device.vendor_id, device.product_id
                );

                // Check if this device was previously attached
                let was_attached = previously_attached.read().await.contains(&device.id);

                // Check if this device matches auto_attach filter
                let matches_filter = server_config
                    .as_ref()
                    .map(|s| {
                        s.should_auto_attach(
                            device.vendor_id,
                            device.product_id,
                            device.product.as_deref(),
                        )
                    })
                    .unwrap_or(false);

                // Auto-attach if previously attached OR matches auto_attach filter
                if was_attached || matches_filter {
                    let reason = if was_attached {
                        "previously attached"
                    } else {
                        "matches auto_attach filter"
                    };
                    info!(
                        "Auto-attaching device {:?} ({:04x}:{:04x}) - {}",
                        device.id, device.vendor_id, device.product_id, reason
                    );

                    // Attempt to attach the device
                    match IrohClient::create_device_proxy(client.clone(), server_id, device.clone())
                        .await
                    {
                        Ok(device_proxy) => match virtual_usb.attach_device(device_proxy).await {
                            Ok(global_id) => {
                                info!(
                                    "Auto-attach successful for device {:?} ({})",
                                    device.id, global_id
                                );
                                // Track for future auto-reattach
                                previously_attached.write().await.insert(device.id);
                            }
                            Err(e) => {
                                warn!("Auto-attach failed for device {:?}: {:#}", device.id, e);
                            }
                        },
                        Err(e) => {
                            warn!(
                                "Failed to create device proxy for auto-attach {:?}: {:#}",
                                device.id, e
                            );
                        }
                    }
                }
            }
            Ok(DeviceNotification::DeviceRemoved {
                device_id,
                invalidated_handles,
                reason,
            }) => {
                info!("Device {:?} removed from server: {:?}", device_id, reason);

                // Track this device for potential auto-reattach when it returns
                // (it was attached if we had handles for it)
                if !invalidated_handles.is_empty() {
                    previously_attached.write().await.insert(device_id);
                    debug!(
                        "Device {:?} added to previously_attached for auto-reattach",
                        device_id
                    );
                }

                if let Err(e) = virtual_usb
                    .handle_device_removed(server_id, device_id, invalidated_handles)
                    .await
                {
                    warn!("Error during device cleanup: {}", e);
                }
            }
            Ok(DeviceNotification::DeviceStatusChanged {
                device_id,
                device_info,
                reason,
            }) => {
                info!(
                    "Device {:?} status changed: {:?}, info: {:?}",
                    device_id,
                    reason,
                    device_info.as_ref().map(|d| &d.id)
                );
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                warn!("Missed {} device notifications", n);
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                debug!("Notification channel closed");
                break;
            }
        }
    }
}

/// Set up the reconciliation callback for handling device state after reconnection
///
/// This callback is invoked after successful reconnection to a server.
/// It compares the server's current device list with locally attached devices
/// and cleans up any stale local state.
async fn setup_reconciliation_callback(
    client: &Arc<IrohClient>,
    virtual_usb: Arc<VirtualUsbManager>,
) {
    let virtual_usb_clone = virtual_usb.clone();

    let callback: network::ReconciliationCallback = Arc::new(move |server_id, server_devices| {
        let virtual_usb = virtual_usb_clone.clone();

        Box::pin(async move { reconcile_devices(server_id, server_devices, &virtual_usb).await })
    });

    client.set_reconciliation_callback(callback).await;
}

/// Reconcile local device state with server's current device list
///
/// After a reconnection, devices may have been removed from the server
/// while we were disconnected. This function:
/// 1. Gets the list of devices currently on the server
/// 2. Compares with locally attached virtual devices
/// 3. Detaches any local devices that no longer exist on the server
async fn reconcile_devices(
    server_id: EndpointId,
    server_devices: Vec<protocol::DeviceInfo>,
    virtual_usb: &VirtualUsbManager,
) -> Result<ReconciliationResult> {
    info!(
        "Reconciling devices for server {}: {} devices on server",
        server_id,
        server_devices.len()
    );

    // Build set of device IDs currently on server
    let server_device_ids: HashSet<DeviceId> = server_devices.iter().map(|d| d.id).collect();

    // Get locally attached devices for this server
    let local_device_info = virtual_usb.get_attached_device_info(server_id).await;

    debug!(
        "Local attached devices for server: {}, Server devices: {}",
        local_device_info.len(),
        server_device_ids.len()
    );

    let mut result = ReconciliationResult::default();

    // Find devices that are locally attached but no longer on server
    for (global_id, device_id) in local_device_info {
        if !server_device_ids.contains(&device_id) {
            info!(
                "Device {:?} ({}) no longer on server, detaching local virtual device",
                device_id, global_id
            );

            match virtual_usb.detach_device(global_id).await {
                Ok(()) => {
                    info!("Successfully detached stale device {:?}", device_id);
                    result.detached_count += 1;
                }
                Err(e) => {
                    warn!("Failed to detach stale device {:?}: {}", device_id, e);
                    result.failed_device_ids.push(device_id);
                }
            }
        } else {
            debug!(
                "Device {:?} ({}) still exists on server",
                device_id, global_id
            );
        }
    }

    info!(
        "Reconciliation complete: {} devices detached, {} failures",
        result.detached_count,
        result.failed_device_ids.len()
    );

    Ok(result)
}
