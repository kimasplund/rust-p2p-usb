//! Client Integration Tests
//!
//! Comprehensive tests for the client crate covering:
//! - Configuration loading and validation
//! - USB/IP message serialization (Linux only)
//! - Transfer type handling
//! - Protocol message construction
//!
//! Note: These tests replicate config structures for testing since
//! the client crate is a binary-only crate.
//!
//! Run with: `cargo test -p client --test integration_tests`

use common::test_utils::{
    DEFAULT_TEST_TIMEOUT, create_mock_device_descriptor, create_mock_device_info,
    create_mock_setup_packet, with_timeout,
};
use protocol::{DeviceHandle, DeviceId, DeviceSpeed, RequestId, TransferResult, TransferType};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tempfile::tempdir;

// ============================================================================
// Config Structures (duplicated for testing since client is binary crate)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClientConfig {
    client: ClientSettings,
    servers: ServersSettings,
    iroh: IrohSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClientSettings {
    auto_connect: bool,
    log_level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ServersSettings {
    approved_servers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IrohSettings {
    relay_servers: Option<Vec<String>>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            client: ClientSettings {
                auto_connect: true,
                log_level: "info".to_string(),
            },
            servers: ServersSettings {
                approved_servers: Vec::new(),
            },
            iroh: IrohSettings {
                relay_servers: None,
            },
        }
    }
}

impl ClientConfig {
    fn default_path() -> PathBuf {
        if let Some(config_dir) = dirs::config_dir() {
            config_dir.join("p2p-usb").join("client.toml")
        } else {
            PathBuf::from(".config/p2p-usb/client.toml")
        }
    }
}

// ============================================================================
// Client Configuration Tests
// ============================================================================

#[test]
fn test_client_config_default() {
    let config = ClientConfig::default();

    assert_eq!(config.client.log_level, "info");
    assert!(config.client.auto_connect);
    assert!(config.servers.approved_servers.is_empty());
    assert!(config.iroh.relay_servers.is_none());
}

#[test]
fn test_client_config_serialization_roundtrip() {
    let config = ClientConfig::default();
    let toml_str = toml::to_string(&config).expect("Failed to serialize");
    let parsed: ClientConfig = toml::from_str(&toml_str).expect("Failed to parse");

    assert_eq!(config.client.log_level, parsed.client.log_level);
    assert_eq!(config.client.auto_connect, parsed.client.auto_connect);
}

#[test]
fn test_client_config_with_custom_values() {
    let toml_content = r#"
[client]
auto_connect = false
log_level = "trace"

[servers]
approved_servers = ["server1", "server2", "server3"]

[iroh]
relay_servers = ["relay1.example.com", "relay2.example.com"]
"#;

    let config: ClientConfig = toml::from_str(toml_content).expect("Failed to parse");

    assert!(!config.client.auto_connect);
    assert_eq!(config.client.log_level, "trace");
    assert_eq!(config.servers.approved_servers.len(), 3);
    assert!(config.iroh.relay_servers.is_some());
    assert_eq!(config.iroh.relay_servers.as_ref().unwrap().len(), 2);
}

#[test]
fn test_client_config_log_levels() {
    let valid_levels = ["trace", "debug", "info", "warn", "error"];

    for level in valid_levels {
        let mut config = ClientConfig::default();
        config.client.log_level = level.to_string();
        assert_eq!(config.client.log_level, level);
    }
}

#[test]
fn test_client_config_save_and_load() {
    let dir = tempdir().expect("Failed to create temp dir");
    let config_path = dir.path().join("client.toml");

    let mut config = ClientConfig::default();
    config.client.log_level = "debug".to_string();
    config.client.auto_connect = false;
    config
        .servers
        .approved_servers
        .push("test-server".to_string());

    // Save config
    let toml_str = toml::to_string(&config).expect("Failed to serialize");
    std::fs::write(&config_path, toml_str).expect("Failed to write");

    // Verify file exists
    assert!(config_path.exists());

    // Load config
    let content = std::fs::read_to_string(&config_path).expect("Failed to read");
    let loaded: ClientConfig = toml::from_str(&content).expect("Failed to parse");

    assert_eq!(loaded.client.log_level, "debug");
    assert!(!loaded.client.auto_connect);
    assert_eq!(loaded.servers.approved_servers.len(), 1);
}

#[test]
fn test_client_config_default_path() {
    let path = ClientConfig::default_path();

    // Should contain "p2p-usb" and "client.toml"
    let path_str = path.to_string_lossy();
    assert!(path_str.contains("p2p-usb"));
    assert!(path_str.contains("client.toml"));
}

// ============================================================================
// USB/IP Protocol Tests (Linux only)
// ============================================================================

#[cfg(target_os = "linux")]
mod usbip_tests {
    use super::*;
    use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
    use std::io::{Cursor, Read, Write};

