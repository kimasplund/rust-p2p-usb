//! TUI rendering with ratatui
//!
//! Implements the visual layout and rendering for the server TUI.
//! Uses ratatui widgets for the device table, status bar, and dialogs.

use protocol::DeviceSpeed;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Wrap},
};
use std::time::Duration;

use super::app::{App, DeviceState, Dialog};

/// Main render function
///
/// Renders the complete UI based on current application state.
pub fn render(frame: &mut Frame, app: &App) {
    // Main layout: status bar (top), device list (center), help bar (bottom)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Status bar
            Constraint::Min(10),   // Device list
            Constraint::Length(3), // Help bar
        ])
        .split(frame.area());

    // Render each section
    render_status_bar(frame, app, chunks[0]);
    render_device_list(frame, app, chunks[1]);
    render_help_bar(frame, chunks[2]);

    // Render dialog on top if open
    match app.dialog() {
        Dialog::None => {}
        Dialog::Help => render_help_dialog(frame),
        Dialog::DeviceDetails => render_device_details_dialog(frame, app),
        Dialog::Clients => render_clients_dialog(frame, app),
        Dialog::ConfirmReset => render_confirm_reset_dialog(frame, app),
    }
}

/// Render the status bar (top panel)
fn render_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let uptime = format_duration(app.uptime());
    let endpoint_id = app.endpoint_id().to_string();
    // Truncate endpoint ID to fit (64 chars is full, show first 16 + "...")
    let endpoint_display = if endpoint_id.len() > 20 {
        format!("{}...", &endpoint_id[..16])
    } else {
        endpoint_id
    };

    let status_text = vec![
        Span::styled("EndpointId: ", Style::default().fg(Color::DarkGray)),
        Span::styled(endpoint_display, Style::default().fg(Color::Cyan)),
        Span::raw("  |  "),
        Span::styled("Connections: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{}", app.connection_count()),
            Style::default().fg(Color::Yellow),
        ),
        Span::raw("  |  "),
        Span::styled("Uptime: ", Style::default().fg(Color::DarkGray)),
        Span::styled(uptime, Style::default().fg(Color::Green)),
    ];

    let status = Paragraph::new(Line::from(status_text))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" P2P USB Server ")
                .title_alignment(Alignment::Center)
                .border_style(Style::default().fg(Color::Blue)),
        )
        .alignment(Alignment::Center);

    frame.render_widget(status, area);
}

/// Render the device list (center panel)
fn render_device_list(frame: &mut Frame, app: &App, area: Rect) {
    let devices = app.devices();

    // Table header
    let header_cells = ["ID", "VID:PID", "Name", "Status", "Clients"]
        .iter()
        .map(|h| {
            Cell::from(*h).style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
        });
    let header = Row::new(header_cells).height(1);

    // Table rows
    let rows: Vec<Row> = devices
        .iter()
        .enumerate()
        .map(|(idx, device)| {
            let is_selected = idx == app.selected_index();
            create_device_row(device, is_selected)
        })
        .collect();

    // Create table
    let table = Table::new(
        rows,
        [
            Constraint::Length(4),  // ID
            Constraint::Length(10), // VID:PID
            Constraint::Min(20),    // Name
            Constraint::Length(10), // Status
            Constraint::Length(8),  // Clients
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" USB Devices ({}) ", devices.len()))
            .border_style(Style::default().fg(Color::Blue)),
    )
    .row_highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );

    // Create table state for highlighting
    let mut state = TableState::default();
    if !devices.is_empty() {
        state.select(Some(app.selected_index()));
    }

    frame.render_stateful_widget(table, area, &mut state);
}

/// Create a table row for a device
fn create_device_row(device: &DeviceState, _is_selected: bool) -> Row<'static> {
    let info = &device.info;

    // Status styling
    let (status_text, status_style) = if device.shared {
        ("Shared", Style::default().fg(Color::Green))
    } else {
        ("Private", Style::default().fg(Color::Red))
    };

    // Device name (product or fallback)
    let name = info
        .product
        .clone()
        .unwrap_or_else(|| "Unknown Device".to_string());

    // Client count
    let client_count = device.clients.len();
    let client_style = if client_count > 0 {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let cells = vec![
        Cell::from(format!("{}", info.id.0)),
        Cell::from(format!("{:04x}:{:04x}", info.vendor_id, info.product_id)),
        Cell::from(name),
        Cell::from(status_text).style(status_style),
        Cell::from(format!("{}", client_count)).style(client_style),
    ];

    Row::new(cells)
}

/// Render the help bar (bottom panel)
fn render_help_bar(frame: &mut Frame, area: Rect) {
    let help_text = vec![
        Span::styled(
            "q",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" Quit  "),
        Span::styled(
            "j/k",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" Navigate  "),
        Span::styled(
            "Space",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" Toggle Share  "),
        Span::styled(
            "Enter",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" Details  "),
        Span::styled(
            "c",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" Clients  "),
        Span::styled(
            "R",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" Reset  "),
        Span::styled(
            "?",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" Help"),
    ];

    let help = Paragraph::new(Line::from(help_text))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .alignment(Alignment::Center);

    frame.render_widget(help, area);
}

