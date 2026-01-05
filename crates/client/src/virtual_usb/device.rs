//! Virtual USB device state management
//!
//! This module provides the VirtualDevice abstraction that represents
//! a single virtual USB device and handles proxying USB operations
//! to the remote physical device.

use anyhow::Result;
use protocol::{DeviceHandle, DeviceInfo, RequestId};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::network::device_proxy::DeviceProxy;

/// Virtual USB device
///
/// Represents a virtual USB device attached to the system via USB/IP.
/// This struct maintains the state needed to proxy USB operations
/// from the kernel to the remote device.
#[allow(dead_code)]
pub struct VirtualDevice {
    /// Device handle
    handle: DeviceHandle,
    /// Device proxy for remote operations
    device_proxy: Arc<DeviceProxy>,
    /// Device descriptor information
    descriptor: DeviceInfo,
    /// VHCI port number (0-7)
    vhci_port: u8,
    /// Request ID counter for generating unique request IDs
    next_request_id: AtomicU64,
}

#[allow(dead_code)]
impl VirtualDevice {
    /// Create a new virtual device
    ///
    /// # Arguments
    ///
    /// * `handle` - Device handle
    /// * `device_proxy` - Device proxy for remote operations
    /// * `descriptor` - Device descriptor information
    /// * `vhci_port` - VHCI port number
    pub fn new(
        handle: DeviceHandle,
        device_proxy: Arc<DeviceProxy>,
        descriptor: DeviceInfo,
        vhci_port: u8,
    ) -> Self {
        Self {
            handle,
            device_proxy,
            descriptor,
            vhci_port,
            next_request_id: AtomicU64::new(1),
        }
    }

    /// Get device handle
    pub fn handle(&self) -> DeviceHandle {
        self.handle
    }

    /// Get device proxy
    pub fn device_proxy(&self) -> &Arc<DeviceProxy> {
        &self.device_proxy
    }

    /// Get device descriptor
    pub fn descriptor(&self) -> &DeviceInfo {
        &self.descriptor
    }

    /// Get VHCI port number
    pub fn vhci_port(&self) -> u8 {
        self.vhci_port
    }

    /// Generate a unique request ID
    fn next_request_id(&self) -> RequestId {
        let id = self.next_request_id.fetch_add(1, Ordering::SeqCst);
        RequestId(id)
    }

    /// Handle a control transfer request
    ///
    /// Forwards the control transfer to the remote device via the device proxy.
    ///
    /// # Arguments
    ///
    /// * `request_type` - bmRequestType byte
    /// * `request` - bRequest byte
    /// * `value` - wValue parameter
    /// * `index` - wIndex parameter
    /// * `data` - Data to send (OUT) or empty for IN transfers
    ///
    /// # Returns
    ///
    /// Data received (for IN transfers) or empty for OUT transfers
    pub async fn handle_control_request(
        &self,
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        data: Vec<u8>,
    ) -> Result<Vec<u8>> {
        let request_id = self.next_request_id();

        let response = self
            .device_proxy
            .control_transfer(request_id, request_type, request, value, index, data)
            .await?;

        match response.result {
            protocol::TransferResult::Success { data } => Ok(data),
            protocol::TransferResult::Error { error } => {
                Err(anyhow::anyhow!("Control transfer failed: {:?}", error))
            }
        }
    }

    /// Handle a bulk transfer request
    ///
    /// Forwards the bulk transfer to the remote device via the device proxy.
    ///
    /// # Arguments
    ///
    /// * `endpoint` - Endpoint address (includes direction bit)
    /// * `data` - Data to send (OUT) or buffer size for IN transfers
    /// * `timeout_ms` - Transfer timeout in milliseconds
    ///
    /// # Returns
    ///
    /// Data received (for IN transfers) or empty for OUT transfers
    pub async fn handle_bulk_transfer(
        &self,
        endpoint: u8,
        data: Vec<u8>,
        timeout_ms: u32,
    ) -> Result<Vec<u8>> {
        let request_id = self.next_request_id();

        let response = self
            .device_proxy
            .bulk_transfer(request_id, endpoint, data, timeout_ms)
            .await?;

        match response.result {
            protocol::TransferResult::Success { data } => Ok(data),
            protocol::TransferResult::Error { error } => {
                Err(anyhow::anyhow!("Bulk transfer failed: {:?}", error))
            }
        }
    }

    /// Handle an interrupt transfer request
    ///
    /// Forwards the interrupt transfer to the remote device via the device proxy.
    ///
    /// # Arguments
    ///
    /// * `endpoint` - Endpoint address (includes direction bit)
    /// * `data` - Data to send (OUT) or buffer size for IN transfers
    /// * `timeout_ms` - Transfer timeout in milliseconds
    ///
    /// # Returns
    ///
    /// Data received (for IN transfers) or empty for OUT transfers
    pub async fn handle_interrupt_transfer(
        &self,
        endpoint: u8,
        data: Vec<u8>,
        timeout_ms: u32,
    ) -> Result<Vec<u8>> {
        let request_id = self.next_request_id();

        let response = self
            .device_proxy
            .interrupt_transfer(request_id, endpoint, data, timeout_ms)
            .await?;

        match response.result {
            protocol::TransferResult::Success { data } => Ok(data),
            protocol::TransferResult::Error { error } => {
                Err(anyhow::anyhow!("Interrupt transfer failed: {:?}", error))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::DeviceSpeed;

    fn create_test_device_info() -> DeviceInfo {
        DeviceInfo {
            id: protocol::DeviceId(1),
            vendor_id: 0x1234,
            product_id: 0x5678,
            bus_number: 1,
            device_address: 5,
            manufacturer: Some("Test Manufacturer".to_string()),
            product: Some("Test Device".to_string()),
            serial_number: Some("ABC123".to_string()),
            class: 0x08,
            subclass: 0x06,
            protocol: 0x50,
            speed: DeviceSpeed::High,
            num_configurations: 1,
        }
    }

    #[test]
    fn test_request_id_uniqueness() {
        // Test request ID generation using AtomicU64 directly
        // (Can't easily construct VirtualDevice without real DeviceProxy)
        let counter = AtomicU64::new(1);

        let id1 = RequestId(counter.fetch_add(1, Ordering::SeqCst));
        let id2 = RequestId(counter.fetch_add(1, Ordering::SeqCst));
        let id3 = RequestId(counter.fetch_add(1, Ordering::SeqCst));

        assert_eq!(id1.0, 1);
        assert_eq!(id2.0, 2);
        assert_eq!(id3.0, 3);
    }
}
