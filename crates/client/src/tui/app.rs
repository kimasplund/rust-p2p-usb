//! TUI application state
//!
//! Manages the application state including server connections, device lists,
//! two-pane navigation, and popup dialogs.

use iroh::PublicKey as EndpointId;
use protocol::{DeviceHandle, DeviceId, DeviceInfo};
use std::collections::HashMap;

/// Server connection status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerStatus {
    /// Not connected
    Disconnected,
    /// Connection in progress
    Connecting,
    /// Connected and ready
    Connected,
    /// Connection failed or lost
    Failed,
}

/// Virtual device attachment status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum DeviceStatus {
    /// Available but not attached
    Available,
    /// Currently attaching
    Attaching,
    /// Attached as virtual USB device
    Attached,
    /// In use by another client
    Busy,
    /// Detaching in progress
    Detaching,
}

/// Information about a known server
#[derive(Debug, Clone)]
pub struct ServerInfo {
    /// Server EndpointId
    pub endpoint_id: EndpointId,
    /// Human-readable name (optional)
    pub name: Option<String>,
    /// Connection status
    pub status: ServerStatus,
    /// Number of devices available
    pub device_count: usize,
    /// Error message if failed
    pub error: Option<String>,
}

/// Information about a remote device with local status
#[derive(Debug, Clone)]
pub struct RemoteDevice {
    /// Device info from server
    pub info: DeviceInfo,
    /// Local attachment status
    pub status: DeviceStatus,
    /// Local device handle if attached
    pub handle: Option<DeviceHandle>,
    /// Error message if any
    pub error: Option<String>,
}

/// Active pane in the two-pane layout
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivePane {
    /// Server list pane (left)
    Servers,
    /// Device list pane (right)
    Devices,
}

/// Input mode for the application
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputMode {
    /// Normal navigation mode
    Normal,
    /// Adding a new server (input dialog)
    AddServer { input: String },
    /// Showing help overlay
    Help,
    /// Confirm quit dialog
    ConfirmQuit,
}

/// User action to be processed by the main loop
#[derive(Debug, Clone)]
pub enum AppAction {
    /// No action
    None,
    /// Quit the application
    Quit,
    /// Connect to selected server
    ConnectServer(EndpointId),
    /// Disconnect from selected server
    DisconnectServer(EndpointId),
    /// Attach selected device as virtual USB
    AttachDevice(EndpointId, DeviceId),
    /// Detach virtual USB device
    DetachDevice(EndpointId, DeviceHandle),
    /// Refresh device list from server
    RefreshDevices(EndpointId),
    /// Add a new server by EndpointId string
    AddServer(String),
}

/// Main application state
pub struct App {
    /// Client's own EndpointId
    pub client_endpoint_id: EndpointId,
    /// Known servers (by EndpointId)
    pub servers: HashMap<EndpointId, ServerInfo>,
    /// Server list order (for stable iteration)
    pub server_order: Vec<EndpointId>,
    /// Remote devices per server
    pub devices: HashMap<EndpointId, Vec<RemoteDevice>>,
    /// Currently active pane
    pub active_pane: ActivePane,
    /// Selected server index
    pub selected_server: usize,
    /// Selected device index (per server)
    pub selected_device: HashMap<EndpointId, usize>,
    /// Current input mode
    pub input_mode: InputMode,
    /// Status message to display
    pub status_message: Option<String>,
    /// Should quit flag
    pub should_quit: bool,
}

impl App {
    /// Create a new application state
    pub fn new(client_endpoint_id: EndpointId) -> Self {
        Self {
            client_endpoint_id,
            servers: HashMap::new(),
            server_order: Vec::new(),
            devices: HashMap::new(),
            active_pane: ActivePane::Servers,
            selected_server: 0,
            selected_device: HashMap::new(),
            input_mode: InputMode::Normal,
            status_message: None,
            should_quit: false,
        }
    }

