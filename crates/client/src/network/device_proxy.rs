//! Remote USB device abstraction
//!
//! Provides a transparent proxy for remote USB devices, handling
//! descriptor caching, operation queueing, and retry logic.

use anyhow::{Context, Result, anyhow};
use iroh::PublicKey as EndpointId;
use protocol::{
    DeviceHandle, DeviceId, DeviceInfo, RequestId, TransferResult, TransferType, UsbRequest,
    UsbResponse, integrity::{compute_checksum, verify_checksum}, UsbError,
};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use super::client::IrohClient;

/// Remote USB device proxy
///
/// Represents a USB device connected to a remote server.
/// Provides methods for performing USB operations transparently
/// as if the device were local.
pub struct DeviceProxy {
    /// Client connection pool
    client: Arc<IrohClient>,
    /// Server hosting this device
    server_id: EndpointId,
    /// Device information (cached)
    info: DeviceInfo,
    /// Device handle (if attached)
    handle: Arc<RwLock<Option<DeviceHandle>>>,
}

impl DeviceProxy {
    /// Create a new device proxy
    ///
    /// # Arguments
    /// * `client` - IrohClient for network communication
    /// * `server_id` - Server hosting the device
    /// * `info` - Device information
    pub fn new(client: Arc<IrohClient>, server_id: EndpointId, info: DeviceInfo) -> Self {
        Self {
            client,
            server_id,
            info,
            handle: Arc::new(RwLock::new(None)),
        }
    }

    /// Get device information
    pub fn device_info(&self) -> &DeviceInfo {
        &self.info
    }

    /// Get device ID
    #[allow(dead_code)]
    pub fn device_id(&self) -> DeviceId {
        self.info.id
    }

    /// Get server NodeId
    #[allow(dead_code)]
    pub fn server_id(&self) -> EndpointId {
        self.server_id
    }

    /// Check if device is attached
    pub async fn is_attached(&self) -> bool {
        self.handle.read().await.is_some()
    }

    /// Attach to the device
    ///
    /// Must be called before performing any USB operations.
    pub async fn attach(&self) -> Result<()> {
        // Check if already attached
        if self.is_attached().await {
            return Ok(());
        }

        debug!(
            "Attaching to device {} on server {}",
            self.info.id.0, self.server_id
        );

        let handle = self
            .client
            .attach_device(self.server_id, self.info.id)
            .await
            .context("Failed to attach to device")?;

        *self.handle.write().await = Some(handle);

        debug!("Successfully attached to device {}", self.info.id.0);
        Ok(())
    }

    /// Detach from the device
    ///
    /// Releases the device handle. Any subsequent USB operations will fail
    /// until attach() is called again.
    pub async fn detach(&self) -> Result<()> {
        let mut handle_guard = self.handle.write().await;

        if let Some(handle) = *handle_guard {
            debug!("Detaching from device {}", self.info.id.0);

            self.client
                .detach_device(self.server_id, handle)
                .await
                .context("Failed to detach from device")?;

            *handle_guard = None;

            debug!("Successfully detached from device {}", self.info.id.0);
            Ok(())
        } else {
            Err(anyhow!("Device not attached"))
        }
    }

    /// Get device handle (returns error if not attached)
    async fn get_handle(&self) -> Result<DeviceHandle> {
        self.handle
            .read()
            .await
            .ok_or_else(|| anyhow!("Device not attached. Call attach() first."))
    }

    /// Get device handle (public version for USB/IP bridge)
    ///
    /// Returns the device handle if attached, or an error if not.
    /// Used by the USB/IP socket bridge to construct requests.
    pub async fn handle(&self) -> Result<DeviceHandle> {
        self.get_handle().await
    }

    /// Perform a control transfer
    ///
    /// # Arguments
    /// * `request_id` - Unique request ID
    /// * `request_type` - bmRequestType byte
    /// * `request` - bRequest byte
    /// * `value` - wValue parameter
    /// * `index` - wIndex parameter
    /// * `data` - Data to send (OUT) or empty for IN transfers
    ///
    /// # Returns
    /// Transfer result with data (for IN transfers)
    #[allow(dead_code)]
    pub async fn control_transfer(
        &self,
        request_id: RequestId,
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        data: Vec<u8>,
    ) -> Result<UsbResponse> {
        let handle = self.get_handle().await?;

        let usb_request = UsbRequest {
            id: request_id,
            handle,
            transfer: TransferType::Control {
                request_type,
                request,
                value,
                index,
                data,
            },
        };

        self.submit_transfer(usb_request).await
    }

    /// Perform an interrupt transfer
    ///
    /// # Arguments
    /// * `request_id` - Unique request ID
    /// * `endpoint` - Endpoint address (includes direction bit)
    /// * `data` - Data to send (OUT) or buffer size for IN transfers
    /// * `timeout_ms` - Transfer timeout in milliseconds
    ///
    /// # Returns
    /// Transfer result with data (for IN transfers)
    #[allow(dead_code)]
    pub async fn interrupt_transfer(
        &self,
        request_id: RequestId,
        endpoint: u8,
        data: Vec<u8>,
        timeout_ms: u32,
    ) -> Result<UsbResponse> {
        let handle = self.get_handle().await?;
        let is_in = (endpoint & 0x80) != 0;

        // Calculate checksum for OUT transfers
        let checksum = if !is_in {
            Some(compute_checksum(&data))
        } else {
            None
        };

        let usb_request = UsbRequest {
            id: request_id,
            handle,
            transfer: TransferType::Interrupt {
                endpoint,
                data,
                timeout_ms,
                checksum,
            },
        };

        let response = self.submit_transfer(usb_request).await?;

        // Verify checksum for IN transfers
        if is_in {
            if let TransferResult::Success { data, checksum: Some(expected) } = &response.result {
                if !verify_checksum(data, *expected) {
                    warn!("Bulk IN checksum mismatch: expected {:#x}", expected);
                    return Ok(UsbResponse {
                        id: request_id,
                        result: TransferResult::Error {
                            error: UsbError::Other {
                                message: "Checksum mismatch".to_string(),
                            },
                        },
                    });
                }
            }
        }

        Ok(response)
    }

