//! Test utilities for rust-p2p-usb
//!
//! Provides mock implementations and helper functions for testing across crates.
//!
//! # Example
//!
//! ```
//! use common::test_utils::{create_mock_device_info, TestTimeout};
//!
//! # fn main() {
//! let device = create_mock_device_info(1, 0x1234, 0x5678);
//! assert_eq!(device.vendor_id, 0x1234);
//! # }
//! ```

use protocol::{DeviceHandle, DeviceId, DeviceInfo, DeviceSpeed, RequestId};
use std::future::Future;
use std::time::Duration;

/// Default test timeout (5 seconds)
pub const DEFAULT_TEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Create a mock DeviceInfo for testing
///
/// # Arguments
/// * `id` - Device ID number
/// * `vendor_id` - USB Vendor ID
/// * `product_id` - USB Product ID
///
/// # Example
/// ```
/// use common::test_utils::create_mock_device_info;
///
/// let device = create_mock_device_info(1, 0x1234, 0x5678);
/// assert_eq!(device.id.0, 1);
/// assert_eq!(device.vendor_id, 0x1234);
/// ```
pub fn create_mock_device_info(id: u32, vendor_id: u16, product_id: u16) -> DeviceInfo {
    DeviceInfo {
        id: DeviceId(id),
        vendor_id,
        product_id,
        bus_number: 1,
        device_address: (id % 128) as u8,
        manufacturer: Some(format!("Test Manufacturer {}", id)),
        product: Some(format!("Test Product {}", id)),
        serial_number: Some(format!("SN{:06}", id)),
        class: 0x00,
        subclass: 0x00,
        protocol: 0x00,
        speed: DeviceSpeed::High,
        num_configurations: 1,
    }
}

/// Create a mock DeviceInfo with specific USB class
///
/// # Arguments
/// * `id` - Device ID number
/// * `vendor_id` - USB Vendor ID
/// * `product_id` - USB Product ID
/// * `class` - USB Device Class
/// * `subclass` - USB Device Subclass
/// * `protocol` - USB Device Protocol
pub fn create_mock_device_info_with_class(
    id: u32,
    vendor_id: u16,
    product_id: u16,
    class: u8,
    subclass: u8,
    protocol: u8,
) -> DeviceInfo {
    DeviceInfo {
        id: DeviceId(id),
        vendor_id,
        product_id,
        bus_number: 1,
        device_address: (id % 128) as u8,
        manufacturer: Some(format!("Test Manufacturer {}", id)),
        product: Some(format!("Test Product {}", id)),
        serial_number: Some(format!("SN{:06}", id)),
        class,
        subclass,
        protocol,
        speed: DeviceSpeed::High,
        num_configurations: 1,
    }
}

/// Create a mock mass storage device info
pub fn create_mock_mass_storage_device(id: u32) -> DeviceInfo {
    create_mock_device_info_with_class(id, 0x0781, 0x5581, 0x08, 0x06, 0x50)
}

/// Create a mock HID device info (keyboard/mouse)
pub fn create_mock_hid_device(id: u32) -> DeviceInfo {
    create_mock_device_info_with_class(id, 0x046d, 0xc52b, 0x03, 0x00, 0x00)
}

/// Create a mock hub device info
pub fn create_mock_hub_device(id: u32) -> DeviceInfo {
    create_mock_device_info_with_class(id, 0x05e3, 0x0608, 0x09, 0x00, 0x00)
}

/// Create a mock DeviceInfo with specific speed
pub fn create_mock_device_info_with_speed(
    id: u32,
    vendor_id: u16,
    product_id: u16,
    speed: DeviceSpeed,
) -> DeviceInfo {
    DeviceInfo {
        id: DeviceId(id),
        vendor_id,
        product_id,
        bus_number: 1,
        device_address: (id % 128) as u8,
        manufacturer: Some(format!("Test Manufacturer {}", id)),
        product: Some(format!("Test Product {}", id)),
        serial_number: Some(format!("SN{:06}", id)),
        class: 0x00,
        subclass: 0x00,
        protocol: 0x00,
        speed,
        num_configurations: 1,
    }
}