    // USB/IP protocol constants
    const USBIP_VERSION: u16 = 0x0111;
    const OP_REQ_IMPORT: u16 = 0x8003;
    const OP_REP_IMPORT: u16 = 0x0003;

    #[repr(u16)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum UsbIpCommand {
        CmdSubmit = 0x0001,
        RetSubmit = 0x0003,
        CmdUnlink = 0x0002,
        RetUnlink = 0x0004,
    }

    impl UsbIpCommand {
        fn from_u16(value: u16) -> Option<Self> {
            match value {
                0x0001 => Some(Self::CmdSubmit),
                0x0003 => Some(Self::RetSubmit),
                0x0002 => Some(Self::CmdUnlink),
                0x0004 => Some(Self::RetUnlink),
                _ => None,
            }
        }
    }

    // USB/IP Header (20 bytes)
    struct UsbIpHeader {
        command: u32,
        seqnum: u32,
        devid: u32,
        direction: u32,
        ep: u32,
    }

    impl UsbIpHeader {
        const SIZE: usize = 20;

        fn new(command: UsbIpCommand, seqnum: u32, devid: u32) -> Self {
            Self {
                command: command as u32,
                seqnum,
                devid,
                direction: 0,
                ep: 0,
            }
        }

        fn read_from<R: Read>(reader: &mut R) -> std::io::Result<Self> {
            Ok(Self {
                command: reader.read_u32::<BigEndian>()?,
                seqnum: reader.read_u32::<BigEndian>()?,
                devid: reader.read_u32::<BigEndian>()?,
                direction: reader.read_u32::<BigEndian>()?,
                ep: reader.read_u32::<BigEndian>()?,
            })
        }

        fn write_to<W: Write>(&self, writer: &mut W) -> std::io::Result<()> {
            writer.write_u32::<BigEndian>(self.command)?;
            writer.write_u32::<BigEndian>(self.seqnum)?;
            writer.write_u32::<BigEndian>(self.devid)?;
            writer.write_u32::<BigEndian>(self.direction)?;
            writer.write_u32::<BigEndian>(self.ep)?;
            Ok(())
        }
    }

    // CMD_SUBMIT payload (28 bytes)
    struct UsbIpCmdSubmit {
        transfer_flags: u32,
        transfer_buffer_length: u32,
        start_frame: u32,
        number_of_packets: u32,
        interval: u32,
        setup: [u8; 8],
    }

    impl UsbIpCmdSubmit {
        const SIZE: usize = 28;

        fn read_from<R: Read>(reader: &mut R) -> std::io::Result<Self> {
            let transfer_flags = reader.read_u32::<BigEndian>()?;
            let transfer_buffer_length = reader.read_u32::<BigEndian>()?;
            let start_frame = reader.read_u32::<BigEndian>()?;
            let number_of_packets = reader.read_u32::<BigEndian>()?;
            let interval = reader.read_u32::<BigEndian>()?;

            let mut setup = [0u8; 8];
            reader.read_exact(&mut setup)?;

            Ok(Self {
                transfer_flags,
                transfer_buffer_length,
                start_frame,
                number_of_packets,
                interval,
                setup,
            })
        }

        fn write_to<W: Write>(&self, writer: &mut W) -> std::io::Result<()> {
            writer.write_u32::<BigEndian>(self.transfer_flags)?;
            writer.write_u32::<BigEndian>(self.transfer_buffer_length)?;
            writer.write_u32::<BigEndian>(self.start_frame)?;
            writer.write_u32::<BigEndian>(self.number_of_packets)?;
            writer.write_u32::<BigEndian>(self.interval)?;
            writer.write_all(&self.setup)?;
            Ok(())
        }
    }

    // RET_SUBMIT payload (20 bytes)
    struct UsbIpRetSubmit {
        status: i32,
        actual_length: u32,
        start_frame: u32,
        number_of_packets: u32,
        error_count: u32,
    }

    impl UsbIpRetSubmit {
        const SIZE: usize = 20;

        fn success(actual_length: u32) -> Self {
            Self {
                status: 0,
                actual_length,
                start_frame: 0,
                number_of_packets: 0,
                error_count: 0,
            }
        }

        fn error(status: i32) -> Self {
            Self {
                status,
                actual_length: 0,
                start_frame: 0,
                number_of_packets: 0,
                error_count: 0,
            }
        }

        fn write_to<W: Write>(&self, writer: &mut W) -> std::io::Result<()> {
            writer.write_i32::<BigEndian>(self.status)?;
            writer.write_i32::<BigEndian>(self.actual_length as i32)?;
            writer.write_i32::<BigEndian>(self.start_frame as i32)?;
            writer.write_i32::<BigEndian>(self.number_of_packets as i32)?;
            writer.write_i32::<BigEndian>(self.error_count as i32)?;
            Ok(())
        }
    }

    #[test]
    fn test_usbip_header_size() {
        assert_eq!(UsbIpHeader::SIZE, 20);
    }

    #[test]
    fn test_usbip_cmd_submit_size() {
        assert_eq!(UsbIpCmdSubmit::SIZE, 28);
    }

    #[test]
    fn test_usbip_ret_submit_size() {
        assert_eq!(UsbIpRetSubmit::SIZE, 20);
    }

    #[test]
    fn test_usbip_header_roundtrip() {
        let header = UsbIpHeader::new(UsbIpCommand::CmdSubmit, 42, 1);

        let mut buf = Vec::new();
        header.write_to(&mut buf).expect("Failed to write");

        assert_eq!(buf.len(), UsbIpHeader::SIZE);

        let mut cursor = Cursor::new(buf);
        let decoded = UsbIpHeader::read_from(&mut cursor).expect("Failed to read");

        assert_eq!(decoded.command, header.command);
        assert_eq!(decoded.seqnum, header.seqnum);
        assert_eq!(decoded.devid, header.devid);
    }

    #[test]
    fn test_usbip_header_command_types() {
        let commands = [
            (UsbIpCommand::CmdSubmit, 0x0001u16),
            (UsbIpCommand::RetSubmit, 0x0003),
            (UsbIpCommand::CmdUnlink, 0x0002),
            (UsbIpCommand::RetUnlink, 0x0004),
        ];

        for (cmd, value) in commands {
            let header = UsbIpHeader::new(cmd, 0, 0);
            assert_eq!(header.command as u16, value);

            let parsed = UsbIpCommand::from_u16(value).expect("Failed to parse");
            assert_eq!(parsed, cmd);
        }
    }

    #[test]
    fn test_usbip_cmd_submit_roundtrip() {
        let setup = create_mock_setup_packet(0x80, 0x06, 0x0100, 0x0000, 0x0012);

        let cmd = UsbIpCmdSubmit {
            transfer_flags: 0,
            transfer_buffer_length: 18,
            start_frame: 0,
            number_of_packets: 0,
            interval: 0,
            setup,
        };

        let mut buf = Vec::new();
        cmd.write_to(&mut buf).expect("Failed to write");

        assert_eq!(buf.len(), UsbIpCmdSubmit::SIZE);

        let mut cursor = Cursor::new(buf);
        let decoded = UsbIpCmdSubmit::read_from(&mut cursor).expect("Failed to read");

        assert_eq!(decoded.transfer_buffer_length, 18);
        assert_eq!(decoded.setup, setup);
    }

    #[test]
    fn test_usbip_ret_submit_success() {
        let ret = UsbIpRetSubmit::success(64);

        assert_eq!(ret.status, 0);
        assert_eq!(ret.actual_length, 64);
        assert_eq!(ret.error_count, 0);

        let mut buf = Vec::new();
        ret.write_to(&mut buf).expect("Failed to write");

        assert_eq!(buf.len(), UsbIpRetSubmit::SIZE);
    }

    #[test]
    fn test_usbip_ret_submit_error() {
        // Test various error codes
        let error_codes = [
            (-110, "ETIMEDOUT"),
            (-32, "EPIPE"),
            (-19, "ENODEV"),
            (-22, "EINVAL"),
            (-16, "EBUSY"),
            (-75, "EOVERFLOW"),
            (-5, "EIO"),
            (-13, "EACCES"),
            (-2, "ENOENT"),
        ];

        for (errno, _name) in error_codes {
            let ret = UsbIpRetSubmit::error(errno);
            assert_eq!(ret.status, errno);
            assert_eq!(ret.actual_length, 0);
        }
    }

    #[test]
    fn test_usbip_version_constant() {
        assert_eq!(USBIP_VERSION, 0x0111);
    }

    #[test]
    fn test_usbip_op_codes() {
        assert_eq!(OP_REQ_IMPORT, 0x8003);
        assert_eq!(OP_REP_IMPORT, 0x0003);
    }

    #[test]
    fn test_device_speed_mapping() {
        let speeds = [
            (DeviceSpeed::Low, 1u32),
            (DeviceSpeed::Full, 2),
            (DeviceSpeed::High, 3),
            (DeviceSpeed::Super, 5),
            (DeviceSpeed::SuperPlus, 6),
        ];

        for (speed, expected) in speeds {
            let value = match speed {
                DeviceSpeed::Low => 1,
                DeviceSpeed::Full => 2,
                DeviceSpeed::High => 3,
                DeviceSpeed::Super => 5,
                DeviceSpeed::SuperPlus => 6,
            };
            assert_eq!(value, expected);
        }
    }

    #[test]
    fn test_usb_error_to_errno_mapping() {
        use protocol::UsbError;

        let mappings = [
            (UsbError::Timeout, -110),
            (UsbError::Pipe, -32),
            (UsbError::NoDevice, -19),
            (UsbError::InvalidParam, -22),
            (UsbError::Busy, -16),
            (UsbError::Overflow, -75),
            (UsbError::Io, -5),
            (UsbError::Access, -13),
            (UsbError::NotFound, -2),
        ];

        for (error, expected_errno) in mappings {
            let errno = match error {
                UsbError::Timeout => -110,
                UsbError::Pipe => -32,
                UsbError::NoDevice => -19,
                UsbError::InvalidParam => -22,
                UsbError::Busy => -16,
                UsbError::Overflow => -75,
                UsbError::Io => -5,
                UsbError::Access => -13,
                UsbError::NotFound => -2,
                UsbError::Other { .. } => -5,
            };
            assert_eq!(errno, expected_errno);
        }
    }
}