    /// Add a server to the list
    pub fn add_server(&mut self, endpoint_id: EndpointId, name: Option<String>) {
        if !self.servers.contains_key(&endpoint_id) {
            self.servers.insert(
                endpoint_id,
                ServerInfo {
                    endpoint_id,
                    name,
                    status: ServerStatus::Disconnected,
                    device_count: 0,
                    error: None,
                },
            );
            self.server_order.push(endpoint_id);
        }
    }

    /// Remove a server from the list
    #[allow(dead_code)]
    pub fn remove_server(&mut self, endpoint_id: &EndpointId) {
        self.servers.remove(endpoint_id);
        self.server_order.retain(|id| id != endpoint_id);
        self.devices.remove(endpoint_id);
        self.selected_device.remove(endpoint_id);

        // Adjust selected index if needed
        if self.selected_server >= self.server_order.len() && self.selected_server > 0 {
            self.selected_server -= 1;
        }
    }

    /// Update server status
    pub fn update_server_status(&mut self, endpoint_id: &EndpointId, status: ServerStatus) {
        if let Some(server) = self.servers.get_mut(endpoint_id) {
            server.status = status;
            if status == ServerStatus::Connected {
                server.error = None;
            }
        }
    }

    /// Set server error
    pub fn set_server_error(&mut self, endpoint_id: &EndpointId, error: String) {
        if let Some(server) = self.servers.get_mut(endpoint_id) {
            server.status = ServerStatus::Failed;
            server.error = Some(error);
        }
    }

    /// Update devices for a server
    pub fn update_devices(&mut self, endpoint_id: &EndpointId, device_infos: Vec<DeviceInfo>) {
        // Preserve existing status for devices we already know about
        let existing_statuses: HashMap<DeviceId, (DeviceStatus, Option<DeviceHandle>)> = self
            .devices
            .get(endpoint_id)
            .map(|devices| {
                devices
                    .iter()
                    .map(|d| (d.info.id, (d.status, d.handle)))
                    .collect()
            })
            .unwrap_or_default();

        let devices: Vec<RemoteDevice> = device_infos
            .into_iter()
            .map(|info| {
                let (status, handle) = existing_statuses
                    .get(&info.id)
                    .copied()
                    .unwrap_or((DeviceStatus::Available, None));
                RemoteDevice {
                    info,
                    status,
                    handle,
                    error: None,
                }
            })
            .collect();

        let device_count = devices.len();
        self.devices.insert(*endpoint_id, devices);

        if let Some(server) = self.servers.get_mut(endpoint_id) {
            server.device_count = device_count;
        }
    }

    /// Update device status
    pub fn update_device_status(
        &mut self,
        endpoint_id: &EndpointId,
        device_id: DeviceId,
        status: DeviceStatus,
        handle: Option<DeviceHandle>,
    ) {
        if let Some(devices) = self.devices.get_mut(endpoint_id) {
            if let Some(device) = devices.iter_mut().find(|d| d.info.id == device_id) {
                device.status = status;
                device.handle = handle;
                if status == DeviceStatus::Attached || status == DeviceStatus::Available {
                    device.error = None;
                }
            }
        }
    }

    /// Set device error
    pub fn set_device_error(
        &mut self,
        endpoint_id: &EndpointId,
        device_id: DeviceId,
        error: String,
    ) {
        if let Some(devices) = self.devices.get_mut(endpoint_id) {
            if let Some(device) = devices.iter_mut().find(|d| d.info.id == device_id) {
                device.error = Some(error);
                device.status = DeviceStatus::Available;
            }
        }
    }

    /// Get the currently selected server
    pub fn selected_server(&self) -> Option<&ServerInfo> {
        self.server_order
            .get(self.selected_server)
            .and_then(|id| self.servers.get(id))
    }

    /// Get the currently selected server's EndpointId
    pub fn selected_server_id(&self) -> Option<EndpointId> {
        self.server_order.get(self.selected_server).copied()
    }

    /// Get devices for the selected server
    pub fn selected_server_devices(&self) -> Option<&Vec<RemoteDevice>> {
        self.selected_server_id()
            .and_then(|id| self.devices.get(&id))
    }