/// Create a list of mock devices for testing
///
/// # Arguments
/// * `count` - Number of devices to create
///
/// # Example
/// ```
/// use common::test_utils::create_mock_device_list;
///
/// let devices = create_mock_device_list(5);
/// assert_eq!(devices.len(), 5);
/// ```
pub fn create_mock_device_list(count: u32) -> Vec<DeviceInfo> {
    (1..=count)
        .map(|i| create_mock_device_info(i, 0x1000 + (i as u16), 0x2000 + (i as u16)))
        .collect()
}

/// Create a mock DeviceHandle for testing
pub fn create_mock_handle(id: u32) -> DeviceHandle {
    DeviceHandle(id)
}

/// Create a mock RequestId for testing
pub fn create_mock_request_id(id: u64) -> RequestId {
    RequestId(id)
}

/// Timeout wrapper for async tests
///
/// Wraps an async operation with a timeout to prevent tests from hanging.
///
/// # Arguments
/// * `duration` - Maximum time to wait
/// * `future` - The async operation to run
///
/// # Returns
/// Result containing the operation result or a timeout error
///
/// # Example
/// ```ignore
/// use common::test_utils::{with_timeout, DEFAULT_TEST_TIMEOUT};
///
/// #[tokio::test]
/// async fn test_with_timeout() {
///     let result = with_timeout(DEFAULT_TEST_TIMEOUT, async {
///         // Your async test logic here
///         Ok(42)
///     }).await.unwrap();
///     assert_eq!(result, 42);
/// }
/// ```
pub async fn with_timeout<T, F>(duration: Duration, future: F) -> Result<T, TimeoutError>
where
    F: Future<Output = T>,
{
    tokio::time::timeout(duration, future)
        .await
        .map_err(|_| TimeoutError { duration })
}

/// Error returned when a test times out
#[derive(Debug)]
pub struct TimeoutError {
    /// The timeout duration that was exceeded
    pub duration: Duration,
}

impl std::fmt::Display for TimeoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Test timed out after {:?}", self.duration)
    }
}

impl std::error::Error for TimeoutError {}

/// Generate a random-ish test ID based on current time
/// Useful for creating unique test identifiers
pub fn generate_test_id() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// Create a mock USB descriptor response (GET_DESCRIPTOR Device)
///
/// Returns a standard 18-byte device descriptor
pub fn create_mock_device_descriptor() -> Vec<u8> {
    vec![
        0x12, // bLength
        0x01, // bDescriptorType (Device)
        0x00, 0x02, // bcdUSB (2.00)
        0x00, // bDeviceClass
        0x00, // bDeviceSubClass
        0x00, // bDeviceProtocol
        0x40, // bMaxPacketSize0 (64 bytes)
        0x34, 0x12, // idVendor (0x1234)
        0x78, 0x56, // idProduct (0x5678)
        0x00, 0x01, // bcdDevice (1.00)
        0x01, // iManufacturer
        0x02, // iProduct
        0x03, // iSerialNumber
        0x01, // bNumConfigurations
    ]
}

/// Create a mock USB configuration descriptor
///
/// Returns a minimal configuration descriptor with one interface
pub fn create_mock_config_descriptor() -> Vec<u8> {
    vec![
        // Configuration descriptor
        0x09, // bLength
        0x02, // bDescriptorType (Configuration)
        0x19, 0x00, // wTotalLength (25 bytes)
        0x01, // bNumInterfaces
        0x01, // bConfigurationValue
        0x00, // iConfiguration
        0x80, // bmAttributes (Bus-powered)
        0x32, // bMaxPower (100mA)
        // Interface descriptor
        0x09, // bLength
        0x04, // bDescriptorType (Interface)
        0x00, // bInterfaceNumber
        0x00, // bAlternateSetting
        0x01, // bNumEndpoints
        0xFF, // bInterfaceClass (Vendor-specific)
        0x00, // bInterfaceSubClass
        0x00, // bInterfaceProtocol
        0x00, // iInterface
        // Endpoint descriptor
        0x07, // bLength
        0x05, // bDescriptorType (Endpoint)
        0x81, // bEndpointAddress (EP1 IN)
        0x02, // bmAttributes (Bulk)
        0x00, 0x02, // wMaxPacketSize (512 bytes)
        0x00, // bInterval
    ]
}

