//! TUI application state
//!
//! Manages the application state, event loop, and coordinates between
//! the UI rendering and the USB/network subsystems.

use anyhow::{Context, Result};
use common::{UsbBridge, UsbCommand, UsbEvent};
use crossterm::{
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use iroh::PublicKey as EndpointId;
use protocol::DeviceInfo;
use ratatui::{Terminal, backend::CrosstermBackend};
use std::collections::{HashMap, HashSet};
use std::io::{self, Stdout};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use super::events::{Action, Event, EventHandler};
use super::ui;

/// Device sharing state
#[derive(Debug, Clone)]
pub struct DeviceState {
    /// Device information
    pub info: DeviceInfo,
    /// Whether the device is shared (available for clients)
    pub shared: bool,
    /// Connected client EndpointIds using this device
    pub clients: HashSet<String>,
}

/// Current dialog/popup being displayed
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dialog {
    /// No dialog open
    None,
    /// Help dialog showing keybindings
    Help,
    /// Device details dialog
    DeviceDetails,
    /// Connected clients dialog
    Clients,
    /// Confirm device reset
    ConfirmReset,
}

/// Application state
pub struct App {
    /// Server's EndpointId
    endpoint_id: EndpointId,
    /// USB device states indexed by device ID
    devices: HashMap<u32, DeviceState>,
    /// Ordered list of device IDs for display
    device_order: Vec<u32>,
    /// Currently selected device index
    selected_index: usize,
    /// Current dialog being displayed
    dialog: Dialog,
    /// Whether the app should quit
    should_quit: bool,
    /// Server start time (for uptime calculation)
    start_time: Instant,
    /// Number of active client connections
    connection_count: usize,
    /// USB bridge for communication with USB subsystem
    usb_bridge: UsbBridge,
    /// Channel receiver for network events
    network_rx: mpsc::UnboundedReceiver<NetworkEvent>,
    /// Whether auto-share is enabled
    auto_share: bool,
    /// Pending reset action confirmed by user
    pub pending_reset: bool,
}

/// Network events for updating the TUI
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum NetworkEvent {
    /// Client connected
    ClientConnected { endpoint_id: String },
    /// Client disconnected
    ClientDisconnected { endpoint_id: String },
    /// Client attached to a device
    ClientAttachedDevice { endpoint_id: String, device_id: u32 },
    /// Client detached from a device
    ClientDetachedDevice { endpoint_id: String, device_id: u32 },
}

impl App {
    /// Create a new application instance
    pub fn new(
        endpoint_id: EndpointId,
        usb_bridge: UsbBridge,
        network_rx: mpsc::UnboundedReceiver<NetworkEvent>,
        auto_share: bool,
    ) -> Self {
        Self {
            endpoint_id,
            devices: HashMap::new(),
            device_order: Vec::new(),
            selected_index: 0,
            dialog: Dialog::None,
            should_quit: false,
            start_time: Instant::now(),
            connection_count: 0,
            usb_bridge,
            network_rx,
            auto_share,
            pending_reset: false,
        }
    }

    /// Get the server's EndpointId
    pub fn endpoint_id(&self) -> &EndpointId {
        &self.endpoint_id
    }

    /// Get the server uptime
    pub fn uptime(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// Get the connection count
    pub fn connection_count(&self) -> usize {
        self.connection_count
    }

    /// Get the device list for display
    pub fn devices(&self) -> Vec<&DeviceState> {
        self.device_order
            .iter()
            .filter_map(|id| self.devices.get(id))
            .collect()
    }

    /// Get the currently selected device
    pub fn selected_device(&self) -> Option<&DeviceState> {
        self.device_order
            .get(self.selected_index)
            .and_then(|id| self.devices.get(id))
    }

    /// Get the selected index
    pub fn selected_index(&self) -> usize {
        self.selected_index
    }

    /// Get the current dialog
    pub fn dialog(&self) -> &Dialog {
        &self.dialog
    }

    /// Check if app should quit
    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    /// Handle user action
    pub fn handle_action(&mut self, action: Action) {
        match action {
            Action::Quit => {
                if self.dialog != Dialog::None {
                    self.dialog = Dialog::None;
                } else {
                    self.should_quit = true;
                }
            }
            Action::CloseDialog => {
                self.dialog = Dialog::None;
            }
            Action::Up => {
                if self.dialog == Dialog::None && !self.device_order.is_empty() {
                    if self.selected_index > 0 {
                        self.selected_index -= 1;
                    }
                }
            }
            Action::Down => {
                if self.dialog == Dialog::None && !self.device_order.is_empty() {
                    if self.selected_index < self.device_order.len() - 1 {
                        self.selected_index += 1;
                    }
                }
            }
            Action::ToggleSharing => {
                if self.dialog == Dialog::None {
                    if let Some(&device_id) = self.device_order.get(self.selected_index) {
                        if let Some(device) = self.devices.get_mut(&device_id) {
                            device.shared = !device.shared;
                            info!(
                                "Device {} sharing: {}",
                                device_id,
                                if device.shared { "enabled" } else { "disabled" }
                            );
                        }
                    }
                }
            }
            Action::ViewDetails => {
                match self.dialog {
                    Dialog::None => {
                        if self.selected_device().is_some() {
                            self.dialog = Dialog::DeviceDetails;
                        }
                    }
                    Dialog::ConfirmReset => {
                        // Enter in ConfirmReset dialog confirms the reset
                        self.dialog = Dialog::None;
                        self.pending_reset = true;
                    }
                    _ => {}
                }
            }
            Action::ViewClients => {
                if self.dialog == Dialog::None {
                    self.dialog = Dialog::Clients;
                }
            }
            Action::ShowHelp => {
                self.dialog = Dialog::Help;
            }
            Action::Refresh => {
                // Refresh will be handled in the main loop by re-fetching devices
                debug!("Refresh requested");
            }
            Action::ResetDevice => {
                if self.dialog == Dialog::None && self.selected_device().is_some() {
                    self.dialog = Dialog::ConfirmReset;
                }
            }
            Action::Confirm => {
                if self.dialog == Dialog::ConfirmReset {
                    // Reset confirmed
                    self.dialog = Dialog::None;
                    self.pending_reset = true;
                }
            }
            Action::None => {}
        }
    }

    /// Reset the currently selected device
    pub async fn reset_selected_device(&self) -> Result<()> {
        if let Some(&device_id) = self.device_order.get(self.selected_index) {
             let (tx, rx) = tokio::sync::oneshot::channel();
             // We need to look up the handle from device_id, but the current state structure
             // assumes DeviceId == DeviceHandle value (which is true in current implementation).
             // Let's use the device_id as the handle value.
             let handle = protocol::DeviceHandle(device_id);

             info!("Sending reset command for device {}", device_id);
             self.usb_bridge
                .send_command(UsbCommand::ResetDevice { handle, response: tx })
                .await
                .context("Failed to send ResetDevice command")?;

             match rx.await.context("Failed to receive reset response")? {
                 Ok(_) => {
                     info!("Device {} reset successfully", device_id);
                 }
                 Err(e) => {
                     warn!("Failed to reset device {}: {:?}", device_id, e);
                     return Err(anyhow::anyhow!("Reset failed: {:?}", e));
                 }
             }
        }
        Ok(())
    }

    /// Update device list from USB subsystem
    pub async fn refresh_devices(&mut self) -> Result<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.usb_bridge
            .send_command(UsbCommand::ListDevices { response: tx })
            .await
            .context("Failed to send ListDevices command")?;

        let devices = rx.await.context("Failed to receive device list")?;

        // Update device states, preserving sharing status for existing devices
        let mut new_devices = HashMap::new();
        let mut new_order = Vec::new();

        for info in devices {
            let id = info.id.0;
            let state = if let Some(existing) = self.devices.get(&id) {
                DeviceState {
                    info,
                    shared: existing.shared,
                    clients: existing.clients.clone(),
                }
            } else {
                DeviceState {
                    info,
                    shared: self.auto_share,
                    clients: HashSet::new(),
                }
            };
            new_devices.insert(id, state);
            new_order.push(id);
        }

        self.devices = new_devices;
        self.device_order = new_order;

        // Adjust selected index if necessary
        if !self.device_order.is_empty() && self.selected_index >= self.device_order.len() {
            self.selected_index = self.device_order.len() - 1;
        }

        Ok(())
    }

    /// Process USB events (hotplug)
    pub fn handle_usb_event(&mut self, event: UsbEvent) {
        match event {
            UsbEvent::DeviceArrived { device } => {
                let id = device.id.0;
                info!(
                    "Device arrived: {} {:04x}:{:04x}",
                    device.product.as_deref().unwrap_or("Unknown"),
                    device.vendor_id,
                    device.product_id
                );
                self.devices.insert(
                    id,
                    DeviceState {
                        info: device,
                        shared: self.auto_share,
                        clients: HashSet::new(),
                    },
                );
                if !self.device_order.contains(&id) {
                    self.device_order.push(id);
                }
            }
            UsbEvent::DeviceLeft { device_id, .. } => {
                let id = device_id.0;
                info!("Device left: {}", id);
                self.devices.remove(&id);
                self.device_order.retain(|&x| x != id);

                // Adjust selected index
                if !self.device_order.is_empty() && self.selected_index >= self.device_order.len() {
                    self.selected_index = self.device_order.len() - 1;
                }
            }
        }
    }

    /// Process network events
    pub fn handle_network_event(&mut self, event: NetworkEvent) {
        match event {
            NetworkEvent::ClientConnected { endpoint_id } => {
                self.connection_count += 1;
                info!("Client connected: {}", endpoint_id);
            }
            NetworkEvent::ClientDisconnected { endpoint_id } => {
                if self.connection_count > 0 {
                    self.connection_count -= 1;
                }
                info!("Client disconnected: {}", endpoint_id);
                // Remove client from all devices
                for device in self.devices.values_mut() {
                    device.clients.remove(&endpoint_id);
                }
            }
            NetworkEvent::ClientAttachedDevice {
                endpoint_id,
                device_id,
            } => {
                if let Some(device) = self.devices.get_mut(&device_id) {
                    device.clients.insert(endpoint_id.clone());
                    info!("Client {} attached to device {}", endpoint_id, device_id);
                }
            }
            NetworkEvent::ClientDetachedDevice {
                endpoint_id,
                device_id,
            } => {
                if let Some(device) = self.devices.get_mut(&device_id) {
                    device.clients.remove(&endpoint_id);
                    info!("Client {} detached from device {}", endpoint_id, device_id);
                }
            }
        }
    }

    /// Drain pending network events
    pub fn drain_network_events(&mut self) {
        while let Ok(event) = self.network_rx.try_recv() {
            self.handle_network_event(event);
        }
    }
}

/// Terminal wrapper for setup/teardown
pub struct Tui {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl Tui {
    /// Create and initialize the terminal
    pub fn new() -> Result<Self> {
        let backend = CrosstermBackend::new(io::stdout());
        let terminal = Terminal::new(backend)?;
        Ok(Self { terminal })
    }

    /// Enter TUI mode (raw mode, alternate screen)
    pub fn enter(&mut self) -> Result<()> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen)?;
        self.terminal.hide_cursor()?;
        self.terminal.clear()?;
        Ok(())
    }

    /// Exit TUI mode (restore terminal state)
    pub fn exit(&mut self) -> Result<()> {
        disable_raw_mode()?;
        execute!(io::stdout(), LeaveAlternateScreen)?;
        self.terminal.show_cursor()?;
        Ok(())
    }

    /// Draw the UI
    pub fn draw(&mut self, app: &App) -> Result<()> {
        self.terminal.draw(|frame| {
            ui::render(frame, app);
        })?;
        Ok(())
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        // Best effort cleanup
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

/// Run the TUI application
///
/// This is the main entry point for the TUI mode.
pub async fn run(
    endpoint_id: EndpointId,
    usb_bridge: UsbBridge,
    network_rx: mpsc::UnboundedReceiver<NetworkEvent>,
    auto_share: bool,
) -> Result<()> {
    // Initialize TUI
    let mut tui = Tui::new()?;
    tui.enter()?;

    // Create app state
    let mut app = App::new(endpoint_id, usb_bridge.clone(), network_rx, auto_share);

    // Initial device list fetch
    if let Err(e) = app.refresh_devices().await {
        warn!("Failed to fetch initial device list: {:#}", e);
    }

    // Create event handler (250ms tick rate for UI updates)
    let mut events = EventHandler::new(Duration::from_millis(250));

    // Main event loop
    loop {
        // Draw UI
        if let Err(e) = tui.draw(&app) {
            error!("Failed to draw UI: {:#}", e);
            break;
        }

        // Handle pending reset (check this after handle_action updates state)
        if app.pending_reset {
            app.pending_reset = false;
            if let Err(e) = app.reset_selected_device().await {
                warn!("Failed to reset device: {:#}", e);
            }
        }

        // Handle events
        tokio::select! {
            // Terminal events (keyboard, resize, tick)
            event = events.next() => {
                match event {
                    Some(Event::Key(key)) => {
                        let action = Action::from(key);
                        app.handle_action(action);

                        // Special handling for refresh
                        if action == Action::Refresh {
                            if let Err(e) = app.refresh_devices().await {
                                warn!("Failed to refresh devices: {:#}", e);
                            }
                        }
                        // ResetDevice logic is now handled by checking app.pending_reset
                        // which is set by handle_action when ConfirmReset dialog is confirmed
                    }
                    Some(Event::Resize(_, _)) => {
                        // Terminal resize is handled automatically by ratatui
                    }
                    Some(Event::Tick) => {
                        // Periodic update - drain network events
                        app.drain_network_events();
                    }
                    None => {
                        // Event channel closed
                        break;
                    }
                }
            }

            // USB events (hotplug)
            usb_event = usb_bridge.recv_event() => {
                match usb_event {
                    Ok(event) => app.handle_usb_event(event),
                    Err(e) => {
                        warn!("USB event error: {:#}", e);
                    }
                }
            }
        }

        // Check if we should quit
        if app.should_quit() {
            break;
        }
    }

    // Cleanup
    tui.exit()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::DeviceId;

    fn create_test_device_info(id: u32) -> DeviceInfo {
        DeviceInfo {
            id: DeviceId(id),
            vendor_id: 0x1234,
            product_id: 0x5678,
            bus_number: 1,
            device_address: id as u8,
            manufacturer: Some("Test Manufacturer".to_string()),
            product: Some("Test Product".to_string()),
            serial_number: Some("12345".to_string()),
            class: 0,
            subclass: 0,
            protocol: 0,
            speed: protocol::DeviceSpeed::High,
            num_configurations: 1,
        }
    }

    #[test]
    fn test_device_state() {
        let info = create_test_device_info(1);
        let state = DeviceState {
            info,
            shared: false,
            clients: HashSet::new(),
        };

        assert!(!state.shared);
        assert!(state.clients.is_empty());
    }

    #[test]
    fn test_handle_usb_event_arrived() {
        let (_, network_rx) = mpsc::unbounded_channel();
        let (usb_bridge, _worker) = common::create_usb_bridge();
        let endpoint_id = EndpointId::from_bytes(&[0u8; 32]).unwrap();

        let mut app = App::new(endpoint_id, usb_bridge, network_rx, false);

        let device = create_test_device_info(42);
        app.handle_usb_event(UsbEvent::DeviceArrived { device });

        assert_eq!(app.devices.len(), 1);
        assert!(app.devices.contains_key(&42));
        assert_eq!(app.device_order, vec![42]);
    }

    #[test]
    fn test_handle_usb_event_left() {
        let (_, network_rx) = mpsc::unbounded_channel();
        let (usb_bridge, _worker) = common::create_usb_bridge();
        let endpoint_id = EndpointId::from_bytes(&[0u8; 32]).unwrap();

        let mut app = App::new(endpoint_id, usb_bridge, network_rx, false);

        // Add device
        let device = create_test_device_info(42);
        app.handle_usb_event(UsbEvent::DeviceArrived { device });
        assert_eq!(app.devices.len(), 1);

        // Remove device
        app.handle_usb_event(UsbEvent::DeviceLeft {
            device_id: DeviceId(42),
            invalidated_handles: vec![],
            affected_clients: vec![],
        });
        assert!(app.devices.is_empty());
        assert!(app.device_order.is_empty());
    }

    #[test]
    fn test_navigation() {
        let (_, network_rx) = mpsc::unbounded_channel();
        let (usb_bridge, _worker) = common::create_usb_bridge();
        let endpoint_id = EndpointId::from_bytes(&[0u8; 32]).unwrap();

        let mut app = App::new(endpoint_id, usb_bridge, network_rx, false);

        // Add some devices
        for i in 0..3 {
            let device = create_test_device_info(i);
            app.handle_usb_event(UsbEvent::DeviceArrived { device });
        }

        assert_eq!(app.selected_index, 0);

        app.handle_action(Action::Down);
        assert_eq!(app.selected_index, 1);

        app.handle_action(Action::Down);
        assert_eq!(app.selected_index, 2);

        // Should not go past end
        app.handle_action(Action::Down);
        assert_eq!(app.selected_index, 2);

        app.handle_action(Action::Up);
        assert_eq!(app.selected_index, 1);

        app.handle_action(Action::Up);
        assert_eq!(app.selected_index, 0);

        // Should not go below 0
        app.handle_action(Action::Up);
        assert_eq!(app.selected_index, 0);
    }

    #[test]
    fn test_toggle_sharing() {
        let (_, network_rx) = mpsc::unbounded_channel();
        let (usb_bridge, _worker) = common::create_usb_bridge();
        let endpoint_id = EndpointId::from_bytes(&[0u8; 32]).unwrap();

        let mut app = App::new(endpoint_id, usb_bridge, network_rx, false);

        let device = create_test_device_info(42);
        app.handle_usb_event(UsbEvent::DeviceArrived { device });

        assert!(!app.devices.get(&42).unwrap().shared);

        app.handle_action(Action::ToggleSharing);
        assert!(app.devices.get(&42).unwrap().shared);

        app.handle_action(Action::ToggleSharing);
        assert!(!app.devices.get(&42).unwrap().shared);
    }

    #[test]
    fn test_dialogs() {
        let (_, network_rx) = mpsc::unbounded_channel();
        let (usb_bridge, _worker) = common::create_usb_bridge();
        let endpoint_id = EndpointId::from_bytes(&[0u8; 32]).unwrap();

        let mut app = App::new(endpoint_id, usb_bridge, network_rx, false);

        assert_eq!(app.dialog, Dialog::None);

        app.handle_action(Action::ShowHelp);
        assert_eq!(app.dialog, Dialog::Help);

        app.handle_action(Action::CloseDialog);
        assert_eq!(app.dialog, Dialog::None);

        app.handle_action(Action::ViewClients);
        assert_eq!(app.dialog, Dialog::Clients);

        // Quit should close dialog first
        app.handle_action(Action::Quit);
        assert_eq!(app.dialog, Dialog::None);
        assert!(!app.should_quit);

        // Then quit should actually quit
        app.handle_action(Action::Quit);
        assert!(app.should_quit);
    }
}
