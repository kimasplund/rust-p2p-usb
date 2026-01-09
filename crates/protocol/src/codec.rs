//! Message serialization and deserialization using postcard
//!
//! This module provides encoding/decoding functions for protocol messages.
//! Messages are serialized using postcard (compact binary format) and can be
//! framed with a length prefix for use over QUIC streams.
//!
//! # Frame Format
//!
//! For QUIC streams, messages are length-prefixed:
//! ```text
//! [Length: u32 (big-endian)][Message bytes (postcard serialized)]
//! ```
//!
//! Maximum frame size is 16 MiB (16,777,216 bytes) to prevent memory exhaustion.

use crate::{CURRENT_VERSION, Message, ProtocolVersion, error::ProtocolError, error::Result};
use std::io::{Read, Write};

#[cfg(feature = "async")]
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Maximum allowed frame size (32 MiB) - increased for device lists
pub const MAX_FRAME_SIZE: usize = 32 * 1024 * 1024;

/// Encode a message to bytes using postcard
///
/// # Example
/// ```
/// use protocol::{Message, MessagePayload, CURRENT_VERSION, encode_message};
///
/// let msg = Message {
///     version: CURRENT_VERSION,
///     payload: MessagePayload::Ping,
/// };
/// let bytes = encode_message(&msg).unwrap();
/// assert!(!bytes.is_empty());
/// ```
pub fn encode_message(message: &Message) -> Result<Vec<u8>> {
    postcard::to_allocvec(message).map_err(ProtocolError::from)
}

/// Decode a message from bytes using postcard
///
/// # Example
/// ```
/// use protocol::{Message, MessagePayload, CURRENT_VERSION, encode_message, decode_message};
///
/// let msg = Message {
///     version: CURRENT_VERSION,
///     payload: MessagePayload::Ping,
/// };
/// let bytes = encode_message(&msg).unwrap();
/// let decoded = decode_message(&bytes).unwrap();
/// assert_eq!(decoded.version, CURRENT_VERSION);
/// ```
pub fn decode_message(bytes: &[u8]) -> Result<Message> {
    postcard::from_bytes(bytes).map_err(ProtocolError::from)
}

/// Validate protocol version compatibility
///
/// Returns an error if the message version is incompatible with the current version.
/// Compatible if major versions match. Minor version differences are allowed (forward and backward compatible).
pub fn validate_version(message_version: &ProtocolVersion) -> Result<()> {
    // Major versions must match
    if message_version.major != CURRENT_VERSION.major {
        return Err(ProtocolError::IncompatibleVersion {
            major: message_version.major,
            minor: message_version.minor,
            expected_major: CURRENT_VERSION.major,
            expected_minor: CURRENT_VERSION.minor,
        });
    }
    // Minor version differences are allowed (both forward and backward compatible)
    Ok(())
}

/// Encode a message with length prefix for framing
///
/// Frame format: [4-byte length (big-endian)][postcard message bytes]
///
/// # Example
/// ```
/// use protocol::{Message, MessagePayload, CURRENT_VERSION, encode_framed};
///
/// let msg = Message {
///     version: CURRENT_VERSION,
///     payload: MessagePayload::Ping,
/// };
/// let framed = encode_framed(&msg).unwrap();
/// assert!(framed.len() >= 4); // At least length prefix
/// ```
pub fn encode_framed(message: &Message) -> Result<Vec<u8>> {
    let message_bytes = encode_message(message)?;
    let message_len = message_bytes.len();

    // Check maximum frame size
    if message_len > MAX_FRAME_SIZE {
        return Err(ProtocolError::FrameTooLarge {
            size: message_len,
            max: MAX_FRAME_SIZE,
        });
    }

    // Build frame: [length: u32][message bytes]
    let mut frame = Vec::with_capacity(4 + message_len);
    frame.extend_from_slice(&(message_len as u32).to_be_bytes());
    frame.extend_from_slice(&message_bytes);

    Ok(frame)
}