/// Render the help dialog
fn render_help_dialog(frame: &mut Frame) {
    let area = centered_rect(60, 70, frame.area());

    let help_content = vec![
        Line::from(vec![Span::styled(
            "Navigation",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Up / k       ", Style::default().fg(Color::Cyan)),
            Span::raw("Move selection up"),
        ]),
        Line::from(vec![
            Span::styled("  Down / j     ", Style::default().fg(Color::Cyan)),
            Span::raw("Move selection down"),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Actions",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Space        ", Style::default().fg(Color::Cyan)),
            Span::raw("Toggle device sharing on/off"),
        ]),
        Line::from(vec![
            Span::styled("  Enter        ", Style::default().fg(Color::Cyan)),
            Span::raw("View device details"),
        ]),
        Line::from(vec![
            Span::styled("  c            ", Style::default().fg(Color::Cyan)),
            Span::raw("View connected clients"),
        ]),
        Line::from(vec![
            Span::styled("  r            ", Style::default().fg(Color::Cyan)),
            Span::raw("Refresh device list"),
        ]),
        Line::from(vec![
            Span::styled("  R            ", Style::default().fg(Color::Cyan)),
            Span::raw("Reset selected device"),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "General",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  ?            ", Style::default().fg(Color::Cyan)),
            Span::raw("Show this help"),
        ]),
        Line::from(vec![
            Span::styled("  Esc          ", Style::default().fg(Color::Cyan)),
            Span::raw("Close dialog"),
        ]),
        Line::from(vec![
            Span::styled("  q / Ctrl+C   ", Style::default().fg(Color::Cyan)),
            Span::raw("Quit application"),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Status Colors",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Shared       ", Style::default().fg(Color::Green)),
            Span::raw("Device available for clients"),
        ]),
        Line::from(vec![
            Span::styled("  Private      ", Style::default().fg(Color::Red)),
            Span::raw("Device not shared"),
        ]),
    ];

    let help_paragraph = Paragraph::new(help_content)
        .block(
            Block::default()
                .title(" Help ")
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .wrap(Wrap { trim: false });

    // Clear the area first
    frame.render_widget(Clear, area);
    frame.render_widget(help_paragraph, area);
}

/// Render the device details dialog
fn render_device_details_dialog(frame: &mut Frame, app: &App) {
    let area = centered_rect(50, 60, frame.area());

    let content = if let Some(device) = app.selected_device() {
        let info = &device.info;
        vec![
            Line::from(vec![
                Span::styled("Device ID:       ", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{}", info.id.0), Style::default().fg(Color::White)),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled("Vendor ID:       ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{:04x}", info.vendor_id),
                    Style::default().fg(Color::Cyan),
                ),
            ]),
            Line::from(vec![
                Span::styled("Product ID:      ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{:04x}", info.product_id),
                    Style::default().fg(Color::Cyan),
                ),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled("Manufacturer:    ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    info.manufacturer
                        .clone()
                        .unwrap_or_else(|| "Unknown".to_string()),
                    Style::default().fg(Color::White),
                ),
            ]),
            Line::from(vec![
                Span::styled("Product:         ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    info.product
                        .clone()
                        .unwrap_or_else(|| "Unknown".to_string()),
                    Style::default().fg(Color::White),
                ),
            ]),
            Line::from(vec![
                Span::styled("Serial Number:   ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    info.serial_number
                        .clone()
                        .unwrap_or_else(|| "N/A".to_string()),
                    Style::default().fg(Color::White),
                ),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled("Bus:             ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{}", info.bus_number),
                    Style::default().fg(Color::White),
                ),
            ]),
            Line::from(vec![
                Span::styled("Address:         ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{}", info.device_address),
                    Style::default().fg(Color::White),
                ),
            ]),
            Line::from(vec![
                Span::styled("Speed:           ", Style::default().fg(Color::DarkGray)),
                Span::styled(format_speed(info.speed), Style::default().fg(Color::Yellow)),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled("Class:           ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{:02x}", info.class),
                    Style::default().fg(Color::White),
                ),
            ]),
            Line::from(vec![
                Span::styled("Subclass:        ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{:02x}", info.subclass),
                    Style::default().fg(Color::White),
                ),
            ]),
            Line::from(vec![
                Span::styled("Protocol:        ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{:02x}", info.protocol),
                    Style::default().fg(Color::White),
                ),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled("Status:          ", Style::default().fg(Color::DarkGray)),
                if device.shared {
                    Span::styled("Shared", Style::default().fg(Color::Green))
                } else {
                    Span::styled("Private", Style::default().fg(Color::Red))
                },
            ]),
            Line::from(vec![
                Span::styled("Connected:       ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{} client(s)", device.clients.len()),
                    Style::default().fg(Color::Cyan),
                ),
            ]),
        ]
    } else {
        vec![Line::from("No device selected")]
    };

    let paragraph = Paragraph::new(content)
        .block(
            Block::default()
                .title(" Device Details ")
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, area);
    frame.render_widget(paragraph, area);
}

