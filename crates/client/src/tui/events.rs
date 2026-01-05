//! TUI event handling
//!
//! Handles keyboard input using crossterm and dispatches actions to the application.

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use std::time::Duration;

use super::app::{App, AppAction, InputMode};

/// Event handler for TUI input
pub struct EventHandler {
    /// Tick rate for polling events
    tick_rate: Duration,
}

impl Default for EventHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl EventHandler {
    /// Create a new event handler
    pub fn new() -> Self {
        Self {
            tick_rate: Duration::from_millis(100),
        }
    }

    /// Create event handler with custom tick rate
    pub fn with_tick_rate(tick_rate: Duration) -> Self {
        Self { tick_rate }
    }

    /// Poll for next event
    ///
    /// Returns Some(Event) if an event occurred, None if tick timeout elapsed.
    pub fn poll(&self) -> Result<Option<Event>> {
        if event::poll(self.tick_rate)? {
            Ok(Some(event::read()?))
        } else {
            Ok(None)
        }
    }

    /// Handle a key event and return the resulting action
    pub fn handle_key(&self, app: &mut App, key: KeyEvent) -> AppAction {
        // Handle based on current input mode
        match &app.input_mode {
            InputMode::Normal => self.handle_normal_mode(app, key),
            InputMode::AddServer { .. } => self.handle_add_server_mode(app, key),
            InputMode::Help => self.handle_help_mode(app, key),
            InputMode::ConfirmQuit => self.handle_confirm_quit_mode(app, key),
        }
    }

    /// Handle key events in normal navigation mode
    fn handle_normal_mode(&self, app: &mut App, key: KeyEvent) -> AppAction {
        match key.code {
            // Quit
            KeyCode::Char('q') => {
                app.show_quit_confirm();
                AppAction::None
            }
            // Ctrl+C for immediate quit
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => AppAction::Quit,

            // Navigation
            KeyCode::Tab => {
                app.toggle_pane();
                AppAction::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.navigate_up();
                AppAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                app.navigate_down();
                AppAction::None
            }

            // Actions
            KeyCode::Enter => app.handle_enter(),
            KeyCode::Char('d') => app.handle_disconnect(),
            KeyCode::Char('r') => app.handle_refresh(),
            KeyCode::Char('a') => {
                app.start_add_server();
                AppAction::None
            }

            // Help
            KeyCode::Char('?') => {
                app.show_help();
                AppAction::None
            }

            _ => AppAction::None,
        }
    }

    /// Handle key events in add server mode
    fn handle_add_server_mode(&self, app: &mut App, key: KeyEvent) -> AppAction {
        match key.code {
            KeyCode::Esc => {
                app.cancel_input();
                AppAction::None
            }
            KeyCode::Enter => app.confirm_add_server(),
            KeyCode::Backspace => {
                app.handle_add_server_backspace();
                AppAction::None
            }
            KeyCode::Char(c) => {
                // Allow alphanumeric and common EndpointId characters
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    app.handle_add_server_input(c);
                }
                AppAction::None
            }
            _ => AppAction::None,
        }
    }

    /// Handle key events in help overlay mode
    fn handle_help_mode(&self, app: &mut App, key: KeyEvent) -> AppAction {
        match key.code {
            KeyCode::Esc | KeyCode::Char('?') | KeyCode::Enter | KeyCode::Char('q') => {
                app.cancel_input();
                AppAction::None
            }
            _ => AppAction::None,
        }
    }

    /// Handle key events in quit confirmation mode
    fn handle_confirm_quit_mode(&self, app: &mut App, key: KeyEvent) -> AppAction {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                app.confirm_quit();
                AppAction::Quit
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                app.cancel_input();
                AppAction::None
            }
            _ => AppAction::None,
        }
    }
}

/// Async event handler for use with tokio
pub struct AsyncEventHandler {
    /// Inner synchronous handler
    inner: EventHandler,
}

impl Default for AsyncEventHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl AsyncEventHandler {
    /// Create a new async event handler
    pub fn new() -> Self {
        Self {
            inner: EventHandler::new(),
        }
    }

    /// Create async event handler with custom tick rate
    pub fn with_tick_rate(tick_rate: Duration) -> Self {
        Self {
            inner: EventHandler::with_tick_rate(tick_rate),
        }
    }

    /// Poll for next event asynchronously
    ///
    /// This wraps the blocking poll in a spawn_blocking to avoid
    /// blocking the tokio runtime.
    pub async fn poll(&self) -> Result<Option<Event>> {
        let tick_rate = self.inner.tick_rate;

        // Use tokio::task::spawn_blocking for the blocking poll
        tokio::task::spawn_blocking(move || {
            if event::poll(tick_rate)? {
                Ok(Some(event::read()?))
            } else {
                Ok(None)
            }
        })
        .await?
    }