/// Decode a framed message
///
/// Expects frame format: [4-byte length (big-endian)][postcard message bytes]
///
/// # Example
/// ```
/// use protocol::{Message, MessagePayload, CURRENT_VERSION, encode_framed, decode_framed};
///
/// let msg = Message {
///     version: CURRENT_VERSION,
///     payload: MessagePayload::Pong,
/// };
/// let framed = encode_framed(&msg).unwrap();
/// let decoded = decode_framed(&framed).unwrap();
/// assert_eq!(decoded.version, CURRENT_VERSION);
/// ```
pub fn decode_framed(frame: &[u8]) -> Result<Message> {
    // Need at least 4 bytes for length prefix
    if frame.len() < 4 {
        return Err(ProtocolError::IncompleteFrame {
            expected: 4,
            actual: frame.len(),
        });
    }

    // Read length prefix
    let length = u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]) as usize;

    // Check maximum frame size
    if length > MAX_FRAME_SIZE {
        return Err(ProtocolError::FrameTooLarge {
            size: length,
            max: MAX_FRAME_SIZE,
        });
    }

    // Check we have enough data
    if frame.len() < 4 + length {
        return Err(ProtocolError::IncompleteFrame {
            expected: 4 + length,
            actual: frame.len(),
        });
    }

    // Decode message
    let message_bytes = &frame[4..4 + length];
    decode_message(message_bytes)
}

/// Write a framed message to a writer (e.g., QUIC stream)
///
/// # Example
/// ```no_run
/// use protocol::{Message, MessagePayload, CURRENT_VERSION, write_framed};
/// use std::io::Cursor;
///
/// let msg = Message {
///     version: CURRENT_VERSION,
///     payload: MessagePayload::Ping,
/// };
/// let mut buffer = Vec::new();
/// write_framed(&mut buffer, &msg).unwrap();
/// ```
pub fn write_framed<W: Write>(writer: &mut W, message: &Message) -> Result<()> {
    let framed = encode_framed(message)?;
    writer.write_all(&framed)?;
    Ok(())
}

/// Read a framed message from a reader (e.g., QUIC stream)
///
/// # Example
/// ```no_run
/// use protocol::{Message, MessagePayload, CURRENT_VERSION, write_framed, read_framed};
/// use std::io::Cursor;
///
/// let msg = Message {
///     version: CURRENT_VERSION,
///     payload: MessagePayload::Ping,
/// };
/// let mut buffer = Vec::new();
/// write_framed(&mut buffer, &msg).unwrap();
///
/// let mut cursor = Cursor::new(buffer);
/// let decoded = read_framed(&mut cursor).unwrap();
/// assert_eq!(decoded.version, CURRENT_VERSION);
/// ```
pub fn read_framed<R: Read>(reader: &mut R) -> Result<Message> {
    // Read length prefix
    let mut len_bytes = [0u8; 4];
    reader.read_exact(&mut len_bytes)?;
    let length = u32::from_be_bytes(len_bytes) as usize;

    // Check maximum frame size
    if length > MAX_FRAME_SIZE {
        return Err(ProtocolError::FrameTooLarge {
            size: length,
            max: MAX_FRAME_SIZE,
        });
    }

    // Read message bytes
    let mut message_bytes = vec![0u8; length];
    reader.read_exact(&mut message_bytes)?;

    // Decode message
    decode_message(&message_bytes)
}

/// Async: Write a framed message to an async writer (e.g., QUIC stream)
#[cfg(feature = "async")]
pub async fn write_framed_async<W>(writer: &mut W, framed_bytes: &[u8]) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    writer.write_all(framed_bytes).await?;
    Ok(())
}

