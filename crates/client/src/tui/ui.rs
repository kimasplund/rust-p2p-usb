//! TUI rendering with ratatui
//!
//! Renders the terminal user interface using ratatui widgets and layouts.

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

use super::app::{ActivePane, App, DeviceStatus, InputMode, ServerStatus};

/// Colors used in the UI
mod colors {
    use ratatui::style::Color;

    pub const CONNECTED: Color = Color::Green;
    pub const CONNECTING: Color = Color::Yellow;
    pub const DISCONNECTED: Color = Color::Red;
    pub const FAILED: Color = Color::Red;

    pub const ATTACHED: Color = Color::Green;
    pub const AVAILABLE: Color = Color::White;
    pub const BUSY: Color = Color::Yellow;
    pub const ATTACHING: Color = Color::Yellow;

    pub const ACTIVE_BORDER: Color = Color::Cyan;
    pub const INACTIVE_BORDER: Color = Color::Gray;

    pub const HIGHLIGHT_BG: Color = Color::DarkGray;
    pub const STATUS_BAR_BG: Color = Color::Blue;
    pub const HELP_BAR_BG: Color = Color::DarkGray;
}

/// Render the complete UI
pub fn render(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Status bar
            Constraint::Min(10),   // Main content (two panes)
            Constraint::Length(1), // Help bar
        ])
        .split(frame.area());

    render_status_bar(frame, app, chunks[0]);
    render_main_content(frame, app, chunks[1]);
    render_help_bar(frame, app, chunks[2]);

    // Render overlays based on input mode
    match &app.input_mode {
        InputMode::AddServer { input } => {
            render_add_server_dialog(frame, input);
        }
        InputMode::Help => {
            render_help_overlay(frame);
        }
        InputMode::ConfirmQuit => {
            render_quit_dialog(frame);
        }
        InputMode::Normal => {}
    }
}

/// Render the top status bar
fn render_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let endpoint_str = format!("{}", app.client_endpoint_id);
    let short_id = if endpoint_str.len() > 16 {
        format!("{}...", &endpoint_str[..16])
    } else {
        endpoint_str
    };

    let connected_count = app.connected_server_count();
    let attached_count = app.attached_device_count();

    let status_text = format!(
        " Client: {} | Servers: {} connected | Devices: {} attached",
        short_id, connected_count, attached_count
    );

    let status_message = app
        .status_message
        .as_ref()
        .map(|m| format!(" | {}", m))
        .unwrap_or_default();

    let paragraph = Paragraph::new(Line::from(vec![
        Span::styled(status_text, Style::default().fg(Color::White)),
        Span::styled(status_message, Style::default().fg(Color::Yellow)),
    ]))
    .style(Style::default().bg(colors::STATUS_BAR_BG))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" P2P USB Client ")
            .title_style(Style::default().add_modifier(Modifier::BOLD)),
    );

    frame.render_widget(paragraph, area);
}

/// Render the main two-pane content area
fn render_main_content(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(40), // Server list
            Constraint::Percentage(60), // Device list
        ])
        .split(area);

    render_server_list(frame, app, chunks[0]);
    render_device_list(frame, app, chunks[1]);
}

/// Render the server list pane
fn render_server_list(frame: &mut Frame, app: &App, area: Rect) {
    let is_active = app.active_pane == ActivePane::Servers;
    let border_color = if is_active {
        colors::ACTIVE_BORDER
    } else {
        colors::INACTIVE_BORDER
    };

    let items: Vec<ListItem> = app
        .server_order
        .iter()
        .filter_map(|id| app.servers.get(id))
        .map(|server| {
            let (status_icon, status_color) = match server.status {
                ServerStatus::Connected => ("[*]", colors::CONNECTED),
                ServerStatus::Connecting => ("[~]", colors::CONNECTING),
                ServerStatus::Disconnected => ("[ ]", colors::DISCONNECTED),
                ServerStatus::Failed => ("[!]", colors::FAILED),
            };

            let endpoint_str = format!("{}", server.endpoint_id);
            let short_id = if endpoint_str.len() > 20 {
                format!("{}...", &endpoint_str[..20])
            } else {
                endpoint_str
            };

            let name_display = server
                .name
                .as_ref()
                .map(|n| format!("{} ({})", n, short_id))
                .unwrap_or(short_id);

            let device_count = format!(" [{} devices]", server.device_count);

            let line = Line::from(vec![
                Span::styled(
                    format!("{} ", status_icon),
                    Style::default().fg(status_color),
                ),
                Span::raw(name_display),
                Span::styled(device_count, Style::default().fg(Color::DarkGray)),
            ]);

            ListItem::new(line)
        })
        .collect();

    let title = if is_active {
        " Servers (active) "
    } else {
        " Servers "
    };

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color))
                .title(title)
                .title_style(if is_active {
                    Style::default()
                        .fg(colors::ACTIVE_BORDER)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                }),
        )
        .highlight_style(
            Style::default()
                .bg(colors::HIGHLIGHT_BG)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    let mut state = ListState::default();
    if !app.server_order.is_empty() {
        state.select(Some(app.selected_server));
    }

    frame.render_stateful_widget(list, area, &mut state);
}