    /// Perform a bulk transfer
    ///
    /// # Arguments
    /// * `request_id` - Unique request ID
    /// * `endpoint` - Endpoint address (includes direction bit)
    /// * `data` - Data to send (OUT) or buffer size for IN transfers
    /// * `timeout_ms` - Transfer timeout in milliseconds
    ///
    /// # Returns
    /// Transfer result with data (for IN transfers)
    #[allow(dead_code)]
    pub async fn bulk_transfer(
        &self,
        request_id: RequestId,
        endpoint: u8,
        data: Vec<u8>,
        timeout_ms: u32,
    ) -> Result<UsbResponse> {
        let handle = self.get_handle().await?;

        let usb_request = UsbRequest {
            id: request_id,
            handle,
            transfer: TransferType::Bulk {
                endpoint,
                data,
                timeout_ms,
            },
        };

        self.submit_transfer(usb_request).await
    }

    /// Submit transfer with automatic retry on transient errors
    pub async fn submit_transfer(&self, request: UsbRequest) -> Result<UsbResponse> {
        const MAX_RETRIES: u32 = 3;
        let mut attempts = 0;

        loop {
            attempts += 1;

            match self
                .client
                .submit_transfer(self.server_id, request.clone())
                .await
            {
                Ok(response) => {
                    // Check if response indicates a retryable error
                    if let TransferResult::Error { ref error } = response.result
                        && Self::is_retryable_error(error)
                        && attempts < MAX_RETRIES
                    {
                        warn!(
                            "Retryable error on attempt {}/{}: {:?}",
                            attempts, MAX_RETRIES, error
                        );
                        tokio::time::sleep(tokio::time::Duration::from_millis(
                            100 * attempts as u64,
                        ))
                        .await;
                        continue;
                    }

                    return Ok(response);
                }
                Err(e) => {
                    if attempts < MAX_RETRIES {
                        warn!(
                            "Transfer failed on attempt {}/{}: {}",
                            attempts, MAX_RETRIES, e
                        );
                        tokio::time::sleep(tokio::time::Duration::from_millis(
                            100 * attempts as u64,
                        ))
                        .await;
                        continue;
                    } else {
                        return Err(e).context("Transfer failed after retries");
                    }
                }
            }
        }
    }

    /// Check if a USB error is retryable
    fn is_retryable_error(error: &protocol::UsbError) -> bool {
        matches!(
            error,
            protocol::UsbError::Timeout | protocol::UsbError::Busy | protocol::UsbError::Io
        )
    }

    /// Get device descriptor string
    ///
    /// Returns a human-readable description of the device.
    pub fn description(&self) -> String {
        let manufacturer = self.info.manufacturer.as_deref().unwrap_or("Unknown");
        let product = self.info.product.as_deref().unwrap_or("Unknown");
        let vendor_id = self.info.vendor_id;
        let product_id = self.info.product_id;

        format!(
            "{} {} (VID: {:04x}, PID: {:04x})",
            manufacturer, product, vendor_id, product_id
        )
    }
}

impl Drop for DeviceProxy {
    fn drop(&mut self) {
        // Note: We can't do async operations in Drop, so we just log
        // The client should call detach() explicitly before dropping
        if let Ok(guard) = self.handle.try_write()
            && let Some(handle) = *guard
        {
            warn!(
                "DeviceProxy dropped without detaching (handle: {}). \
                     Call detach() explicitly before dropping.",
                handle.0
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::DeviceSpeed;

    fn create_test_device_info() -> DeviceInfo {
        DeviceInfo {
            id: DeviceId(1),
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
    fn test_device_info() {
        let info = create_test_device_info();
        assert_eq!(info.vendor_id, 0x1234);
        assert_eq!(info.product_id, 0x5678);
    }

    #[test]
    fn test_description() {
        let info = create_test_device_info();
        let desc = format!(
            "{} {} (VID: {:04x}, PID: {:04x})",
            info.manufacturer.as_ref().unwrap(),
            info.product.as_ref().unwrap(),
            info.vendor_id,
            info.product_id
        );

        assert!(desc.contains("Test Manufacturer"));
        assert!(desc.contains("Test Device"));
        assert!(desc.contains("1234"));
        assert!(desc.contains("5678"));
    }

    #[test]
    fn test_retryable_errors() {
        use protocol::UsbError;

        assert!(DeviceProxy::is_retryable_error(&UsbError::Timeout));
        assert!(DeviceProxy::is_retryable_error(&UsbError::Busy));
        assert!(DeviceProxy::is_retryable_error(&UsbError::Io));

        assert!(!DeviceProxy::is_retryable_error(&UsbError::NoDevice));
        assert!(!DeviceProxy::is_retryable_error(&UsbError::Pipe));
        assert!(!DeviceProxy::is_retryable_error(&UsbError::NotFound));
    }
}