// ============================================================================
// Device Info and Descriptor Tests
// ============================================================================

#[test]
fn test_device_info_creation() {
    let device = create_mock_device_info(1, 0x1234, 0x5678);

    assert_eq!(device.id.0, 1);
    assert_eq!(device.vendor_id, 0x1234);
    assert_eq!(device.product_id, 0x5678);
    assert!(device.manufacturer.is_some());
    assert!(device.product.is_some());
    assert!(device.serial_number.is_some());
}

#[test]
fn test_device_descriptor_format() {
    let descriptor = create_mock_device_descriptor();

    assert_eq!(descriptor.len(), 18);
    assert_eq!(descriptor[0], 0x12); // bLength
    assert_eq!(descriptor[1], 0x01); // bDescriptorType (Device)

    // USB 2.0
    assert_eq!(descriptor[2], 0x00);
    assert_eq!(descriptor[3], 0x02);

    // bMaxPacketSize0 = 64
    assert_eq!(descriptor[7], 0x40);

    // bNumConfigurations = 1
    assert_eq!(descriptor[17], 0x01);
}

// ============================================================================
// Transfer Type Tests
// ============================================================================

#[test]
fn test_control_transfer_type() {
    let control = TransferType::Control {
        request_type: 0x80,
        request: 0x06,
        value: 0x0100,
        index: 0,
        data: vec![],
    };

    if let TransferType::Control {
        request_type,
        request,
        value,
        index,
        ..
    } = control
    {
        assert_eq!(request_type, 0x80);
        assert_eq!(request, 0x06);
        assert_eq!(value, 0x0100);
        assert_eq!(index, 0);
    }
}

