//! QUIC Stream Management
//!
//! Manages QUIC stream multiplexing for different USB transfer types.
//! In the current implementation (Phase 3), we use a simple one-stream-per-request
//! model. Future phases may implement dedicated streams per endpoint type for
//! optimal performance (control, interrupt, bulk).

use anyhow::{Context, Result};
use iroh::endpoint::{Connection, RecvStream, SendStream};
use protocol::{Message, decode_framed, encode_framed};
use tracing::trace;

/// Stream helper for sending messages
///
/// Wraps the low-level QUIC send stream with protocol-aware operations
pub struct MessageSender {
    send: SendStream,
}

impl MessageSender {
    /// Create a new message sender
    pub fn new(send: SendStream) -> Self {
        Self { send }
    }

    /// Send a protocol message
    pub async fn send_message(&mut self, message: &Message) -> Result<()> {
        let bytes = encode_framed(message).context("Failed to encode message")?;

        protocol::write_framed_async(&mut self.send, &bytes)
            .await
            .context("Failed to write framed message")?;

        trace!("Sent message: {:?}", message.payload);
        Ok(())
    }

    /// Finish the stream (no more writes)
    pub async fn finish(mut self) -> Result<()> {
        self.send.finish().context("Failed to finish stream")?;
        Ok(())
    }
}

/// Stream helper for receiving messages
///
/// Wraps the low-level QUIC receive stream with protocol-aware operations
pub struct MessageReceiver {
    recv: RecvStream,
}

impl MessageReceiver {
    /// Create a new message receiver
    pub fn new(recv: RecvStream) -> Self {
        Self { recv }
    }

    /// Receive a protocol message
    pub async fn recv_message(&mut self) -> Result<Message> {
        let bytes = protocol::read_framed_async(&mut self.recv)
            .await
            .context("Failed to read framed message")?;

        let message = decode_framed(&bytes).context("Failed to decode message")?;

        trace!("Received message: {:?}", message.payload);
        Ok(message)
    }
}

/// Stream multiplexing strategy
///
/// For Phase 3, we use a simple approach: one QUIC bi-directional stream per request.
/// This is sufficient for the MVP and avoids complexity.
///
/// Future optimization (Phase 9):
/// - Dedicated control stream (always open, bi-directional)
/// - Per-endpoint streams for bulk/interrupt transfers
/// - Stream pooling to reduce overhead
pub struct StreamMultiplexer {
    // Future: maintain stream pools per device
    // control_streams: HashMap<DeviceHandle, (MessageSender, MessageReceiver)>,
    // bulk_streams: HashMap<DeviceHandle, StreamPool>,
}

impl StreamMultiplexer {
    /// Create a new stream multiplexer
    pub fn new() -> Self {
        Self {
            // Placeholder for future implementation
        }
    }

    /// Get or create a control stream for a device
    ///
    /// Currently: creates new stream per request (simple approach)
    /// Future: maintains persistent control stream per device
    pub async fn get_control_stream(
        &mut self,
        _connection: &Connection,
    ) -> Result<(MessageSender, MessageReceiver)> {
        // Phase 3: Not implemented, using per-request streams in connection.rs
        // Phase 9: Implement persistent stream management
        Err(anyhow::anyhow!("Not implemented in Phase 3"))
    }
}

impl Default for StreamMultiplexer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_multiplexer_creation() {
        let multiplexer = StreamMultiplexer::new();
        // Basic sanity check
        assert!(true);
    }
}
