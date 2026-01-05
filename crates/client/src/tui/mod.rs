//! Terminal User Interface
//!
//! Provides an interactive TUI for managing server connections and devices.
//!
//! # Layout
//!
//! The TUI is organized in three main sections:
//! - **Top Panel**: Status bar showing client EndpointId and connection stats
//! - **Center Panel**: Two-pane view with servers (left) and devices (right)
//! - **Bottom Panel**: Help bar with context-sensitive keybindings
//!
//! # Keybindings
//!
//! - `Tab`: Switch between server and device pane
//! - `j/k` or arrow keys: Navigate lists
//! - `Enter`: Connect to server / Attach device
//! - `d`: Disconnect from server / Detach device
//! - `r`: Refresh device list
//! - `a`: Add server by EndpointId
//! - `q`: Quit (with confirmation)
//! - `?`: Show help

pub mod app;
pub mod events;
pub mod ui;

use anyhow::{Context, Result};
use crossterm::{
    event::Event,
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use iroh::PublicKey as EndpointId;
use protocol::DeviceInfo;
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io::{self, Stdout};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::config::ClientConfig;
use crate::network::IrohClient;
use crate::virtual_usb::VirtualUsbManager;

pub use app::{App, AppAction, DeviceStatus, ServerStatus};
pub use events::{AsyncEventHandler, EventHandler};

/// Messages sent from async tasks to the TUI
#[derive(Debug)]
pub enum TuiMessage {
    /// Server connection established
    ServerConnected(EndpointId),
    /// Server connection failed
    ServerConnectionFailed(EndpointId, String),
    /// Server disconnected
    ServerDisconnected(EndpointId),
    /// Device list received from server
    DeviceListReceived(EndpointId, Vec<DeviceInfo>),
    /// Device attached successfully
    DeviceAttached(EndpointId, protocol::DeviceId, protocol::DeviceHandle),
    /// Device attach failed
    DeviceAttachFailed(EndpointId, protocol::DeviceId, String),
    /// Device detached
    DeviceDetached(EndpointId, protocol::DeviceHandle),
    /// Status message
    StatusMessage(String),
}

/// TUI runner that manages the terminal and event loop
pub struct TuiRunner {
    /// Terminal instance
    terminal: Terminal<CrosstermBackend<Stdout>>,
    /// Application state
    app: App,
    /// Event handler
    event_handler: EventHandler,
    /// Iroh network client
    client: Arc<IrohClient>,
    /// Virtual USB manager
    virtual_usb: Arc<VirtualUsbManager>,
    /// Channel for receiving messages from async tasks
    message_rx: mpsc::Receiver<TuiMessage>,
    /// Channel for sending messages from async tasks
    message_tx: mpsc::Sender<TuiMessage>,
}

impl TuiRunner {
    /// Create a new TUI runner
    pub fn new(
        client: Arc<IrohClient>,
        virtual_usb: Arc<VirtualUsbManager>,
        config: &ClientConfig,
    ) -> Result<Self> {
        // Setup terminal
        enable_raw_mode().context("Failed to enable raw mode")?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen).context("Failed to enter alternate screen")?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend).context("Failed to create terminal")?;

        // Create message channel
        let (message_tx, message_rx) = mpsc::channel(100);

        // Create application state
        let mut app = App::new(client.endpoint_id());

        // Add pre-configured servers
        for server_str in &config.servers.approved_servers {
            if let Ok(server_id) = server_str.parse::<EndpointId>() {
                app.add_server(server_id, None);
            }
        }

        Ok(Self {
            terminal,
            app,
            event_handler: EventHandler::new(),
            client,
            virtual_usb,
            message_rx,
            message_tx,
        })
    }

    /// Get a clone of the message sender for async tasks
    pub fn message_sender(&self) -> mpsc::Sender<TuiMessage> {
        self.message_tx.clone()
    }

    /// Run the TUI main loop
    pub async fn run(&mut self) -> Result<()> {
        info!("Starting TUI");

        // Initial render
        self.terminal.draw(|f| ui::render(f, &self.app))?;

        loop {
            // Process any pending messages from async tasks
            while let Ok(msg) = self.message_rx.try_recv() {
                self.handle_message(msg);
            }

            // Poll for terminal events
            if let Some(event) = self.event_handler.poll()? {
                let action = match event {
                    Event::Key(key) => self.event_handler.handle_key(&mut self.app, key),
                    Event::Resize(_, _) => {
                        // Terminal will re-render on next draw
                        AppAction::None
                    }
                    _ => AppAction::None,
                };

                // Handle the action
                self.handle_action(action).await?;
            }

            // Check if we should quit
            if self.app.should_quit {
                break;
            }

            // Render
            self.terminal.draw(|f| ui::render(f, &self.app))?;
        }

        info!("TUI shutting down");
        Ok(())
    }

    /// Handle TUI message from async task
    fn handle_message(&mut self, msg: TuiMessage) {
        match msg {
            TuiMessage::ServerConnected(endpoint_id) => {
                self.app
                    .update_server_status(&endpoint_id, ServerStatus::Connected);
                self.app
                    .set_status(format!("Connected to {}", truncate_id(&endpoint_id)));
            }
            TuiMessage::ServerConnectionFailed(endpoint_id, error) => {
                self.app.set_server_error(&endpoint_id, error.clone());
                self.app.set_status(format!(
                    "Failed to connect to {}: {}",
                    truncate_id(&endpoint_id),
                    error
                ));
            }
            TuiMessage::ServerDisconnected(endpoint_id) => {
                self.app
                    .update_server_status(&endpoint_id, ServerStatus::Disconnected);
                self.app
                    .set_status(format!("Disconnected from {}", truncate_id(&endpoint_id)));
            }
            TuiMessage::DeviceListReceived(endpoint_id, devices) => {
                self.app.update_devices(&endpoint_id, devices);
            }
            TuiMessage::DeviceAttached(endpoint_id, device_id, handle) => {
                self.app.update_device_status(
                    &endpoint_id,
                    device_id,
                    DeviceStatus::Attached,
                    Some(handle),
                );
                self.app
                    .set_status(format!("Attached device {} as virtual USB", device_id.0));
            }
            TuiMessage::DeviceAttachFailed(endpoint_id, device_id, error) => {
                self.app.set_device_error(&endpoint_id, device_id, error);
            }
            TuiMessage::DeviceDetached(endpoint_id, handle) => {
                // Find the device by handle and update its status
                if let Some(devices) = self.app.devices.get(&endpoint_id) {
                    if let Some(device) = devices.iter().find(|d| d.handle == Some(handle)) {
                        let device_id = device.info.id;
                        self.app.update_device_status(
                            &endpoint_id,
                            device_id,
                            DeviceStatus::Available,
                            None,
                        );
                    }
                }
                self.app.set_status("Device detached".to_string());
            }
            TuiMessage::StatusMessage(msg) => {
                self.app.set_status(msg);
            }
        }
    }

    /// Handle an application action
    async fn handle_action(&mut self, action: AppAction) -> Result<()> {
        match action {
            AppAction::None => {}
            AppAction::Quit => {
                // Cleanup will happen in the caller
            }
            AppAction::ConnectServer(endpoint_id) => {
                self.app
                    .update_server_status(&endpoint_id, ServerStatus::Connecting);
                self.spawn_connect_server(endpoint_id);
            }
            AppAction::DisconnectServer(endpoint_id) => {
                self.spawn_disconnect_server(endpoint_id);
            }
            AppAction::AttachDevice(endpoint_id, device_id) => {
                self.app.update_device_status(
                    &endpoint_id,
                    device_id,
                    DeviceStatus::Attaching,
                    None,
                );
                self.spawn_attach_device(endpoint_id, device_id);
            }
            AppAction::DetachDevice(endpoint_id, handle) => {
                // Find the device by handle
                if let Some(devices) = self.app.devices.get(&endpoint_id) {
                    if let Some(device) = devices.iter().find(|d| d.handle == Some(handle)) {
                        let device_id = device.info.id;
                        self.app.update_device_status(
                            &endpoint_id,
                            device_id,
                            DeviceStatus::Detaching,
                            Some(handle),
                        );
                    }
                }
                self.spawn_detach_device(endpoint_id, handle);
            }
            AppAction::RefreshDevices(endpoint_id) => {
                self.app.set_status("Refreshing device list...".to_string());
                self.spawn_refresh_devices(endpoint_id);
            }
            AppAction::AddServer(server_str) => match server_str.parse::<EndpointId>() {
                Ok(endpoint_id) => {
                    self.app.add_server(endpoint_id, None);
                    self.app
                        .set_status(format!("Added server {}", truncate_id(&endpoint_id)));
                }
                Err(e) => {
                    self.app.set_status(format!("Invalid EndpointId: {}", e));
                }
            },
        }
        Ok(())
    }

    /// Spawn async task to connect to server
    fn spawn_connect_server(&self, endpoint_id: EndpointId) {
        let client = self.client.clone();
        let tx = self.message_tx.clone();

        tokio::spawn(async move {
            match client.connect_to_server(endpoint_id, None).await {
                Ok(()) => {
                    let _ = tx.send(TuiMessage::ServerConnected(endpoint_id)).await;

                    // Automatically fetch device list after connecting
                    match client.list_remote_devices(endpoint_id).await {
                        Ok(devices) => {
                            let _ = tx
                                .send(TuiMessage::DeviceListReceived(endpoint_id, devices))
                                .await;
                        }
                        Err(e) => {
                            warn!("Failed to list devices: {}", e);
                        }
                    }
                }
                Err(e) => {
                    let _ = tx
                        .send(TuiMessage::ServerConnectionFailed(
                            endpoint_id,
                            e.to_string(),
                        ))
                        .await;
                }
            }
        });
    }

    /// Spawn async task to disconnect from server
    fn spawn_disconnect_server(&self, endpoint_id: EndpointId) {
        let client = self.client.clone();
        let tx = self.message_tx.clone();

        tokio::spawn(async move {
            match client.disconnect_from_server(endpoint_id).await {
                Ok(()) => {
                    let _ = tx.send(TuiMessage::ServerDisconnected(endpoint_id)).await;
                }
                Err(e) => {
                    error!("Failed to disconnect: {}", e);
                    let _ = tx
                        .send(TuiMessage::StatusMessage(format!(
                            "Failed to disconnect: {}",
                            e
                        )))
                        .await;
                }
            }
        });
    }

    /// Spawn async task to attach device
    fn spawn_attach_device(&self, endpoint_id: EndpointId, device_id: protocol::DeviceId) {
        let client = self.client.clone();
        let virtual_usb = self.virtual_usb.clone();
        let tx = self.message_tx.clone();

        tokio::spawn(async move {
            // Get device info
            let device_info = match client.list_remote_devices(endpoint_id).await {
                Ok(devices) => devices.into_iter().find(|d| d.id == device_id),
                Err(e) => {
                    let _ = tx
                        .send(TuiMessage::DeviceAttachFailed(
                            endpoint_id,
                            device_id,
                            e.to_string(),
                        ))
                        .await;
                    return;
                }
            };

            let device_info = match device_info {
                Some(info) => info,
                None => {
                    let _ = tx
                        .send(TuiMessage::DeviceAttachFailed(
                            endpoint_id,
                            device_id,
                            "Device not found".to_string(),
                        ))
                        .await;
                    return;
                }
            };

            // Create device proxy
            let device_proxy =
                match IrohClient::create_device_proxy(client, endpoint_id, device_info).await {
                    Ok(proxy) => proxy,
                    Err(e) => {
                        let _ = tx
                            .send(TuiMessage::DeviceAttachFailed(
                                endpoint_id,
                                device_id,
                                e.to_string(),
                            ))
                            .await;
                        return;
                    }
                };

            // Attach as virtual USB
            match virtual_usb.attach_device(device_proxy).await {
                Ok(handle) => {
                    let _ = tx
                        .send(TuiMessage::DeviceAttached(endpoint_id, device_id, handle))
                        .await;
                }
                Err(e) => {
                    let _ = tx
                        .send(TuiMessage::DeviceAttachFailed(
                            endpoint_id,
                            device_id,
                            e.to_string(),
                        ))
                        .await;
                }
            }
        });
    }

    /// Spawn async task to detach device
    fn spawn_detach_device(&self, endpoint_id: EndpointId, handle: protocol::DeviceHandle) {
        let virtual_usb = self.virtual_usb.clone();
        let tx = self.message_tx.clone();

        tokio::spawn(async move {
            match virtual_usb.detach_device(handle).await {
                Ok(()) => {
                    let _ = tx
                        .send(TuiMessage::DeviceDetached(endpoint_id, handle))
                        .await;
                }
                Err(e) => {
                    let _ = tx
                        .send(TuiMessage::StatusMessage(format!(
                            "Failed to detach: {}",
                            e
                        )))
                        .await;
                }
            }
        });
    }

    /// Spawn async task to refresh device list
    fn spawn_refresh_devices(&self, endpoint_id: EndpointId) {
        let client = self.client.clone();
        let tx = self.message_tx.clone();

        tokio::spawn(async move {
            match client.list_remote_devices(endpoint_id).await {
                Ok(devices) => {
                    let _ = tx
                        .send(TuiMessage::DeviceListReceived(endpoint_id, devices))
                        .await;
                }
                Err(e) => {
                    let _ = tx
                        .send(TuiMessage::StatusMessage(format!(
                            "Failed to refresh devices: {}",
                            e
                        )))
                        .await;
                }
            }
        });
    }
}

