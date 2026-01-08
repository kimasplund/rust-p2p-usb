//! Protocol library for rust-p2p-usb
//!
//! This crate defines the message protocol for communication between the USB server and client.
//! It provides type-safe message definitions, serialization/deserialization using postcard,
//! and protocol versioning.
//!
//! # Example
//!
//! ```
//! use protocol::{Message, MessagePayload, CURRENT_VERSION};
//! use protocol::{encode_message, decode_message};
//!
//! // Create a message
//! let msg = Message {
//!     version: CURRENT_VERSION,
//!     payload: MessagePayload::Ping,
//! };
//!
//! // Serialize
//! let bytes = encode_message(&msg).unwrap();
//!
//! // Deserialize
//! let decoded = decode_message(&bytes).unwrap();
//! assert_eq!(decoded.version, CURRENT_VERSION);
//! ```
//!
//! # Framed Messages
//!
//! For QUIC stream communication, use length-prefixed framing:
//!
//! ```
//! use protocol::{Message, MessagePayload, CURRENT_VERSION};
//! use protocol::{encode_framed, decode_framed};
//!
//! let msg = Message {
//!     version: CURRENT_VERSION,
//!     payload: MessagePayload::Pong,
//! };
//!
//! // Encode with length prefix
//! let framed = encode_framed(&msg).unwrap();
//!
//! // Decode framed message
//! let decoded = decode_framed(&framed).unwrap();
//! ```

pub mod codec;
pub mod error;
pub mod integrity;
pub mod messages;
pub mod types;
pub mod version;

pub use codec::{
    MAX_FRAME_SIZE, decode_framed, decode_message, encode_framed, encode_message, read_framed,
    validate_version, write_framed,
};

#[cfg(feature = "async")]
pub use codec::{read_framed_async, write_framed_async};
pub use error::{ProtocolError, Result};
pub use messages::{Message, MessagePayload};
pub use types::{
    AggregatedNotification, AttachError, ClientMetrics, DetachError, DeviceHandle, DeviceId,
    DeviceInfo, DeviceMetrics, DeviceRemovalReason, DeviceSharingStatus, DeviceSpeed,
    DeviceStatusChangeReason, ForceDetachReason, InterruptStreamInfo, InterruptStreamStats,
    IsoPacketDescriptor, IsoPacketResult, LockResult, ProtocolLatencyStats, ProtocolMetrics,
    QueuePositionUpdate, RequestId, ServerMetricsSummary, SharingMode, SuperSpeedConfig,
    TransferResult, TransferType, UnlockResult, UsbError, UsbRequest, UsbResponse,
};
pub use version::{CURRENT_VERSION, ProtocolVersion};
