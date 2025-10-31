//! Protocol error types

use thiserror::Error;

/// Protocol-level errors
#[derive(Debug, Error)]
pub enum ProtocolError {
    /// Serialization error from postcard
    #[error("Serialization error: {0}")]
    Serialization(#[from] postcard::Error),

    /// Incompatible protocol version detected
    #[error(
        "Incompatible protocol version: {major}.{minor} (expected {expected_major}.{expected_minor})"
    )]
    IncompatibleVersion {
        major: u8,
        minor: u8,
        expected_major: u8,
        expected_minor: u8,
    },

    /// Invalid message type encountered
    #[error("Invalid message type")]
    InvalidMessageType,

    /// Buffer too small for operation
    #[error("Buffer too small: needed {needed}, got {available}")]
    BufferTooSmall { needed: usize, available: usize },

    /// Frame length exceeds maximum allowed size
    #[error("Frame too large: {size} bytes (max: {max})")]
    FrameTooLarge { size: usize, max: usize },

    /// Incomplete frame data
    #[error("Incomplete frame: expected {expected} bytes, got {actual}")]
    IncompleteFrame { expected: usize, actual: usize },

    /// I/O error during frame operations
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Type alias for protocol results
pub type Result<T> = std::result::Result<T, ProtocolError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = ProtocolError::IncompatibleVersion {
            major: 2,
            minor: 0,
            expected_major: 1,
            expected_minor: 0,
        };
        let msg = format!("{}", err);
        assert!(msg.contains("Incompatible protocol version"));
        assert!(msg.contains("2.0"));
        assert!(msg.contains("1.0"));
    }

    #[test]
    fn test_frame_too_large_error() {
        let err = ProtocolError::FrameTooLarge {
            size: 10_000_000,
            max: 1_000_000,
        };
        let msg = format!("{}", err);
        assert!(msg.contains("Frame too large"));
    }
}
