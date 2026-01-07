//! Iroh networking extensions and utilities
//!
//! This module contains utilities for working with Iroh:
//! - Stream helpers
//! - Connection management utilities
//! - Node ID handling
//! - ALPN protocol constants

/// ALPN protocol identifier for rust-p2p-usb
pub const ALPN_PROTOCOL: &[u8] = b"rust-p2p-usb/1";

/// Generate a test endpoint ID for testing purposes
///
/// Creates a random PublicKey that can be used as an endpoint ID in tests.
#[cfg(test)]
pub fn generate_test_endpoint_id() -> iroh::PublicKey {
    iroh::SecretKey::generate(&mut rand::rng()).public()
}
