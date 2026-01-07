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

use super::app::{ActivePane, App, DeviceStatus, InputMode, ServerStatus, ToastType};
use super::qr;
use crate::network::{ConnectionQuality, HealthState};

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

    // Toast notification colors
    pub const TOAST_INFO_BG: Color = Color::Blue;
    pub const TOAST_SUCCESS_BG: Color = Color::Green;
    pub const TOAST_WARNING_BG: Color = Color::Yellow;
    pub const TOAST_ERROR_BG: Color = Color::Red;

    // Device list changed indicator
    pub const CHANGED_INDICATOR: Color = Color::Magenta;

    // Connection health quality colors
    pub const QUALITY_GOOD: Color = Color::Green;
    pub const QUALITY_FAIR: Color = Color::Yellow;
    pub const QUALITY_POOR: Color = Color::Red;
    pub const QUALITY_UNKNOWN: Color = Color::Gray;

    // Health state colors
    pub const HEALTH_DEGRADED: Color = Color::Yellow;
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
        InputMode::QrCode => {
            render_qr_code_dialog(frame, app);
        }
        InputMode::Normal => {}
    }

    // Render toast notifications (in top-right corner)
    render_toasts(frame, app);
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

/// Render the main content area with optional metrics panel
fn render_main_content(frame: &mut Frame, app: &App, area: Rect) {
    // Split vertically: top for server/device lists, bottom for metrics
    let vertical_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(70), // Server + Device lists
            Constraint::Percentage(30), // Metrics panel
        ])
        .split(area);

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(40), // Server list
            Constraint::Percentage(60), // Device list
        ])
        .split(vertical_chunks[0]);

    render_server_list(frame, app, chunks[0]);
    render_device_list(frame, app, chunks[1]);
    render_metrics_panel(frame, app, vertical_chunks[1]);
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

            let device_count = format!(" [{} dev]", server.device_count);

            // Build health indicator string
            let health_info = if server.status == ServerStatus::Connected {
                if let Some(ref health) = server.health {
                    let (quality_icon, quality_color) = match health.quality {
                        ConnectionQuality::Good => ("G", colors::QUALITY_GOOD),
                        ConnectionQuality::Fair => ("F", colors::QUALITY_FAIR),
                        ConnectionQuality::Poor => ("P", colors::QUALITY_POOR),
                        ConnectionQuality::Unknown => ("?", colors::QUALITY_UNKNOWN),
                    };

                    let rtt_str = health
                        .average_rtt_ms
                        .map(|rtt| format!("{}ms", rtt))
                        .unwrap_or_else(|| "-".to_string());

                    // Show degraded state warning
                    let state_warning = if health.state == HealthState::Degraded {
                        "!"
                    } else {
                        ""
                    };

                    Some((
                        format!(" [{}{}|{}]", quality_icon, state_warning, rtt_str),
                        quality_color,
                    ))
                } else {
                    None
                }
            } else {
                None
            };

            let mut spans = vec![
                Span::styled(
                    format!("{} ", status_icon),
                    Style::default().fg(status_color),
                ),
                Span::raw(name_display),
                Span::styled(device_count, Style::default().fg(Color::DarkGray)),
            ];

            // Add health indicator if available
            if let Some((health_str, health_color)) = health_info {
                spans.push(Span::styled(health_str, Style::default().fg(health_color)));
            }

            let line = Line::from(spans);

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

/// Render the metrics panel
fn render_metrics_panel(frame: &mut Frame, app: &App, area: Rect) {
    use common::MetricsSnapshot;

    // Get metrics for the selected server or aggregated metrics
    let (title, metrics) = if let Some(server_metrics) = app.selected_server_metrics() {
        let server_name = app
            .selected_server()
            .and_then(|s| s.name.clone())
            .unwrap_or_else(|| "Selected Server".to_string());
        (format!(" Metrics: {} ", server_name), server_metrics)
    } else {
        (
            " Metrics: All Servers ".to_string(),
            app.aggregated_metrics(),
        )
    };

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
            Span::styled("Sent: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                metrics.format_bytes_sent(),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(vec![
            Span::styled("Received: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                metrics.format_bytes_received(),
                Style::default().fg(Color::White),
            ),
        ]),
    ];

    let transfer_block = Paragraph::new(transfer_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(colors::INACTIVE_BORDER))
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
            .border_style(Style::default().fg(colors::INACTIVE_BORDER))
            .title(" Latency ")
            .title_style(Style::default().fg(Color::White)),
    );
    frame.render_widget(latency_block, chunks[1]);

    // Throughput statistics
    let throughput_lines = vec![
        Line::from(vec![
            Span::styled("TX: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                metrics.format_throughput_tx(),
                Style::default().fg(Color::Cyan),
            ),
        ]),
        Line::from(vec![
            Span::styled("RX: ", Style::default().fg(Color::DarkGray)),
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
            .border_style(Style::default().fg(colors::INACTIVE_BORDER))
            .title(" Throughput ")
            .title_style(Style::default().fg(Color::White)),
    );
    frame.render_widget(throughput_block, chunks[2]);

    // Connection quality
    let quality = metrics.connection_quality();
    let quality_label = metrics.connection_quality_label();
    let quality_color = match quality {
        90..=100 => colors::QUALITY_GOOD,
        70..=89 => Color::LightGreen,
        50..=69 => colors::QUALITY_FAIR,
        30..=49 => Color::LightRed,
        _ => colors::QUALITY_POOR,
    };

    // Create a simple bar graph for quality
    let bar_filled = (quality as usize * 10) / 100;
    let bar_empty = 10 - bar_filled;
    let quality_bar = format!("[{}{}]", "=".repeat(bar_filled), " ".repeat(bar_empty));

    let uptime_str = metrics.format_uptime();

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
            Span::styled("Uptime: ", Style::default().fg(Color::DarkGray)),
            Span::styled(uptime_str, Style::default().fg(Color::White)),
        ]),
    ];

    let quality_block = Paragraph::new(quality_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(colors::INACTIVE_BORDER))
            .title(" Connection ")
            .title_style(Style::default().fg(Color::White)),
    );
    frame.render_widget(quality_block, chunks[3]);
}

