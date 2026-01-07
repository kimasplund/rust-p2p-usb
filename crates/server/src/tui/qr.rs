//! QR code generation and terminal rendering
//!
//! Generates QR codes for easy server connection sharing.
//! Uses Unicode block characters for compact terminal display.

use iroh::PublicKey as EndpointId;
use qrcode::{EcLevel, QrCode, Version};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// URL scheme for P2P USB connections
const URL_SCHEME: &str = "p2p-usb://connect/";

/// Generate connection URL from EndpointId
pub fn generate_connection_url(endpoint_id: &EndpointId) -> String {
    format!("{}{}", URL_SCHEME, endpoint_id)
}

/// Parse EndpointId from connection URL
///
/// Returns Some(endpoint_id_string) if the URL matches the expected format,
/// None otherwise.
pub fn parse_connection_url(url: &str) -> Option<&str> {
    url.strip_prefix(URL_SCHEME)
}

/// Generate QR code as lines of text for terminal display
///
/// Uses Unicode half-block characters for compact display:
/// - Upper half block (U+2580): represents top module black, bottom white
/// - Lower half block (U+2584): represents top module white, bottom black
/// - Full block (U+2588): both modules black
/// - Space: both modules white
///
/// This allows displaying 2 vertical modules per character row.
pub fn generate_qr_lines(endpoint_id: &EndpointId) -> Vec<Line<'static>> {
    let url = generate_connection_url(endpoint_id);

    // Generate QR code with auto-detected version and medium error correction
    let qr = match QrCode::with_error_correction_level(&url, EcLevel::M) {
        Ok(qr) => qr,
        Err(_) => {
            // Fallback: try with explicit version for longer data
            match QrCode::with_version(&url, Version::Normal(10), EcLevel::L) {
                Ok(qr) => qr,
                Err(_) => {
                    return vec![Line::from(Span::styled(
                        "Failed to generate QR code",
                        Style::default().fg(Color::Red),
                    ))];
                }
            }
        }
    };

    let modules = qr.to_colors();
    let width = qr.width();

    // Add quiet zone (2 modules on each side for compact display)
    let quiet_zone = 2;
    let total_width = width + (quiet_zone * 2);

    let mut lines = Vec::new();

    // Process two rows at a time for half-block rendering
    let mut row = 0;
    while row < width + (quiet_zone * 2) {
        let mut line_spans = Vec::new();

        for col in 0..total_width {
            // Check if we're in the quiet zone
            let in_qr_area_col = col >= quiet_zone && col < width + quiet_zone;
            let in_qr_area_row1 = row >= quiet_zone && row < width + quiet_zone;
            let in_qr_area_row2 = (row + 1) >= quiet_zone && (row + 1) < width + quiet_zone;

            // Get module states (true = black/dark, false = white/light)
            let top_black = if in_qr_area_col && in_qr_area_row1 {
                let qr_row = row - quiet_zone;
                let qr_col = col - quiet_zone;
                let idx = qr_row * width + qr_col;
                modules
                    .get(idx)
                    .map(|c| *c == qrcode::Color::Dark)
                    .unwrap_or(false)
            } else {
                false // Quiet zone is white
            };

            let bottom_black =
                if in_qr_area_col && in_qr_area_row2 && (row + 1) < width + (quiet_zone * 2) {
                    let qr_row = (row + 1) - quiet_zone;
                    let qr_col = col - quiet_zone;
                    let idx = qr_row * width + qr_col;
                    modules
                        .get(idx)
                        .map(|c| *c == qrcode::Color::Dark)
                        .unwrap_or(false)
                } else {
                    false // Quiet zone is white
                };

            // Choose character based on module states
            let ch = match (top_black, bottom_black) {
                (true, true) => '\u{2588}',  // Full block
                (true, false) => '\u{2580}', // Upper half block
                (false, true) => '\u{2584}', // Lower half block
                (false, false) => ' ',       // Space
            };

            line_spans.push(Span::raw(ch.to_string()));
        }

        lines.push(Line::from(line_spans));
        row += 2; // Move by 2 rows since we process pairs
    }

    lines
}

/// Calculate the display dimensions of the QR code
///
/// Returns (width, height) in characters.
/// Height is approximately half of width due to half-block rendering.
pub fn calculate_qr_dimensions(endpoint_id: &EndpointId) -> (usize, usize) {
    let url = generate_connection_url(endpoint_id);

    let qr = match QrCode::with_error_correction_level(&url, EcLevel::M) {
        Ok(qr) => qr,
        Err(_) => {
            match QrCode::with_version(&url, Version::Normal(10), EcLevel::L) {
                Ok(qr) => qr,
                Err(_) => return (30, 15), // Fallback dimensions
            }
        }
    };

    let width = qr.width();
    let quiet_zone = 2;
    let total_width = width + (quiet_zone * 2);
    let total_height = (width + (quiet_zone * 2) + 1) / 2; // Half due to half-block

    (total_width, total_height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_endpoint_id() -> EndpointId {
        EndpointId::from_bytes(&[0u8; 32]).unwrap()
    }

    #[test]
    fn test_generate_connection_url() {
        let endpoint_id = mock_endpoint_id();
        let url = generate_connection_url(&endpoint_id);

        assert!(url.starts_with(URL_SCHEME));
        assert!(url.len() > URL_SCHEME.len());
    }

    #[test]
    fn test_parse_connection_url() {
        let endpoint_id = mock_endpoint_id();
        let url = generate_connection_url(&endpoint_id);

        let parsed = parse_connection_url(&url);
        assert!(parsed.is_some());

        let invalid = parse_connection_url("http://example.com");
        assert!(invalid.is_none());
    }

    #[test]
    fn test_generate_qr_lines() {
        let endpoint_id = mock_endpoint_id();
        let lines = generate_qr_lines(&endpoint_id);

        // Should generate multiple lines
        assert!(!lines.is_empty());

        // Lines should have content
        for line in &lines {
            assert!(!line.spans.is_empty());
        }
    }

    #[test]
    fn test_calculate_qr_dimensions() {
        let endpoint_id = mock_endpoint_id();
        let (width, height) = calculate_qr_dimensions(&endpoint_id);

        // QR code should have reasonable dimensions
        assert!(width > 20);
        assert!(width < 60);
        assert!(height > 10);
        assert!(height < 40);

        // Height should be approximately half of width
        assert!(height <= width);
    }
}