#[test]
fn test_bulk_transfer_type() {
    let bulk = TransferType::Bulk {
        endpoint: 0x81,
        data: vec![0; 512],
        timeout_ms: 5000,
        checksum: None,
    };

    if let TransferType::Bulk {
        endpoint,
        data,
        timeout_ms,
        checksum: _,
    } = bulk
    {
        assert_eq!(endpoint, 0x81);
        assert_eq!(data.len(), 512);
        assert_eq!(timeout_ms, 5000);
    }
}

#[test]
fn test_interrupt_transfer_type() {
    let interrupt = TransferType::Interrupt {
        endpoint: 0x82,
        data: vec![0; 8],
        timeout_ms: 100,
    };

    if let TransferType::Interrupt {
        endpoint,
        data,
        timeout_ms,
    } = interrupt
    {
        assert_eq!(endpoint, 0x82);
        assert_eq!(data.len(), 8);
        assert_eq!(timeout_ms, 100);
    }
}

// ============================================================================
// Transfer Result Tests
// ============================================================================

#[test]
fn test_transfer_result_success() {
    let success = TransferResult::Success {
        data: vec![1, 2, 3, 4],
        checksum: None,
    };

    if let TransferResult::Success { data, .. } = success {
        assert_eq!(data.len(), 4);
    }
}

#[test]
fn test_transfer_result_errors() {
    let errors = [
        protocol::UsbError::Timeout,
        protocol::UsbError::Pipe,
        protocol::UsbError::NoDevice,
        protocol::UsbError::NotFound,
        protocol::UsbError::Busy,
        protocol::UsbError::Overflow,
        protocol::UsbError::Io,
        protocol::UsbError::InvalidParam,
        protocol::UsbError::Access,
        protocol::UsbError::Other {
            message: "Test error".to_string(),
        },
    ];

    for error in errors {
        let result = TransferResult::Error {
            error: error.clone(),
        };
        if let TransferResult::Error { error: e } = result {
            assert_eq!(e, error);
        }
    }
}

// ============================================================================
// Protocol Message Construction Tests
// ============================================================================

#[test]
fn test_list_devices_request_construction() {
    let msg = protocol::Message {
        version: protocol::CURRENT_VERSION,
        payload: protocol::MessagePayload::ListDevicesRequest,
    };

    assert_eq!(msg.version, protocol::CURRENT_VERSION);
    matches!(msg.payload, protocol::MessagePayload::ListDevicesRequest);
}

#[test]
fn test_attach_device_request_construction() {
    let msg = protocol::Message {
        version: protocol::CURRENT_VERSION,
        payload: protocol::MessagePayload::AttachDeviceRequest {
            device_id: DeviceId(42),
        },
    };

    if let protocol::MessagePayload::AttachDeviceRequest { device_id } = msg.payload {
        assert_eq!(device_id.0, 42);
    }
}