/// Render the connected clients dialog
fn render_clients_dialog(frame: &mut Frame, app: &App) {
    let area = centered_rect(60, 50, frame.area());

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Total Connections: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{}", app.connection_count()),
                Style::default().fg(Color::Yellow),
            ),
        ]),
        Line::from(""),
    ];

    // List devices with connected clients
    let devices = app.devices();
    let devices_with_clients: Vec<_> = devices
        .iter()
        .filter(|d| !d.clients.is_empty())
        .copied()
        .collect();

    if devices_with_clients.is_empty() {
        lines.push(Line::from(Span::styled(
            "No clients currently connected to devices",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for device in devices_with_clients {
            lines.push(Line::from(vec![Span::styled(
                format!(
                    "{} ({:04x}:{:04x})",
                    device.info.product.as_deref().unwrap_or("Unknown"),
                    device.info.vendor_id,
                    device.info.product_id
                ),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )]));

            for client in &device.clients {
                // Truncate client ID for display
                let display_id = if client.len() > 20 {
                    format!("{}...", &client[..16])
                } else {
                    client.clone()
                };
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(display_id, Style::default().fg(Color::White)),
                ]));
            }
            lines.push(Line::from(""));
        }
    }

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Connected Clients ")
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, area);
    frame.render_widget(paragraph, area);
}

/// Render the confirm reset dialog
fn render_confirm_reset_dialog(frame: &mut Frame, app: &App) {
    let area = centered_rect(40, 20, frame.area());

    let device_name = if let Some(device) = app.selected_device() {
        device
            .info
            .product
            .as_deref()
            .unwrap_or("Unknown Device")
            .to_string()
    } else {
        "Unknown Device".to_string()
    };

    let content = vec![
        Line::from(Span::styled(
            "Are you sure you want to reset this device?",
            Style::default().fg(Color::White),
        )),
        Line::from(""),
        Line::from(Span::styled(
            device_name,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("y / Enter", Style::default().fg(Color::Red)),
            Span::raw(" to confirm"),
        ]),
        Line::from(vec![
            Span::styled("Esc", Style::default().fg(Color::Green)),
            Span::raw(" to cancel"),
        ]),
    ];

    let paragraph = Paragraph::new(content)
        .block(
            Block::default()
                .title(" Confirm Reset ")
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Red)),
        )
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, area);
    frame.render_widget(paragraph, area);
}

/// Helper to create a centered rectangle
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

/// Format duration for display
fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    let secs = secs % 60;

    if hours > 0 {
        format!("{}h {}m {}s", hours, mins, secs)
    } else if mins > 0 {
        format!("{}m {}s", mins, secs)
    } else {
        format!("{}s", secs)
    }
}

/// Format USB speed for display
fn format_speed(speed: DeviceSpeed) -> String {
    match speed {
        DeviceSpeed::Low => "Low (1.5 Mbps)".to_string(),
        DeviceSpeed::Full => "Full (12 Mbps)".to_string(),
        DeviceSpeed::High => "High (480 Mbps)".to_string(),
        DeviceSpeed::Super => "Super (5 Gbps)".to_string(),
        DeviceSpeed::SuperPlus => "Super+ (10 Gbps)".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration_seconds() {
        assert_eq!(format_duration(Duration::from_secs(45)), "45s");
    }

    #[test]
    fn test_format_duration_minutes() {
        assert_eq!(format_duration(Duration::from_secs(125)), "2m 5s");
    }

    #[test]
    fn test_format_duration_hours() {
        assert_eq!(format_duration(Duration::from_secs(3725)), "1h 2m 5s");
    }

    #[test]
    fn test_format_speed() {
        assert_eq!(format_speed(DeviceSpeed::Low), "Low (1.5 Mbps)");
        assert_eq!(format_speed(DeviceSpeed::Full), "Full (12 Mbps)");
        assert_eq!(format_speed(DeviceSpeed::High), "High (480 Mbps)");
        assert_eq!(format_speed(DeviceSpeed::Super), "Super (5 Gbps)");
        assert_eq!(format_speed(DeviceSpeed::SuperPlus), "Super+ (10 Gbps)");
    }

    #[test]
    fn test_centered_rect() {
        let area = Rect::new(0, 0, 100, 50);
        let centered = centered_rect(50, 50, area);

        // Should be centered and half the size
        assert!(centered.x > 0);
        assert!(centered.y > 0);
        assert!(centered.width < 100);
        assert!(centered.height < 50);
    }
}
