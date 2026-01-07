//! TUI event handling
//!
//! Handles terminal events (keyboard, mouse, resize) using crossterm.
//! Provides an async event stream that integrates with tokio.

use crossterm::event::{self, Event as CrosstermEvent, KeyCode, KeyEvent, KeyModifiers};
use std::time::Duration;
use tokio::sync::mpsc;

/// Terminal event types
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum Event {
    /// Keyboard input event
    Key(KeyEvent),
    /// Terminal resize event
    Resize(u16, u16),
    /// Tick event for periodic UI updates
    Tick,
}

/// User actions derived from keyboard input
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Quit the application
    Quit,
    /// Move selection up
    Up,
    /// Move selection down
    Down,
    /// Toggle sharing for selected device
    ToggleSharing,
    /// View details of selected device
    ViewDetails,
    /// View connected clients
    ViewClients,
    /// Show help dialog
    ShowHelp,
    /// Close dialog/popup
    CloseDialog,
    /// Refresh device list
    Refresh,
    /// Reset selected device
    ResetDevice,
    /// Confirm action (Enter/y)
    Confirm,
    /// No action
    None,
}

impl From<KeyEvent> for Action {
    fn from(key: KeyEvent) -> Self {
        match key.code {
            // Quit
            KeyCode::Char('q') => Action::Quit,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Action::Quit,
            KeyCode::Esc => Action::CloseDialog,

            // Navigation
            KeyCode::Up | KeyCode::Char('k') => Action::Up,
            KeyCode::Down | KeyCode::Char('j') => Action::Down,

            // Actions
            KeyCode::Char(' ') => Action::ToggleSharing,
            KeyCode::Enter => {
                // Enter is overloaded: ViewDetails or Confirm depending on context
                // We'll map to Confirm here if we can context-switch, or just keep generic ViewDetails
                // and let App logic decide. But Action::ViewDetails is for opening details.
                // Let's assume App handles "Enter" as "ViewDetails" normally, but if dialog is open?
                // Actually, the App::handle_action logic handles context.
                // But we need a distinction between "Open Details" and "Confirm Dialog".
                // Let's use a generic "Enter" action or similar?
                // The current code maps Enter -> ViewDetails.
                // Let's change ViewDetails to also mean Confirm in dialog context.
                Action::ViewDetails
            }
            KeyCode::Char('y') => Action::Confirm,
            KeyCode::Char('c') => Action::ViewClients,
            KeyCode::Char('?') => Action::ShowHelp,
            KeyCode::Char('r') => Action::Refresh,
            KeyCode::Char('R') => Action::ResetDevice,

            _ => Action::None,
        }
    }
}

/// Event handler that polls terminal events in a background task
pub struct EventHandler {
    /// Receiver for events
    rx: mpsc::UnboundedReceiver<Event>,
}

impl EventHandler {
    /// Create a new event handler
    ///
    /// Spawns a background task that polls for terminal events
    /// and sends them through the channel.
    pub fn new(tick_rate: Duration) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();

        // Spawn event polling task
        tokio::spawn(async move {
            let mut last_tick = std::time::Instant::now();

            loop {
                // Calculate timeout until next tick
                let timeout = tick_rate
                    .checked_sub(last_tick.elapsed())
                    .unwrap_or(Duration::ZERO);

                // Poll for events with timeout
                if crossterm::event::poll(timeout).unwrap_or(false) {
                    match event::read() {
                        Ok(CrosstermEvent::Key(key)) => {
                            // Ignore key release events on some platforms
                            if key.kind == crossterm::event::KeyEventKind::Press {
                                if tx.send(Event::Key(key)).is_err() {
                                    break;
                                }
                            }
                        }
                        Ok(CrosstermEvent::Resize(width, height)) => {
                            if tx.send(Event::Resize(width, height)).is_err() {
                                break;
                            }
                        }
                        Ok(_) => {} // Ignore other events (mouse, focus, paste)
                        Err(_) => break,
                    }
                }

                // Send tick event if enough time has passed
                if last_tick.elapsed() >= tick_rate {
                    if tx.send(Event::Tick).is_err() {
                        break;
                    }
                    last_tick = std::time::Instant::now();
                }
            }
        });

        Self { rx }
    }

    /// Receive the next event
    ///
    /// Returns None if the event channel is closed.
    pub async fn next(&mut self) -> Option<Event> {
        self.rx.recv().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_from_key_quit() {
        let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert_eq!(Action::from(key), Action::Quit);
    }

    #[test]
    fn test_action_from_key_navigation() {
        let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(Action::from(up), Action::Up);

        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(Action::from(down), Action::Down);

        let k = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE);
        assert_eq!(Action::from(k), Action::Up);

        let j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(Action::from(j), Action::Down);
    }

    #[test]
    fn test_action_from_key_actions() {
        let space = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
        assert_eq!(Action::from(space), Action::ToggleSharing);

        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(Action::from(enter), Action::ViewDetails);

        let help = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE);
        assert_eq!(Action::from(help), Action::ShowHelp);
    }

    #[test]
    fn test_action_ctrl_c_quit() {
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(Action::from(ctrl_c), Action::Quit);
    }
}