/// Create mock bulk transfer data of specified size
pub fn create_mock_bulk_data(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i & 0xFF) as u8).collect()
}

/// Create a mock control transfer setup packet
///
/// # Arguments
/// * `request_type` - bmRequestType
/// * `request` - bRequest
/// * `value` - wValue
/// * `index` - wIndex
/// * `length` - wLength
pub fn create_mock_setup_packet(
    request_type: u8,
    request: u8,
    value: u16,
    index: u16,
    length: u16,
) -> [u8; 8] {
    [
        request_type,
        request,
        (value & 0xFF) as u8,
        ((value >> 8) & 0xFF) as u8,
        (index & 0xFF) as u8,
        ((index >> 8) & 0xFF) as u8,
        (length & 0xFF) as u8,
        ((length >> 8) & 0xFF) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_mock_device_info() {
        let device = create_mock_device_info(42, 0x1234, 0x5678);

        assert_eq!(device.id.0, 42);
        assert_eq!(device.vendor_id, 0x1234);
        assert_eq!(device.product_id, 0x5678);
        assert!(device.manufacturer.is_some());
        assert!(device.product.is_some());
        assert!(device.serial_number.is_some());
    }

    #[test]
    fn test_create_mock_device_list() {
        let devices = create_mock_device_list(10);

        assert_eq!(devices.len(), 10);

        // Verify all IDs are unique
        let ids: Vec<u32> = devices.iter().map(|d| d.id.0).collect();
        let unique_ids: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(ids.len(), unique_ids.len());
    }

    #[test]
    fn test_create_mock_mass_storage_device() {
        let device = create_mock_mass_storage_device(1);

        assert_eq!(device.class, 0x08); // Mass Storage
        assert_eq!(device.subclass, 0x06); // SCSI
        assert_eq!(device.protocol, 0x50); // Bulk-Only
    }

    #[test]
    fn test_create_mock_hid_device() {
        let device = create_mock_hid_device(1);

        assert_eq!(device.class, 0x03); // HID
    }

    #[test]
    fn test_create_mock_device_descriptor() {
        let desc = create_mock_device_descriptor();

        assert_eq!(desc.len(), 18);
        assert_eq!(desc[0], 0x12); // bLength
        assert_eq!(desc[1], 0x01); // bDescriptorType
    }

    #[test]
    fn test_create_mock_config_descriptor() {
        let desc = create_mock_config_descriptor();

        assert_eq!(desc.len(), 25);
        assert_eq!(desc[0], 0x09); // Configuration descriptor length
        assert_eq!(desc[1], 0x02); // bDescriptorType (Configuration)
    }

    #[test]
    fn test_create_mock_bulk_data() {
        let data = create_mock_bulk_data(1024);

        assert_eq!(data.len(), 1024);
        assert_eq!(data[0], 0);
        assert_eq!(data[255], 255);
        assert_eq!(data[256], 0); // Wraps around
    }

    #[test]
    fn test_create_mock_setup_packet() {
        let setup = create_mock_setup_packet(0x80, 0x06, 0x0100, 0x0000, 0x0012);

        assert_eq!(setup.len(), 8);
        assert_eq!(setup[0], 0x80); // bmRequestType (Device-to-host, Standard, Device)
        assert_eq!(setup[1], 0x06); // bRequest (GET_DESCRIPTOR)
        assert_eq!(setup[2], 0x00); // wValue low (Descriptor index)
        assert_eq!(setup[3], 0x01); // wValue high (Descriptor type: Device)
        assert_eq!(setup[6], 0x12); // wLength low (18 bytes)
        assert_eq!(setup[7], 0x00); // wLength high
    }

    #[tokio::test]
    async fn test_with_timeout_success() {
        let result = with_timeout(DEFAULT_TEST_TIMEOUT, async { 42 }).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_with_timeout_failure() {
        let result = with_timeout(Duration::from_millis(10), async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            42
        })
        .await;

        assert!(result.is_err());
    }

    #[test]
    fn test_generate_test_id() {
        let id1 = generate_test_id();
        let id2 = generate_test_id();

        // IDs should be non-zero and likely different
        assert!(id1 > 0);
        assert!(id2 > 0);
    }
}