    /// Handle a key event and return the resulting action
    pub fn handle_key(&self, app: &mut App, key: KeyEvent) -> AppAction {
        self.inner.handle_key(app, key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::SecretKey;

    fn mock_endpoint_id() -> iroh::PublicKey {
        // Create a valid mock EndpointId for testing using SecretKey
        SecretKey::generate(&mut rand::rng()).public()
    }

    #[test]
    fn test_event_handler_creation() {
        let handler = EventHandler::new();
        assert_eq!(handler.tick_rate, Duration::from_millis(100));
    }

    #[test]
    fn test_navigation_keys() {
        let handler = EventHandler::new();
        let mut app = App::new(mock_endpoint_id());

        // Add some servers to navigate
        for _ in 0..3 {
            let server_id = mock_endpoint_id();
            app.add_server(server_id, None);
        }

        // Test down navigation
        let key = KeyEvent::new(KeyCode::Down, KeyModifiers::empty());
        let action = handler.handle_key(&mut app, key);
        assert!(matches!(action, AppAction::None));
        assert_eq!(app.selected_server, 1);

        // Test 'j' navigation
        let key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty());
        handler.handle_key(&mut app, key);
        assert_eq!(app.selected_server, 2);

        // Test up navigation
        let key = KeyEvent::new(KeyCode::Up, KeyModifiers::empty());
        handler.handle_key(&mut app, key);
        assert_eq!(app.selected_server, 1);

        // Test 'k' navigation
        let key = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::empty());
        handler.handle_key(&mut app, key);
        assert_eq!(app.selected_server, 0);
    }

    #[test]
    fn test_tab_toggle() {
        let handler = EventHandler::new();
        let mut app = App::new(mock_endpoint_id());

        use super::super::app::ActivePane;
        assert_eq!(app.active_pane, ActivePane::Servers);

        let key = KeyEvent::new(KeyCode::Tab, KeyModifiers::empty());
        handler.handle_key(&mut app, key);
        assert_eq!(app.active_pane, ActivePane::Devices);

        handler.handle_key(&mut app, key);
        assert_eq!(app.active_pane, ActivePane::Servers);
    }

    #[test]
    fn test_quit_confirmation() {
        let handler = EventHandler::new();
        let mut app = App::new(mock_endpoint_id());

        // Press 'q' to show quit confirmation
        let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::empty());
        handler.handle_key(&mut app, key);
        assert!(matches!(app.input_mode, InputMode::ConfirmQuit));

        // Press 'n' to cancel
        let key = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::empty());
        handler.handle_key(&mut app, key);
        assert!(matches!(app.input_mode, InputMode::Normal));
        assert!(!app.should_quit);

        // Press 'q' again
        let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::empty());
        handler.handle_key(&mut app, key);

        // Press 'y' to confirm
        let key = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::empty());
        let action = handler.handle_key(&mut app, key);
        assert!(matches!(action, AppAction::Quit));
        assert!(app.should_quit);
    }

    #[test]
    fn test_add_server_mode() {
        let handler = EventHandler::new();
        let mut app = App::new(mock_endpoint_id());

        // Press 'a' to start adding server
        let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty());
        handler.handle_key(&mut app, key);
        assert!(matches!(app.input_mode, InputMode::AddServer { .. }));

        // Type some characters
        let key = KeyEvent::new(KeyCode::Char('t'), KeyModifiers::empty());
        handler.handle_key(&mut app, key);
        let key = KeyEvent::new(KeyCode::Char('e'), KeyModifiers::empty());
        handler.handle_key(&mut app, key);
        let key = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::empty());
        handler.handle_key(&mut app, key);
        let key = KeyEvent::new(KeyCode::Char('t'), KeyModifiers::empty());
        handler.handle_key(&mut app, key);

        if let InputMode::AddServer { input } = &app.input_mode {
            assert_eq!(input, "test");
        } else {
            panic!("Expected AddServer mode");
        }

        // Press Escape to cancel
        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::empty());
        handler.handle_key(&mut app, key);
        assert!(matches!(app.input_mode, InputMode::Normal));
    }

    #[test]
    fn test_help_mode() {
        let handler = EventHandler::new();
        let mut app = App::new(mock_endpoint_id());

        // Press '?' to show help
        let key = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::empty());
        handler.handle_key(&mut app, key);
        assert!(matches!(app.input_mode, InputMode::Help));

        // Press '?' again to close
        handler.handle_key(&mut app, key);
        assert!(matches!(app.input_mode, InputMode::Normal));
    }

    #[test]
    fn test_ctrl_c_immediate_quit() {
        let handler = EventHandler::new();
        let mut app = App::new(mock_endpoint_id());

        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let action = handler.handle_key(&mut app, key);
        assert!(matches!(action, AppAction::Quit));
    }
}