#[test]
fn test_submit_transfer_construction() {
    let request = protocol::UsbRequest {
        id: RequestId(12345),
        handle: DeviceHandle(1),
        transfer: TransferType::Control {
            request_type: 0x80,
            request: 0x06,
            value: 0x0100,
            index: 0,
            data: vec![],
        },
    };

    let msg = protocol::Message {
        version: protocol::CURRENT_VERSION,
        payload: protocol::MessagePayload::SubmitTransfer { request },
    };

    if let protocol::MessagePayload::SubmitTransfer { request } = msg.payload {
        assert_eq!(request.id.0, 12345);
        assert_eq!(request.handle.0, 1);
    }
}

// ============================================================================
// Timeout and Error Handling Tests
// ============================================================================

#[tokio::test]
async fn test_timeout_wrapper_success() {
    let result = with_timeout(DEFAULT_TEST_TIMEOUT, async { 42 }).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 42);
}

#[tokio::test]
async fn test_timeout_wrapper_failure() {
    let result = with_timeout(Duration::from_millis(10), async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        42
    })
    .await;
    assert!(result.is_err());
}

// ============================================================================
// ID Equality Tests
// ============================================================================

#[test]
fn test_device_id_equality() {
    let id1 = DeviceId(42);
    let id2 = DeviceId(42);
    let id3 = DeviceId(43);

    assert_eq!(id1, id2);
    assert_ne!(id1, id3);
}

#[test]
fn test_device_handle_equality() {
    let h1 = DeviceHandle(100);
    let h2 = DeviceHandle(100);
    let h3 = DeviceHandle(101);

    assert_eq!(h1, h2);
    assert_ne!(h1, h3);
}

#[test]
fn test_request_id_equality() {
    let r1 = RequestId(1000);
    let r2 = RequestId(1000);
    let r3 = RequestId(1001);

    assert_eq!(r1, r2);
    assert_ne!(r1, r3);
}

// ============================================================================
// Device Speed Tests
// ============================================================================

#[test]
fn test_device_speed_variants() {
    let speeds = [
        DeviceSpeed::Low,
        DeviceSpeed::Full,
        DeviceSpeed::High,
        DeviceSpeed::Super,
        DeviceSpeed::SuperPlus,
    ];

    assert_eq!(speeds.len(), 5);

    assert_eq!(DeviceSpeed::High, DeviceSpeed::High);
    assert_ne!(DeviceSpeed::Low, DeviceSpeed::High);
}

// ============================================================================
// Mock Device Info Exchange Tests
// ============================================================================

#[test]
fn test_mock_device_info_creation_and_validation() {
    let device = create_mock_device_info(1, 0x1234, 0x5678);

    // Validate all required fields are set
    assert_eq!(device.id.0, 1);
    assert_eq!(device.vendor_id, 0x1234);
    assert_eq!(device.product_id, 0x5678);
    assert!(device.bus_number > 0);
    assert!(device.num_configurations >= 1);

    // Validate optional fields
    assert!(device.manufacturer.is_some());
    assert!(device.product.is_some());
    assert!(device.serial_number.is_some());
}

#[test]
fn test_mock_device_list_serialization_roundtrip() {
    use protocol::{CURRENT_VERSION, Message, MessagePayload, decode_message, encode_message};

    // Create mock device list
    let devices: Vec<_> = (1..=5)
        .map(|i| create_mock_device_info(i, 0x1000 + i as u16, 0x2000 + i as u16))
        .collect();

    // Wrap in protocol message
    let msg = Message {
        version: CURRENT_VERSION,
        payload: MessagePayload::ListDevicesResponse {
            devices: devices.clone(),
        },
    };

    // Serialize and deserialize
    let bytes = encode_message(&msg).expect("Failed to encode");
    let decoded = decode_message(&bytes).expect("Failed to decode");

    // Verify
    if let MessagePayload::ListDevicesResponse {
        devices: decoded_devices,
    } = decoded.payload
    {
        assert_eq!(decoded_devices.len(), 5);
        for (original, decoded) in devices.iter().zip(decoded_devices.iter()) {
            assert_eq!(original.id, decoded.id);
            assert_eq!(original.vendor_id, decoded.vendor_id);
            assert_eq!(original.product_id, decoded.product_id);
            assert_eq!(original.manufacturer, decoded.manufacturer);
            assert_eq!(original.product, decoded.product);
        }
    } else {
        panic!("Expected ListDevicesResponse payload");
    }
}

