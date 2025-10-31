//! ALPN protocol identifier for rust-p2p-usb
//!
//! Application-Layer Protocol Negotiation (ALPN) identifier used for Iroh QUIC connections.
//! This ensures that only compatible clients and servers can communicate.

/// ALPN protocol identifier for rust-p2p-usb
///
/// This identifies the application protocol used over QUIC connections.
/// Version 1 of the rust-p2p-usb protocol.
pub const ALPN_PROTOCOL: &[u8] = b"rust-p2p-usb/1";