impl Drop for TuiRunner {
    fn drop(&mut self) {
        // Restore terminal state
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

/// Run the TUI application
///
/// This is the main entry point for TUI mode. It creates a TuiRunner
/// and runs the main event loop.
///
/// # Arguments
/// * `client` - The Iroh network client
/// * `virtual_usb` - The virtual USB manager
/// * `config` - Client configuration
///
/// # Example
/// ```no_run
/// use std::sync::Arc;
/// use client::tui::run;
/// use client::network::IrohClient;
/// use client::virtual_usb::VirtualUsbManager;
/// use client::config::ClientConfig;
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     let client = Arc::new(IrohClient::new(Default::default()).await?);
///     let virtual_usb = Arc::new(VirtualUsbManager::new().await?);
///     let config = ClientConfig::default();
///     run(client, virtual_usb, &config).await
/// }
/// ```
pub async fn run(
    client: Arc<IrohClient>,
    virtual_usb: Arc<VirtualUsbManager>,
    config: &ClientConfig,
) -> Result<()> {
    let mut runner = TuiRunner::new(client.clone(), virtual_usb.clone(), config)?;

    // Run the TUI
    let result = runner.run().await;

    // Cleanup: detach all virtual USB devices
    info!("Cleaning up virtual USB devices...");
    let attached_devices = virtual_usb.list_devices().await;
    for device_handle in attached_devices {
        if let Err(e) = virtual_usb.detach_device(device_handle).await {
            warn!("Failed to detach device {}: {}", device_handle.0, e);
        }
    }

    // Disconnect from all servers
    info!("Disconnecting from servers...");
    let connected_servers = client.connected_servers().await;
    for server_id in connected_servers {
        if let Err(e) = client.disconnect_from_server(server_id).await {
            warn!("Failed to disconnect from {}: {}", server_id, e);
        }
    }

    result
}

/// Helper to truncate EndpointId for display
fn truncate_id(id: &EndpointId) -> String {
    let s = format!("{}", id);
    if s.len() > 16 {
        format!("{}...", &s[..16])
    } else {
        s
    }
}