#[test]
fn test_mock_device_info_with_empty_optionals() {
    use protocol::{DeviceId, DeviceInfo, DeviceSpeed};

    // Create device with no optional fields
    let device = DeviceInfo {
        id: DeviceId(1),
        vendor_id: 0x1234,
        product_id: 0x5678,
        bus_number: 1,
        device_address: 1,
        manufacturer: None,
        product: None,
        serial_number: None,
        class: 0x00,
        subclass: 0x00,
        protocol: 0x00,
        speed: DeviceSpeed::High,
        num_configurations: 1,
    };

    // Should serialize/deserialize correctly
    use protocol::{CURRENT_VERSION, Message, MessagePayload, decode_message, encode_message};

    let msg = Message {
        version: CURRENT_VERSION,
        payload: MessagePayload::ListDevicesResponse {
            devices: vec![device],
        },
    };

    let bytes = encode_message(&msg).expect("Failed to encode");
    let decoded = decode_message(&bytes).expect("Failed to decode");

    if let MessagePayload::ListDevicesResponse { devices } = decoded.payload {
        assert!(devices[0].manufacturer.is_none());
        assert!(devices[0].product.is_none());
        assert!(devices[0].serial_number.is_none());
    } else {
        panic!("Expected ListDevicesResponse");
    }
}

// ============================================================================
// Simulated Client-Server Communication Tests
// ============================================================================

#[test]
fn test_simulated_discovery_flow() {
    use protocol::{CURRENT_VERSION, Message, MessagePayload, decode_framed, encode_framed};

    // Step 1: Client sends ListDevicesRequest
    let request = Message {
        version: CURRENT_VERSION,
        payload: MessagePayload::ListDevicesRequest,
    };
    let request_bytes = encode_framed(&request).expect("Failed to encode request");

    // Step 2: "Server" receives and processes request
    let server_received = decode_framed(&request_bytes).expect("Failed to decode");
    assert!(matches!(
        server_received.payload,
        MessagePayload::ListDevicesRequest
    ));

    // Step 3: "Server" sends response with devices
    let response = Message {
        version: CURRENT_VERSION,
        payload: MessagePayload::ListDevicesResponse {
            devices: vec![
                create_mock_device_info(1, 0x1234, 0x5678),
                create_mock_device_info(2, 0xABCD, 0xEF01),
            ],
        },
    };
    let response_bytes = encode_framed(&response).expect("Failed to encode response");

    // Step 4: Client receives and processes response
    let client_received = decode_framed(&response_bytes).expect("Failed to decode");
    if let MessagePayload::ListDevicesResponse { devices } = client_received.payload {
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].vendor_id, 0x1234);
        assert_eq!(devices[1].vendor_id, 0xABCD);
    } else {
        panic!("Expected ListDevicesResponse");
    }
}

#[test]
fn test_simulated_attach_detach_flow() {
    use protocol::{
        CURRENT_VERSION, DeviceHandle, DeviceId, Message, MessagePayload, decode_framed,
        encode_framed,
    };

    // Step 1: Client sends AttachDeviceRequest
    let attach_request = Message {
        version: CURRENT_VERSION,
        payload: MessagePayload::AttachDeviceRequest {
            device_id: DeviceId(42),
        },
    };
    let attach_bytes = encode_framed(&attach_request).expect("Failed to encode");
    let _ = decode_framed(&attach_bytes).expect("Server should decode");

    // Step 2: Server responds with success
    let attach_response = Message {
        version: CURRENT_VERSION,
        payload: MessagePayload::AttachDeviceResponse {
            result: Ok(DeviceHandle(100)),
        },
    };
    let response_bytes = encode_framed(&attach_response).expect("Failed to encode");
    let client_attach = decode_framed(&response_bytes).expect("Client should decode");

    let handle = if let MessagePayload::AttachDeviceResponse { result } = client_attach.payload {
        result.expect("Attach should succeed")
    } else {
        panic!("Expected AttachDeviceResponse");
    };
    assert_eq!(handle.0, 100);

    // Step 3: Client sends DetachDeviceRequest
    let detach_request = Message {
        version: CURRENT_VERSION,
        payload: MessagePayload::DetachDeviceRequest { handle },
    };
    let detach_bytes = encode_framed(&detach_request).expect("Failed to encode");
    let _ = decode_framed(&detach_bytes).expect("Server should decode");

    // Step 4: Server responds with success
    let detach_response = Message {
        version: CURRENT_VERSION,
        payload: MessagePayload::DetachDeviceResponse { result: Ok(()) },
    };
    let response_bytes = encode_framed(&detach_response).expect("Failed to encode");
    let client_detach = decode_framed(&response_bytes).expect("Client should decode");

    if let MessagePayload::DetachDeviceResponse { result } = client_detach.payload {
        assert!(result.is_ok());
    } else {
        panic!("Expected DetachDeviceResponse");
    }
}

