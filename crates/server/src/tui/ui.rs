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
use super::qr;

/// Main render function
///
/// Renders the complete UI based on current application state.
pub fn render(frame: &mut Frame, app: &App) {
    // Main layout: status bar (top), device list + metrics (center), help bar (bottom)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Status bar
            Constraint::Min(10),   // Device list
            Constraint::Length(8), // Metrics panel
            Constraint::Length(3), // Help bar
        ])
        .split(frame.area());

    // Render each section
    render_status_bar(frame, app, chunks[0]);
    render_device_list(frame, app, chunks[1]);
    render_metrics_panel(frame, app, chunks[2]);
    render_help_bar(frame, chunks[3]);

    // Render dialog on top if open
    match app.dialog() {
        Dialog::None => {}
        Dialog::Help => render_help_dialog(frame),
        Dialog::DeviceDetails => render_device_details_dialog(frame, app),
        Dialog::Clients => render_clients_dialog(frame, app),
        Dialog::ConfirmReset => render_confirm_reset_dialog(frame, app),
        Dialog::QrCode => render_qr_code_dialog(frame, app),
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

/// Render the metrics panel
fn render_metrics_panel(frame: &mut Frame, app: &App, area: Rect) {
    let metrics = app.total_metrics();

    // Split the metrics panel into columns
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25), // Transfer stats
            Constraint::Percentage(25), // Latency
            Constraint::Percentage(25), // Throughput
            Constraint::Percentage(25), // Connection quality
        ])
        .split(area);

    // Transfer statistics
    let transfer_lines = vec![
        Line::from(vec![
            Span::styled("Transfers: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{}", metrics.transfers_completed),
                Style::default().fg(Color::Green),
            ),
            Span::styled(" / ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{} failed", metrics.transfers_failed),
                Style::default().fg(if metrics.transfers_failed > 0 {
                    Color::Red
                } else {
                    Color::DarkGray
                }),
            ),
        ]),
        Line::from(vec![
            Span::styled("Active: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{}", metrics.active_transfers),
                Style::default().fg(Color::Cyan),
            ),
        ]),
        Line::from(vec![
            Span::styled("TX: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                metrics.format_bytes_sent(),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(vec![
            Span::styled("RX: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                metrics.format_bytes_received(),
                Style::default().fg(Color::White),
            ),
        ]),
    ];

    let transfer_block = Paragraph::new(transfer_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" Transfers ")
            .title_style(Style::default().fg(Color::White)),
    );
    frame.render_widget(transfer_block, chunks[0]);

    // Latency statistics
    let latency_color = if metrics.latency.avg_us > 50_000 {
        Color::Red
    } else if metrics.latency.avg_us > 20_000 {
        Color::Yellow
    } else {
        Color::Green
    };

    let latency_lines = vec![
        Line::from(vec![
            Span::styled("Avg: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                metrics.latency.format_avg(),
                Style::default().fg(latency_color),
            ),
        ]),
        Line::from(vec![
            Span::styled("Min: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                metrics.latency.format_min(),
                Style::default().fg(Color::Green),
            ),
        ]),
        Line::from(vec![
            Span::styled("Max: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                metrics.latency.format_max(),
                Style::default().fg(Color::Yellow),
            ),
        ]),
        Line::from(vec![
            Span::styled("Samples: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{}", metrics.latency.sample_count),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
    ];

    let latency_block = Paragraph::new(latency_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" Latency ")
            .title_style(Style::default().fg(Color::White)),
    );
    frame.render_widget(latency_block, chunks[1]);

    // Throughput statistics
    let throughput_lines = vec![
        Line::from(vec![
            Span::styled("TX Rate: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                metrics.format_throughput_tx(),
                Style::default().fg(Color::Cyan),
            ),
        ]),
        Line::from(vec![
            Span::styled("RX Rate: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                metrics.format_throughput_rx(),
                Style::default().fg(Color::Magenta),
            ),
        ]),
        Line::from(vec![
            Span::styled("Loss: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                metrics.format_loss_rate(),
                Style::default().fg(if metrics.loss_rate > 0.01 {
                    Color::Red
                } else {
                    Color::Green
                }),
            ),
        ]),
        Line::from(vec![
            Span::styled("Retries: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{}", metrics.retries),
                Style::default().fg(if metrics.retries > 0 {
                    Color::Yellow
                } else {
                    Color::DarkGray
                }),
            ),
        ]),
    ];

    let throughput_block = Paragraph::new(throughput_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" Throughput ")
            .title_style(Style::default().fg(Color::White)),
    );
    frame.render_widget(throughput_block, chunks[2]);

    // Connection quality/clients
    let quality = metrics.connection_quality();
    let quality_label = metrics.connection_quality_label();
    let quality_color = match quality {
        90..=100 => Color::Green,
        70..=89 => Color::LightGreen,
        50..=69 => Color::Yellow,
        30..=49 => Color::LightRed,
        _ => Color::Red,
    };

    let bar_filled = (quality as usize * 10) / 100;
    let bar_empty = 10 - bar_filled;
    let quality_bar = format!("[{}{}]", "=".repeat(bar_filled), " ".repeat(bar_empty));

    let quality_lines = vec![
        Line::from(vec![
            Span::styled("Quality: ", Style::default().fg(Color::DarkGray)),
            Span::styled(quality_label, Style::default().fg(quality_color)),
        ]),
        Line::from(vec![Span::styled(
            format!("{}%", quality),
            Style::default()
                .fg(quality_color)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![Span::styled(
            quality_bar,
            Style::default().fg(quality_color),
        )]),
        Line::from(vec![
            Span::styled("Clients: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{}", app.connection_count()),
                Style::default().fg(Color::Yellow),
            ),
        ]),
    ];

    let quality_block = Paragraph::new(quality_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" Status ")
            .title_style(Style::default().fg(Color::White)),
    );
    frame.render_widget(quality_block, chunks[3]);
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
        Span::raw(" Share  "),
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
            "Q",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" QR  "),
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
        Line::from(vec![
            Span::styled("  Q            ", Style::default().fg(Color::Cyan)),
            Span::raw("Show QR code for connection"),
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
    let area = centered_rect(55, 75, frame.area());

    let content = if let Some(device) = app.selected_device() {
        let info = &device.info;
        let device_id = app.selected_device_id();
        let device_metrics = device_id.and_then(|id| app.device_metrics(id));

        let mut lines = vec![
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
        ];

        // Add device metrics if available
        if let Some(metrics) = device_metrics {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![Span::styled(
                "Transfer Statistics",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )]));
            lines.push(Line::from(vec![
                Span::styled("TX:              ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    metrics.format_bytes_sent(),
                    Style::default().fg(Color::White),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("RX:              ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    metrics.format_bytes_received(),
                    Style::default().fg(Color::White),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Transfers:       ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{}", metrics.transfers_completed),
                    Style::default().fg(Color::Green),
                ),
                Span::raw(" / "),
                Span::styled(
                    format!("{} failed", metrics.transfers_failed),
                    Style::default().fg(if metrics.transfers_failed > 0 {
                        Color::Red
                    } else {
                        Color::DarkGray
                    }),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Latency (avg):   ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    metrics.latency.format_avg(),
                    Style::default().fg(Color::White),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Throughput TX:   ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    metrics.format_throughput_tx(),
                    Style::default().fg(Color::Cyan),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Throughput RX:   ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    metrics.format_throughput_rx(),
                    Style::default().fg(Color::Magenta),
                ),
            ]));
        }

        lines
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
    let area = centered_rect(70, 60, frame.area());

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

    // Show all clients with metrics
    let client_ids = app.client_ids_with_metrics();
    if client_ids.is_empty() {
        lines.push(Line::from(Span::styled(
            "No clients with metrics data",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        lines.push(Line::from(vec![Span::styled(
            "Client Metrics",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )]));
        lines.push(Line::from(""));

        for client_id in client_ids {
            // Truncate client ID for display
            let display_id = if client_id.len() > 20 {
                format!("{}...", &client_id[..16])
            } else {
                client_id.to_string()
            };

            lines.push(Line::from(vec![Span::styled(
                display_id,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )]));

            // Get metrics for this client
            if let Some(metrics) = app.client_metrics(client_id) {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled("TX: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        metrics.format_bytes_sent(),
                        Style::default().fg(Color::White),
                    ),
                    Span::raw("  "),
                    Span::styled("RX: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        metrics.format_bytes_received(),
                        Style::default().fg(Color::White),
                    ),
                ]));
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled("Transfers: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("{}", metrics.transfers_completed),
                        Style::default().fg(Color::Green),
                    ),
                    Span::raw(" / "),
                    Span::styled(
                        format!("{} failed", metrics.transfers_failed),
                        Style::default().fg(if metrics.transfers_failed > 0 {
                            Color::Red
                        } else {
                            Color::DarkGray
                        }),
                    ),
                ]));
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled("Latency: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        metrics.latency.format_avg(),
                        Style::default().fg(Color::White),
                    ),
                ]));
            }
            lines.push(Line::from(""));
        }
    }

    // Also show devices with connected clients (existing behavior)
    lines.push(Line::from(vec![Span::styled(
        "Devices with Clients",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(""));

    let devices = app.devices();
    let devices_with_clients: Vec<_> = devices
        .iter()
        .filter(|d| !d.clients.is_empty())
        .copied()
        .collect();

    if devices_with_clients.is_empty() {
        lines.push(Line::from(Span::styled(
            "No clients currently attached to devices",
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

/// Render the QR code dialog
fn render_qr_code_dialog(frame: &mut Frame, app: &App) {
    // Calculate QR code dimensions to size the dialog appropriately
    let (qr_width, qr_height) = qr::calculate_qr_dimensions(app.endpoint_id());

    // Add padding for border, title, and endpoint text
    let dialog_width = (qr_width + 4).max(50) as u16;
    let dialog_height = (qr_height + 10) as u16;

    // Calculate percentages based on terminal size
    let term_area = frame.area();
    let percent_x = ((dialog_width as u32 * 100) / term_area.width as u32).min(90) as u16;
    let percent_y = ((dialog_height as u32 * 100) / term_area.height as u32).min(90) as u16;

    let area = centered_rect(percent_x.max(50), percent_y.max(50), term_area);

    let endpoint_id = app.endpoint_id();
    let endpoint_str = endpoint_id.to_string();
    let connection_url = qr::generate_connection_url(endpoint_id);

    // Generate QR code lines
    let qr_lines = qr::generate_qr_lines(endpoint_id);

    // Build content: header, QR code, and footer with endpoint info
    let mut content_lines = Vec::new();

    // Header
    content_lines.push(Line::from(vec![Span::styled(
        "Scan to connect to this server",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )]));
    content_lines.push(Line::from(""));

    // QR Code lines (centered)
    for qr_line in qr_lines {
        content_lines.push(qr_line);
    }

    content_lines.push(Line::from(""));

    // Connection URL
    content_lines.push(Line::from(vec![
        Span::styled("URL: ", Style::default().fg(Color::DarkGray)),
        Span::styled(connection_url, Style::default().fg(Color::Cyan)),
    ]));

    // Full EndpointId (truncated if needed)
    let endpoint_display = if endpoint_str.len() > 50 {
        format!("{}...", &endpoint_str[..47])
    } else {
        endpoint_str.clone()
    };
    content_lines.push(Line::from(vec![
        Span::styled("EndpointId: ", Style::default().fg(Color::DarkGray)),
        Span::styled(endpoint_display, Style::default().fg(Color::Green)),
    ]));

    content_lines.push(Line::from(""));
    content_lines.push(Line::from(vec![Span::styled(
        "Press Esc or Q to close",
        Style::default().fg(Color::DarkGray),
    )]));

    let paragraph = Paragraph::new(content_lines)
        .block(
            Block::default()
                .title(" QR Code - Server Connection ")
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
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