/// Async: Read a framed message from an async reader (e.g., QUIC stream)
///
/// Returns the complete framed message bytes (including length prefix)
#[cfg(feature = "async")]
pub async fn read_framed_async<R>(reader: &mut R) -> Result<Vec<u8>>
where
    R: AsyncReadExt + Unpin,
{
    // Read length prefix
    let mut len_bytes = [0u8; 4];
    reader.read_exact(&mut len_bytes).await?;
    let length = u32::from_be_bytes(len_bytes) as usize;

    // Check maximum frame size
    if length > MAX_FRAME_SIZE {
        return Err(ProtocolError::FrameTooLarge {
            size: length,
            max: MAX_FRAME_SIZE,
        });
    }

    // Read message bytes
    let mut message_bytes = vec![0u8; length];
    reader.read_exact(&mut message_bytes).await?;

    // Return complete frame (length prefix + message)
    let mut frame = Vec::with_capacity(4 + length);
    frame.extend_from_slice(&len_bytes);
    frame.extend_from_slice(&message_bytes);

    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CURRENT_VERSION, MessagePayload,
        types::{DeviceId, DeviceInfo, DeviceSpeed, RequestId, TransferType, UsbRequest},
    };
    use std::io::Cursor;

    #[test]
    fn test_message_roundtrip() {
        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::Ping,
        };

        let bytes = encode_message(&msg).unwrap();
        let decoded = decode_message(&bytes).unwrap();

        assert_eq!(msg.version, decoded.version);
    }

    #[test]
    fn test_list_devices_roundtrip() {
        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::ListDevicesRequest,
        };

        let bytes = encode_message(&msg).unwrap();
        let decoded = decode_message(&bytes).unwrap();

        assert_eq!(msg.version, decoded.version);
    }

    #[test]
    fn test_list_devices_response_roundtrip() {
        let devices = vec![
            DeviceInfo {
                id: DeviceId(1),
                vendor_id: 0x1234,
                product_id: 0x5678,
                bus_number: 1,
                device_address: 5,
                manufacturer: Some("Test Manufacturer".to_string()),
                product: Some("Test Product".to_string()),
                serial_number: Some("ABC123".to_string()),
                class: 0x08,
                subclass: 0x06,
                protocol: 0x50,
                speed: DeviceSpeed::High,
                num_configurations: 1,
            },
            DeviceInfo {
                id: DeviceId(2),
                vendor_id: 0xabcd,
                product_id: 0xef01,
                bus_number: 2,
                device_address: 3,
                manufacturer: None,
                product: None,
                serial_number: None,
                class: 0x03,
                subclass: 0x00,
                protocol: 0x00,
                speed: DeviceSpeed::Full,
                num_configurations: 1,
            },
        ];

        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::ListDevicesResponse { devices },
        };

        let bytes = encode_message(&msg).unwrap();
        let decoded = decode_message(&bytes).unwrap();

        assert_eq!(msg.version, decoded.version);
        let MessagePayload::ListDevicesResponse { devices } = decoded.payload else {
            panic!(
                "Expected ListDevicesResponse payload, got {:?}",
                std::mem::discriminant(&decoded.payload)
            );
        };
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].id, DeviceId(1));
        assert_eq!(devices[1].vendor_id, 0xabcd);
    }

    #[test]
    fn test_usb_transfer_roundtrip() {
        let request = UsbRequest {
            id: RequestId(42),
            handle: crate::types::DeviceHandle(1),
            transfer: TransferType::Control {
                request_type: 0x80,
                request: 0x06,
                value: 0x0100,
                index: 0,
                data: vec![0; 64],
            },
        };

        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::SubmitTransfer { request },
        };

        let bytes = encode_message(&msg).unwrap();
        let decoded = decode_message(&bytes).unwrap();

        assert_eq!(msg.version, decoded.version);
    }

    #[test]
    fn test_bulk_transfer_large_data() {
        let data = vec![0xAB; 4096]; // 4KB bulk transfer
        let request = UsbRequest {
            id: RequestId(100),
            handle: crate::types::DeviceHandle(5),
            transfer: TransferType::Bulk {
                endpoint: 0x81,
                data,
                timeout_ms: 5000,
                checksum: None,
            },
        };

        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::SubmitTransfer { request },
        };

        let bytes = encode_message(&msg).unwrap();
        let decoded = decode_message(&bytes).unwrap();

        let MessagePayload::SubmitTransfer { request } = decoded.payload else {
            panic!(
                "Expected SubmitTransfer payload, got {:?}",
                std::mem::discriminant(&decoded.payload)
            );
        };
        let TransferType::Bulk { data, .. } = request.transfer else {
            panic!(
                "Expected Bulk transfer type, got {:?}",
                std::mem::discriminant(&request.transfer)
            );
        };
        assert_eq!(data.len(), 4096);
        assert_eq!(data[0], 0xAB);
    }

    #[test]
    fn test_framed_encode_decode() {
        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::Pong,
        };

        let framed = encode_framed(&msg).unwrap();
        assert!(framed.len() >= 4); // At least length prefix

        let decoded = decode_framed(&framed).unwrap();
        assert_eq!(msg.version, decoded.version);
    }

    #[test]
    fn test_framed_large_message() {
        let devices = vec![
            DeviceInfo {
                id: DeviceId(1),
                vendor_id: 0x1234,
                product_id: 0x5678,
                bus_number: 1,
                device_address: 5,
                manufacturer: Some("A".repeat(100)),
                product: Some("B".repeat(100)),
                serial_number: Some("C".repeat(50)),
                class: 0x08,
                subclass: 0x06,
                protocol: 0x50,
                speed: DeviceSpeed::High,
                num_configurations: 1,
            };
            100 // 100 devices
        ];

        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::ListDevicesResponse { devices },
        };

        let framed = encode_framed(&msg).unwrap();
        let decoded = decode_framed(&framed).unwrap();

        let MessagePayload::ListDevicesResponse { devices } = decoded.payload else {
            panic!(
                "Expected ListDevicesResponse payload, got {:?}",
                std::mem::discriminant(&decoded.payload)
            );
        };
        assert_eq!(devices.len(), 100);
    }

    #[test]
    fn test_framed_incomplete_frame() {
        let incomplete = vec![0, 0, 0, 10]; // Says 10 bytes but provides none
        let result = decode_framed(&incomplete);
        assert!(result.is_err());
        let Err(ProtocolError::IncompleteFrame { expected, actual }) = result else {
            panic!("Expected IncompleteFrame error, got {:?}", result);
        };
        assert_eq!(expected, 14); // 4 + 10
        assert_eq!(actual, 4);
    }

    #[test]
    fn test_framed_too_large() {
        let too_large = vec![0xFF, 0xFF, 0xFF, 0xFF]; // 4GB frame
        let result = decode_framed(&too_large);
        assert!(result.is_err());
        assert!(matches!(result, Err(ProtocolError::FrameTooLarge { .. })));
    }

    #[test]
    fn test_write_read_framed() {
        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::Ping,
        };

        let mut buffer = Vec::new();
        write_framed(&mut buffer, &msg).unwrap();

        let mut cursor = Cursor::new(buffer);
        let decoded = read_framed(&mut cursor).unwrap();

        assert_eq!(msg.version, decoded.version);
    }

    #[test]
    fn test_validate_version_compatible() {
        let v1_0 = ProtocolVersion {
            major: 1,
            minor: 0,
            patch: 0,
        };
        assert!(validate_version(&v1_0).is_ok());
    }

    #[test]
    fn test_validate_version_incompatible_major() {
        let v2_0 = ProtocolVersion {
            major: 2,
            minor: 0,
            patch: 0,
        };
        let result = validate_version(&v2_0);
        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(ProtocolError::IncompatibleVersion { .. })
        ));
    }

    #[test]
    fn test_validate_version_newer_minor() {
        let v1_5 = ProtocolVersion {
            major: 1,
            minor: 5,
            patch: 0,
        };
        // Newer minor version should be compatible (forward compatible)
        assert!(validate_version(&v1_5).is_ok());
    }

    #[test]
    fn test_empty_frame() {
        let empty: &[u8] = &[];
        let result = decode_framed(empty);
        assert!(result.is_err());
        assert!(matches!(result, Err(ProtocolError::IncompleteFrame { .. })));
    }

    #[test]
    fn test_partial_length_prefix() {
        let partial = vec![0, 0]; // Only 2 bytes of 4-byte length
        let result = decode_framed(&partial);
        assert!(result.is_err());
    }

    #[test]
    fn test_serialize_all_message_types() {
        let messages = vec![
            MessagePayload::Ping,
            MessagePayload::Pong,
            MessagePayload::ListDevicesRequest,
            MessagePayload::Error {
                message: "Test error".to_string(),
            },
        ];

        for payload in messages {
            let msg = Message {
                version: CURRENT_VERSION,
                payload,
            };
            let bytes = encode_message(&msg).unwrap();
            let decoded = decode_message(&bytes).unwrap();
            assert_eq!(msg.version, decoded.version);
        }
    }
}