#[test]
fn test_simulated_transfer_flow() {
    use protocol::{
        CURRENT_VERSION, DeviceHandle, Message, MessagePayload, RequestId, TransferResult,
        TransferType, UsbRequest, UsbResponse, decode_framed, encode_framed,
    };

    // Step 1: Client submits a control transfer
    let submit = Message {
        version: CURRENT_VERSION,
        payload: MessagePayload::SubmitTransfer {
            request: UsbRequest {
                id: RequestId(12345),
                handle: DeviceHandle(100),
                transfer: TransferType::Control {
                    request_type: 0x80, // Device-to-host, Standard, Device
                    request: 0x06,      // GET_DESCRIPTOR
                    value: 0x0100,      // Device descriptor
                    index: 0,
                    data: vec![],
                },
            },
        },
    };
    let submit_bytes = encode_framed(&submit).expect("Failed to encode");
    let server_received = decode_framed(&submit_bytes).expect("Failed to decode");

    // Verify server received correct request
    if let MessagePayload::SubmitTransfer { request } = server_received.payload {
        assert_eq!(request.id.0, 12345);
        assert_eq!(request.handle.0, 100);
    } else {
        panic!("Expected SubmitTransfer");
    }

    // Step 2: Server sends transfer complete
    let complete = Message {
        version: CURRENT_VERSION,
        payload: MessagePayload::TransferComplete {
            response: UsbResponse {
                id: RequestId(12345),
                result: TransferResult::Success {
                    data: create_mock_device_descriptor(),
                checksum: None,
                },
            },
        },
    };
    let complete_bytes = encode_framed(&complete).expect("Failed to encode");
    let client_received = decode_framed(&complete_bytes).expect("Failed to decode");

    // Verify client received correct response
    if let MessagePayload::TransferComplete { response } = client_received.payload {
        assert_eq!(response.id.0, 12345);
        if let TransferResult::Success { data, .. } = response.result {
            assert_eq!(data.len(), 18); // Device descriptor size
            assert_eq!(data[0], 0x12); // bLength
            assert_eq!(data[1], 0x01); // bDescriptorType (Device)
        } else {
            panic!("Expected success result");
        }
    } else {
        panic!("Expected TransferComplete");
    }
}

#[test]
fn test_simulated_error_responses() {
    use protocol::{
        AttachError, CURRENT_VERSION, DetachError, Message, MessagePayload, RequestId,
        TransferResult, UsbError, UsbResponse, decode_framed, encode_framed,
    };

    // Test attach errors
    let attach_errors = [
        AttachError::DeviceNotFound,
        AttachError::AlreadyAttached,
        AttachError::PermissionDenied,
        AttachError::Other {
            message: "Test error".to_string(),
        },
    ];

    for error in attach_errors {
        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::AttachDeviceResponse {
                result: Err(error.clone()),
            },
        };
        let bytes = encode_framed(&msg).expect("Failed to encode");
        let decoded = decode_framed(&bytes).expect("Failed to decode");

        if let MessagePayload::AttachDeviceResponse { result } = decoded.payload {
            assert!(result.is_err());
            assert_eq!(result.unwrap_err(), error);
        } else {
            panic!("Expected AttachDeviceResponse");
        }
    }

    // Test detach errors
    let detach_errors = [
        DetachError::HandleNotFound,
        DetachError::Other {
            message: "Detach failed".to_string(),
        },
    ];

    for error in detach_errors {
        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::DetachDeviceResponse {
                result: Err(error.clone()),
            },
        };
        let bytes = encode_framed(&msg).expect("Failed to encode");
        let decoded = decode_framed(&bytes).expect("Failed to decode");

        if let MessagePayload::DetachDeviceResponse { result } = decoded.payload {
            assert!(result.is_err());
            assert_eq!(result.unwrap_err(), error);
        } else {
            panic!("Expected DetachDeviceResponse");
        }
    }

    // Test transfer errors
    let transfer_errors = [
        UsbError::Timeout,
        UsbError::Pipe,
        UsbError::NoDevice,
        UsbError::NotFound,
        UsbError::Busy,
        UsbError::Overflow,
        UsbError::Io,
        UsbError::InvalidParam,
        UsbError::Access,
        UsbError::Other {
            message: "Unknown error".to_string(),
        },
    ];

    for error in transfer_errors {
        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::TransferComplete {
                response: UsbResponse {
                    id: RequestId(1),
                    result: TransferResult::Error {
                        error: error.clone(),
                    },
                },
            },
        };
        let bytes = encode_framed(&msg).expect("Failed to encode");
        let decoded = decode_framed(&bytes).expect("Failed to decode");

        if let MessagePayload::TransferComplete { response } = decoded.payload {
            if let TransferResult::Error {
                error: decoded_error,
            } = response.result
            {
                assert_eq!(decoded_error, error);
            } else {
                panic!("Expected error result");
            }
        } else {
            panic!("Expected TransferComplete");
        }
    }
}

// ============================================================================
// Protocol Version Handling Tests
// ============================================================================