/// Render the bottom help bar
fn render_help_bar(frame: &mut Frame, app: &App, area: Rect) {
    let help_text = match &app.input_mode {
        InputMode::Normal => {
            if app.active_pane == ActivePane::Servers {
                "Tab: Switch | j/k: Navigate | Enter: Connect | d: Disconnect | a: Add | Q: QR | q: Quit | ?: Help"
            } else {
                "Tab: Switch | j/k: Navigate | Enter: Attach/Detach | d: Detach | r: Refresh | Q: QR | q: Quit | ?: Help"
            }
        }
        InputMode::AddServer { .. } => "Enter: Confirm | Esc: Cancel",
        InputMode::Help => "Press any key to close",
        InputMode::ConfirmQuit => "y: Quit | n: Cancel",
        InputMode::QrCode => "Press Esc or Q to close",
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
        Line::from("  Q            Show QR code (for server approval)"),
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

/// Render the QR code dialog for client EndpointId
fn render_qr_code_dialog(frame: &mut Frame, app: &App) {
    // Calculate QR code dimensions to size the dialog appropriately
    let (qr_width, qr_height) = qr::calculate_qr_dimensions(&app.client_endpoint_id);

    // Add padding for border, title, and endpoint text
    let dialog_width = (qr_width + 4).max(50) as u16;
    let dialog_height = (qr_height + 10) as u16;

    // Calculate percentages based on terminal size
    let term_area = frame.area();
    let percent_x = ((dialog_width as u32 * 100) / term_area.width as u32).min(90) as u16;
    let percent_y = ((dialog_height as u32 * 100) / term_area.height as u32).min(90) as u16;

    let area = centered_rect(percent_x.max(50), percent_y.max(50), term_area);

    let endpoint_id = &app.client_endpoint_id;
    let endpoint_str = endpoint_id.to_string();
    let connection_url = qr::generate_connection_url(endpoint_id);

    // Generate QR code lines
    let qr_lines = qr::generate_qr_lines(endpoint_id);

    // Build content: header, QR code, and footer with endpoint info
    let mut content_lines = Vec::new();

    // Header
    content_lines.push(Line::from(vec![Span::styled(
        "Share this QR code with the server",
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
                .title(" QR Code - Client EndpointId ")
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, area);
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

/// Render toast notifications in the top-right corner
fn render_toasts(frame: &mut Frame, app: &App) {
    if app.toasts.is_empty() {
        return;
    }

    let area = frame.area();
    let toast_width = 40u16.min(area.width.saturating_sub(4));
    let toast_height = 1u16;
    let margin = 2u16;

    // Start from top-right, below status bar
    let start_x = area.width.saturating_sub(toast_width + margin);
    let start_y = 4u16; // Below status bar

    for (i, toast) in app.toasts.iter().enumerate().take(5) {
        let y = start_y + (i as u16) * (toast_height + 1);
        if y + toast_height > area.height.saturating_sub(2) {
            break; // No room for more toasts
        }

        let toast_area = Rect::new(start_x, y, toast_width, toast_height);

        let (bg_color, fg_color) = match toast.toast_type {
            ToastType::Info => (colors::TOAST_INFO_BG, Color::White),
            ToastType::Success => (colors::TOAST_SUCCESS_BG, Color::Black),
            ToastType::Warning => (colors::TOAST_WARNING_BG, Color::Black),
            ToastType::Error => (colors::TOAST_ERROR_BG, Color::White),
        };

        let icon = match toast.toast_type {
            ToastType::Info => "i",
            ToastType::Success => "+",
            ToastType::Warning => "!",
            ToastType::Error => "X",
        };

        // Truncate message if too long
        let max_msg_len = toast_width.saturating_sub(4) as usize;
        let msg = if toast.message.len() > max_msg_len {
            format!("{}...", &toast.message[..max_msg_len.saturating_sub(3)])
        } else {
            toast.message.clone()
        };

        let line = Line::from(vec![
            Span::styled(
                format!("[{}] ", icon),
                Style::default().fg(fg_color).bg(bg_color),
            ),
            Span::styled(msg, Style::default().fg(fg_color).bg(bg_color)),
        ]);

        let paragraph = Paragraph::new(line).style(Style::default().bg(bg_color));

        frame.render_widget(paragraph, toast_area);
    }
}