/// Render the device list pane
fn render_device_list(frame: &mut Frame, app: &App, area: Rect) {
    let is_active = app.active_pane == ActivePane::Devices;
    let border_color = if is_active {
        colors::ACTIVE_BORDER
    } else {
        colors::INACTIVE_BORDER
    };

    let title = if is_active {
        " Devices (active) "
    } else {
        " Devices "
    };

    // Check if we have a selected server and its status
    let Some(server) = app.selected_server() else {
        let paragraph = Paragraph::new("No server selected")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(border_color))
                    .title(title),
            );
        frame.render_widget(paragraph, area);
        return;
    };

    if server.status != ServerStatus::Connected {
        let paragraph = Paragraph::new("Server not connected\nPress Enter to connect")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(border_color))
                    .title(title),
            );
        frame.render_widget(paragraph, area);
        return;
    }

    let Some(devices) = app.selected_server_devices() else {
        let paragraph = Paragraph::new("No devices available")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(border_color))
                    .title(title),
            );
        frame.render_widget(paragraph, area);
        return;
    };

    if devices.is_empty() {
        let paragraph = Paragraph::new("No devices available")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(border_color))
                    .title(title),
            );
        frame.render_widget(paragraph, area);
        return;
    }
    let items: Vec<ListItem> = devices
        .iter()
        .map(|device| {
            let (status_icon, status_color) = match device.status {
                DeviceStatus::Attached => ("[+]", colors::ATTACHED),
                DeviceStatus::Available => ("[ ]", colors::AVAILABLE),
                DeviceStatus::Busy => ("[#]", colors::BUSY),
                DeviceStatus::Attaching | DeviceStatus::Detaching => ("[~]", colors::ATTACHING),
            };

            let vid_pid = format!(
                "{:04x}:{:04x}",
                device.info.vendor_id, device.info.product_id
            );

            let name = device
                .info
                .product
                .as_ref()
                .map(|p| p.as_str())
                .unwrap_or("Unknown Device");

            let line = Line::from(vec![
                Span::styled(
                    format!("{} ", status_icon),
                    Style::default().fg(status_color),
                ),
                Span::styled(
                    format!("[{}] ", device.info.id.0),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(format!("{} ", vid_pid), Style::default().fg(Color::Cyan)),
                Span::raw(name),
            ]);

            ListItem::new(line)
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color))
                .title(title)
                .title_style(if is_active {
                    Style::default()
                        .fg(colors::ACTIVE_BORDER)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                }),
        )
        .highlight_style(
            Style::default()
                .bg(colors::HIGHLIGHT_BG)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    let mut state = ListState::default();
    state.select(Some(app.selected_device_index()));

    frame.render_stateful_widget(list, area, &mut state);
}

/// Render the bottom help bar
fn render_help_bar(frame: &mut Frame, app: &App, area: Rect) {
    let help_text = match &app.input_mode {
        InputMode::Normal => {
            if app.active_pane == ActivePane::Servers {
                "Tab: Switch pane | j/k: Navigate | Enter: Connect/View | d: Disconnect | a: Add | r: Refresh | q: Quit | ?: Help"
            } else {
                "Tab: Switch pane | j/k: Navigate | Enter: Attach/Detach | d: Detach | r: Refresh | q: Quit | ?: Help"
            }
        }
        InputMode::AddServer { .. } => "Enter: Confirm | Esc: Cancel",
        InputMode::Help => "Press any key to close",
        InputMode::ConfirmQuit => "y: Quit | n: Cancel",
    };

    let paragraph = Paragraph::new(help_text)
        .style(Style::default().fg(Color::White).bg(colors::HELP_BAR_BG))
        .alignment(Alignment::Center);

    frame.render_widget(paragraph, area);
}