    /// Get the currently selected device
    pub fn selected_device(&self) -> Option<&RemoteDevice> {
        let server_id = self.selected_server_id()?;
        let devices = self.devices.get(&server_id)?;
        let idx = self.selected_device.get(&server_id).copied().unwrap_or(0);
        devices.get(idx)
    }

    /// Get selected device index for current server
    pub fn selected_device_index(&self) -> usize {
        self.selected_server_id()
            .and_then(|id| self.selected_device.get(&id))
            .copied()
            .unwrap_or(0)
    }

    /// Switch active pane
    pub fn toggle_pane(&mut self) {
        self.active_pane = match self.active_pane {
            ActivePane::Servers => ActivePane::Devices,
            ActivePane::Devices => ActivePane::Servers,
        };
    }

    /// Navigate up in current list
    pub fn navigate_up(&mut self) {
        match self.active_pane {
            ActivePane::Servers => {
                if self.selected_server > 0 {
                    self.selected_server -= 1;
                }
            }
            ActivePane::Devices => {
                if let Some(server_id) = self.selected_server_id() {
                    let idx = self.selected_device.entry(server_id).or_insert(0);
                    if *idx > 0 {
                        *idx -= 1;
                    }
                }
            }
        }
    }

    /// Navigate down in current list
    pub fn navigate_down(&mut self) {
        match self.active_pane {
            ActivePane::Servers => {
                if !self.server_order.is_empty()
                    && self.selected_server < self.server_order.len() - 1
                {
                    self.selected_server += 1;
                }
            }
            ActivePane::Devices => {
                if let Some(server_id) = self.selected_server_id() {
                    if let Some(devices) = self.devices.get(&server_id) {
                        let idx = self.selected_device.entry(server_id).or_insert(0);
                        if !devices.is_empty() && *idx < devices.len() - 1 {
                            *idx += 1;
                        }
                    }
                }
            }
        }
    }

