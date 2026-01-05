//! Client Integration Tests
//!
//! Comprehensive tests for the client crate covering:
//! - Configuration loading and validation
//! - Virtual USB port management
//! - USB/IP message serialization
//! - DeviceProxy operations
//!
//! Run with: `cargo test -p client --test integration_tests`

use client::config::ClientConfig;
use common::test_utils::{
    create_mock_device_descriptor, create_mock_device_info, create_mock_setup_packet,
    with_timeout, DEFAULT_TEST_TIMEOUT,
};
use protocol::{DeviceHandle, DeviceId, DeviceSpeed, RequestId, TransferResult, TransferType};
use std::time::Duration;
use tempfile::tempdir;

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
    config.save(&config_path).expect("Failed to save config");

    // Verify file exists
    assert!(config_path.exists());

    // Load config
    let loaded =
        ClientConfig::load(Some(config_path.clone())).expect("Failed to load config");

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

#[test]
fn test_client_config_load_or_default() {
    // When no config file exists, should return defaults
    let config = ClientConfig::load_or_default();

    assert_eq!(config.client.log_level, "info");
    assert!(config.client.auto_connect);
}

// ============================================================================
// USB/IP Protocol Tests (Linux only)
// ============================================================================

#[cfg(target_os = "linux")]
mod usbip_tests {
    use super::*;
    use client::virtual_usb::usbip_protocol::*;
    use std::io::Cursor;

    #[test]
    fn test_usbip_header_size() {
        // USB/IP basic header is 20 bytes (5 x u32)
        assert_eq!(UsbIpHeader::SIZE, 20);
    }

    #[test]
    fn test_usbip_cmd_submit_size() {
        // CMD_SUBMIT payload is 28 bytes (5 x u32 + 8-byte setup)
        assert_eq!(UsbIpCmdSubmit::SIZE, 28);
    }

    #[test]
    fn test_usbip_ret_submit_size() {
        // RET_SUBMIT payload is 20 bytes (5 x i32)
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
            (UsbIpCommand::CmdSubmit, 0x0001),
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
    fn test_usbip_ret_unlink() {
        let success = UsbIpRetUnlink::success();
        assert_eq!(success.status, 0);

        let not_found = UsbIpRetUnlink::not_found();
        assert_eq!(not_found.status, -2); // ENOENT

        // Test write
        let mut buf = Vec::new();
        success.write_to(&mut buf).expect("Failed to write");
        assert_eq!(buf.len(), UsbIpRetUnlink::SIZE);
    }

    #[test]
    fn test_usbip_req_import() {
        let busid = "1-1.2";
        let req = UsbIpReqImport::new(busid);

        assert_eq!(req.version, USBIP_VERSION);
        assert_eq!(req.command, OP_REQ_IMPORT);
        assert_eq!(req.status, 0);

        // busid should be null-terminated in 32-byte field
        assert_eq!(&req.busid[..busid.len()], busid.as_bytes());
        assert_eq!(req.busid[busid.len()], 0);

        // Write should succeed
        let mut buf = Vec::new();
        req.write_to(&mut buf).expect("Failed to write");
        assert_eq!(buf.len(), 40); // 2 + 2 + 4 + 32
    }

    #[test]
    fn test_usbip_rep_import() {
        let device = create_mock_device_info(1, 0x1234, 0x5678);
        let busid = "1-1";

        let rep = UsbIpRepImport::from_device_info(&device, busid);

        assert_eq!(rep.version, USBIP_VERSION);
        assert_eq!(rep.command, OP_REP_IMPORT);
        assert_eq!(rep.status, 0);
        assert_eq!(rep.id_vendor, 0x1234);
        assert_eq!(rep.id_product, 0x5678);

        // Write should succeed
        let mut buf = Vec::new();
        rep.write_to(&mut buf).expect("Failed to write");
        // Total size: header (8) + udev_path (256) + busid (32) + device info
        assert!(buf.len() > 300);
    }

    #[test]
    fn test_usbip_response_conversion() {
        use protocol::{TransferResult, UsbError, UsbResponse};

        // Success response
        let success_response = UsbResponse {
            id: RequestId(1),
            result: TransferResult::Success {
                data: vec![0x12, 0x01, 0x00, 0x02],
            },
        };

        let (ret, data) = usb_response_to_usbip(&success_response);
        assert_eq!(ret.status, 0);
        assert_eq!(ret.actual_length, 4);
        assert_eq!(data.len(), 4);

        // Error responses
        let error_cases = [
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

        for (error, expected_errno) in error_cases {
            let error_response = UsbResponse {
                id: RequestId(1),
                result: TransferResult::Error { error },
            };

            let (ret, data) = usb_response_to_usbip(&error_response);
            assert_eq!(ret.status, expected_errno);
            assert_eq!(ret.actual_length, 0);
            assert!(data.is_empty());
        }
    }

    #[test]
    fn test_device_speed_to_usbip() {
        let speeds = [
            (DeviceSpeed::Low, 1),
            (DeviceSpeed::Full, 2),
            (DeviceSpeed::High, 3),
            (DeviceSpeed::Super, 5),
            (DeviceSpeed::SuperPlus, 6),
        ];

        for (speed, expected) in speeds {
            let mut device = create_mock_device_info(1, 0x1234, 0x5678);
            device.speed = speed;

            let rep = UsbIpRepImport::from_device_info(&device, "1-1");
            assert_eq!(rep.speed, expected);
        }
    }
}

// ============================================================================
// DeviceProxy Tests (mock based)
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

#[test]
fn test_transfer_types() {
    // Control transfer
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

    // Bulk transfer
    let bulk = TransferType::Bulk {
        endpoint: 0x81,
        data: vec![0; 512],
        timeout_ms: 5000,
    };

    if let TransferType::Bulk {
        endpoint,
        data,
        timeout_ms,
    } = bulk
    {
        assert_eq!(endpoint, 0x81);
        assert_eq!(data.len(), 512);
        assert_eq!(timeout_ms, 5000);
    }

    // Interrupt transfer
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

#[test]
fn test_transfer_result_types() {
    // Success with data
    let success = TransferResult::Success {
        data: vec![1, 2, 3, 4],
    };

    if let TransferResult::Success { data } = success {
        assert_eq!(data.len(), 4);
    }

    // Various error types
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
// Protocol Message Tests
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
async fn test_timeout_wrapper() {
    // Should succeed within timeout
    let result = with_timeout(DEFAULT_TEST_TIMEOUT, async { 42 }).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 42);

    // Should fail on timeout
    let result = with_timeout(Duration::from_millis(10), async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        42
    })
    .await;
    assert!(result.is_err());
}

// ============================================================================
// Device Handle and ID Tests
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

    // Test equality
    assert_eq!(DeviceSpeed::High, DeviceSpeed::High);
    assert_ne!(DeviceSpeed::Low, DeviceSpeed::High);
}