/// Render the add server dialog
fn render_add_server_dialog(frame: &mut Frame, input: &str) {
    let area = centered_rect(60, 20, frame.area());

    // Clear the area first
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Add Server ")
        .title_style(Style::default().add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(colors::ACTIVE_BORDER));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(2), // Label
            Constraint::Length(3), // Input
            Constraint::Min(0),    // Spacing
        ])
        .split(inner);

    let label = Paragraph::new("Enter server EndpointId:").style(Style::default().fg(Color::White));
    frame.render_widget(label, chunks[0]);

    let input_text = format!("{}_", input); // Show cursor
    let input_widget = Paragraph::new(input_text)
        .style(Style::default().fg(Color::Cyan))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        );
    frame.render_widget(input_widget, chunks[1]);
}

/// Render the help overlay
fn render_help_overlay(frame: &mut Frame) {
    let area = centered_rect(70, 70, frame.area());

    // Clear the area first
    frame.render_widget(Clear, area);

    let help_text = Text::from(vec![
        Line::from(Span::styled(
            "Keyboard Shortcuts",
            Style::default()
                .add_modifier(Modifier::BOLD)
                .add_modifier(Modifier::UNDERLINED),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Navigation",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("  Tab          Switch between server and device pane"),
        Line::from("  Up / k       Move selection up"),
        Line::from("  Down / j     Move selection down"),
        Line::from(""),
        Line::from(Span::styled(
            "Server Actions",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("  Enter        Connect to selected server"),
        Line::from("  d            Disconnect from selected server"),
        Line::from("  a            Add new server by EndpointId"),
        Line::from("  r            Refresh device list"),
        Line::from(""),
        Line::from(Span::styled(
            "Device Actions",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("  Enter        Attach/Detach selected device"),
        Line::from("  d            Detach selected device"),
        Line::from(""),
        Line::from(Span::styled(
            "General",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("  ?            Show this help"),
        Line::from("  q            Quit (with confirmation)"),
        Line::from("  Ctrl+C       Quit immediately"),
        Line::from(""),
        Line::from(Span::styled(
            "Status Icons",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("  [*] Connected/Attached    [~] Connecting/Attaching"),
        Line::from("  [ ] Disconnected/Available    [!] Failed    [#] Busy"),
    ]);

    let paragraph = Paragraph::new(help_text)
        .block(
            Block::default()
                .title(" Help ")
                .title_style(Style::default().add_modifier(Modifier::BOLD))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(colors::ACTIVE_BORDER)),
        )
        .wrap(Wrap { trim: false })
        .alignment(Alignment::Left);

    frame.render_widget(paragraph, area);
}

/// Render the quit confirmation dialog
fn render_quit_dialog(frame: &mut Frame) {
    let area = centered_rect(40, 15, frame.area());

    // Clear the area first
    frame.render_widget(Clear, area);

    let text = Text::from(vec![
        Line::from(""),
        Line::from(Span::styled(
            "Are you sure you want to quit?",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("All virtual USB devices will be detached"),
        Line::from("and server connections will be closed."),
        Line::from(""),
        Line::from(vec![
            Span::styled("  [Y]es  ", Style::default().fg(Color::Green)),
            Span::styled("  [N]o  ", Style::default().fg(Color::Red)),
        ]),
    ]);

    let paragraph = Paragraph::new(text)
        .block(
            Block::default()
                .title(" Quit ")
                .title_style(Style::default().add_modifier(Modifier::BOLD))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .alignment(Alignment::Center);

    frame.render_widget(paragraph, area);
}

/// Helper function to create a centered rectangle
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_centered_rect() {
        let area = Rect::new(0, 0, 100, 50);
        let centered = centered_rect(50, 50, area);

        // Should be centered
        assert!(centered.x > 0);
        assert!(centered.y > 0);
        assert!(centered.x + centered.width < area.width);
        assert!(centered.y + centered.height < area.height);
    }
}