#[test]
fn test_protocol_version_compatibility() {
    use protocol::{CURRENT_VERSION, ProtocolVersion, validate_version};

    // Current version should always be compatible
    assert!(validate_version(&CURRENT_VERSION).is_ok());

    // Same major, different minor should be compatible
    let newer_minor = ProtocolVersion {
        major: CURRENT_VERSION.major,
        minor: CURRENT_VERSION.minor + 10,
        patch: 0,
    };
    assert!(validate_version(&newer_minor).is_ok());

    // Different major should be incompatible
    let different_major = ProtocolVersion {
        major: CURRENT_VERSION.major + 1,
        minor: 0,
        patch: 0,
    };
    assert!(validate_version(&different_major).is_err());
}

#[test]
fn test_message_with_different_versions() {
    use protocol::{Message, MessagePayload, ProtocolVersion, decode_message, encode_message};

    // Create message with custom version
    let custom_version = ProtocolVersion {
        major: 1,
        minor: 5,
        patch: 3,
    };

    let msg = Message {
        version: custom_version,
        payload: MessagePayload::Ping,
    };

    let bytes = encode_message(&msg).expect("Failed to encode");
    let decoded = decode_message(&bytes).expect("Failed to decode");

    // Version should be preserved exactly
    assert_eq!(decoded.version.major, 1);
    assert_eq!(decoded.version.minor, 5);
    assert_eq!(decoded.version.patch, 3);
}

// ============================================================================
// Edge Case Tests
// ============================================================================

#[test]
fn test_empty_device_list() {
    use protocol::{CURRENT_VERSION, Message, MessagePayload, decode_framed, encode_framed};

    let msg = Message {
        version: CURRENT_VERSION,
        payload: MessagePayload::ListDevicesResponse { devices: vec![] },
    };

    let bytes = encode_framed(&msg).expect("Failed to encode");
    let decoded = decode_framed(&bytes).expect("Failed to decode");

    if let MessagePayload::ListDevicesResponse { devices } = decoded.payload {
        assert!(devices.is_empty());
    } else {
        panic!("Expected ListDevicesResponse");
    }
}

#[test]
fn test_large_bulk_transfer() {
    use protocol::{
        CURRENT_VERSION, DeviceHandle, Message, MessagePayload, RequestId, TransferType,
        UsbRequest, decode_framed, encode_framed,
    };

    // Test with 64KB bulk transfer (common USB buffer size)
    let large_data = vec![0xAB; 64 * 1024];

    let msg = Message {
        version: CURRENT_VERSION,
        payload: MessagePayload::SubmitTransfer {
            request: UsbRequest {
                id: RequestId(1),
                handle: DeviceHandle(1),
                transfer: TransferType::Bulk {
                    endpoint: 0x02,
                    data: large_data.clone(),
                    timeout_ms: 30000,
                    checksum: None,
                },
            },
        },
    };

    let bytes = encode_framed(&msg).expect("Failed to encode 64KB transfer");
    let decoded = decode_framed(&bytes).expect("Failed to decode 64KB transfer");

    if let MessagePayload::SubmitTransfer { request } = decoded.payload {
        if let TransferType::Bulk { data, .. } = request.transfer {
            assert_eq!(data.len(), 64 * 1024);
            assert!(data.iter().all(|&b| b == 0xAB));
        } else {
            panic!("Expected Bulk transfer");
        }
    } else {
        panic!("Expected SubmitTransfer");
    }
}

#[test]
fn test_ping_pong_roundtrip() {
    use protocol::{CURRENT_VERSION, Message, MessagePayload, decode_framed, encode_framed};

    // Test Ping
    let ping = Message {
        version: CURRENT_VERSION,
        payload: MessagePayload::Ping,
    };
    let ping_bytes = encode_framed(&ping).expect("Failed to encode");
    let ping_decoded = decode_framed(&ping_bytes).expect("Failed to decode");
    assert!(matches!(ping_decoded.payload, MessagePayload::Ping));

    // Test Pong
    let pong = Message {
        version: CURRENT_VERSION,
        payload: MessagePayload::Pong,
    };
    let pong_bytes = encode_framed(&pong).expect("Failed to encode");
    let pong_decoded = decode_framed(&pong_bytes).expect("Failed to decode");
    assert!(matches!(pong_decoded.payload, MessagePayload::Pong));
}

#[test]
fn test_error_message_roundtrip() {
    use protocol::{CURRENT_VERSION, Message, MessagePayload, decode_framed, encode_framed};

    let error_messages = [
        "Simple error",
        "Error with unicode: test",
        "Error with newlines:\nLine 2\nLine 3",
        &"A".repeat(1000), // Long error
        "",                // Empty error
    ];

    for error_msg in error_messages {
        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::Error {
                message: error_msg.to_string(),
            },
        };

        let bytes = encode_framed(&msg).expect("Failed to encode");
        let decoded = decode_framed(&bytes).expect("Failed to decode");

        if let MessagePayload::Error { message } = decoded.payload {
            assert_eq!(message, error_msg);
        } else {
            panic!("Expected Error payload");
        }
    }
}