    /// Handle Enter key press
    pub fn handle_enter(&mut self) -> AppAction {
        match self.active_pane {
            ActivePane::Servers => {
                if let Some(server) = self.selected_server() {
                    let endpoint_id = server.endpoint_id;
                    match server.status {
                        ServerStatus::Disconnected | ServerStatus::Failed => {
                            return AppAction::ConnectServer(endpoint_id);
                        }
                        ServerStatus::Connected => {
                            // Switch to device pane
                            self.active_pane = ActivePane::Devices;
                        }
                        ServerStatus::Connecting => {}
                    }
                }
            }
            ActivePane::Devices => {
                if let (Some(server_id), Some(device)) =
                    (self.selected_server_id(), self.selected_device())
                {
                    match device.status {
                        DeviceStatus::Available => {
                            return AppAction::AttachDevice(server_id, device.info.id);
                        }
                        DeviceStatus::Attached => {
                            if let Some(handle) = device.handle {
                                return AppAction::DetachDevice(server_id, handle);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        AppAction::None
    }

    /// Handle 'd' key (disconnect/detach)
    pub fn handle_disconnect(&mut self) -> AppAction {
        match self.active_pane {
            ActivePane::Servers => {
                if let Some(server) = self.selected_server() {
                    if server.status == ServerStatus::Connected {
                        return AppAction::DisconnectServer(server.endpoint_id);
                    }
                }
            }
            ActivePane::Devices => {
                if let (Some(server_id), Some(device)) =
                    (self.selected_server_id(), self.selected_device())
                {
                    if let (DeviceStatus::Attached, Some(handle)) = (device.status, device.handle) {
                        return AppAction::DetachDevice(server_id, handle);
                    }
                }
            }
        }
        AppAction::None
    }

    /// Handle 'r' key (refresh)
    pub fn handle_refresh(&mut self) -> AppAction {
        if let Some(server) = self.selected_server() {
            if server.status == ServerStatus::Connected {
                return AppAction::RefreshDevices(server.endpoint_id);
            }
        }
        AppAction::None
    }

    /// Handle 'a' key (add server)
    pub fn start_add_server(&mut self) {
        self.input_mode = InputMode::AddServer {
            input: String::new(),
        };
    }

    /// Handle input in AddServer mode
    pub fn handle_add_server_input(&mut self, c: char) {
        if let InputMode::AddServer { input } = &mut self.input_mode {
            input.push(c);
        }
    }

    /// Handle backspace in AddServer mode
    pub fn handle_add_server_backspace(&mut self) {
        if let InputMode::AddServer { input } = &mut self.input_mode {
            input.pop();
        }
    }

    /// Confirm adding server
    pub fn confirm_add_server(&mut self) -> AppAction {
        if let InputMode::AddServer { input } = &self.input_mode {
            let server_str = input.clone();
            self.input_mode = InputMode::Normal;
            if !server_str.is_empty() {
                return AppAction::AddServer(server_str);
            }
        }
        AppAction::None
    }

    /// Cancel current input mode
    pub fn cancel_input(&mut self) {
        self.input_mode = InputMode::Normal;
    }

    /// Show help overlay
    pub fn show_help(&mut self) {
        self.input_mode = InputMode::Help;
    }

    /// Show quit confirmation
    pub fn show_quit_confirm(&mut self) {
        self.input_mode = InputMode::ConfirmQuit;
    }

    /// Confirm quit
    pub fn confirm_quit(&mut self) {
        self.should_quit = true;
    }

    /// Set status message
    pub fn set_status(&mut self, message: String) {
        self.status_message = Some(message);
    }

    /// Clear status message
    #[allow(dead_code)]
    pub fn clear_status(&mut self) {
        self.status_message = None;
    }

    /// Count connected servers
    pub fn connected_server_count(&self) -> usize {
        self.servers
            .values()
            .filter(|s| s.status == ServerStatus::Connected)
            .count()
    }

    /// Count attached devices
    pub fn attached_device_count(&self) -> usize {
        self.devices
            .values()
            .flat_map(|devices| devices.iter())
            .filter(|d| d.status == DeviceStatus::Attached)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::SecretKey;

    fn mock_endpoint_id() -> EndpointId {
        // Create a valid mock EndpointId for testing using SecretKey
        SecretKey::generate(&mut rand::rng()).public()
    }

    #[test]
    fn test_app_creation() {
        let endpoint_id = mock_endpoint_id();
        let app = App::new(endpoint_id);

        assert!(app.servers.is_empty());
        assert_eq!(app.active_pane, ActivePane::Servers);
        assert!(!app.should_quit);
    }

    #[test]
    fn test_add_server() {
        let endpoint_id = mock_endpoint_id();
        let mut app = App::new(endpoint_id);

        let server_id = mock_endpoint_id();
        app.add_server(server_id, Some("Test Server".to_string()));

        assert_eq!(app.servers.len(), 1);
        assert_eq!(app.server_order.len(), 1);

        let server = app.servers.get(&server_id).unwrap();
        assert_eq!(server.status, ServerStatus::Disconnected);
        assert_eq!(server.name, Some("Test Server".to_string()));
    }

    #[test]
    fn test_navigate() {
        let endpoint_id = mock_endpoint_id();
        let mut app = App::new(endpoint_id);

        // Add multiple servers using valid EndpointIds
        for _ in 0..3 {
            let server_id = mock_endpoint_id();
            app.add_server(server_id, None);
        }

        assert_eq!(app.selected_server, 0);

        app.navigate_down();
        assert_eq!(app.selected_server, 1);

        app.navigate_down();
        assert_eq!(app.selected_server, 2);

        // Should not go beyond last item
        app.navigate_down();
        assert_eq!(app.selected_server, 2);

        app.navigate_up();
        assert_eq!(app.selected_server, 1);
    }

    #[test]
    fn test_toggle_pane() {
        let endpoint_id = mock_endpoint_id();
        let mut app = App::new(endpoint_id);

        assert_eq!(app.active_pane, ActivePane::Servers);

        app.toggle_pane();
        assert_eq!(app.active_pane, ActivePane::Devices);

        app.toggle_pane();
        assert_eq!(app.active_pane, ActivePane::Servers);
    }
}
