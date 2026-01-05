//! Protocol Integration Tests
//!
//! Comprehensive tests for the protocol crate covering:
//! - Full message roundtrips for all message types
//! - Large payload handling (64KB+)
//! - Version negotiation and compatibility
//! - Edge cases and error conditions
//!
//! Run with: `cargo test -p protocol`

use protocol::{
    AttachError, CURRENT_VERSION, DetachError, DeviceHandle, DeviceId, DeviceInfo, DeviceSpeed,
    Message, MessagePayload, ProtocolError, ProtocolVersion, RequestId, TransferResult,
    TransferType, UsbError, UsbRequest, UsbResponse, decode_framed, decode_message, encode_framed,
    encode_message, read_framed, validate_version, write_framed,
};
use std::io::Cursor;

// ============================================================================
// Test Utilities
// ============================================================================

/// Create a test DeviceInfo with the given parameters
fn create_test_device(id: u32, vendor_id: u16, product_id: u16) -> DeviceInfo {
    DeviceInfo {
        id: DeviceId(id),
        vendor_id,
        product_id,
        bus_number: 1,
        device_address: 5,
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

/// Create a message with the given payload
fn create_message(payload: MessagePayload) -> Message {
    Message {
        version: CURRENT_VERSION,
        payload,
    }
}

/// Verify message roundtrip (encode -> decode)
fn verify_roundtrip(msg: &Message) {
    let bytes = encode_message(msg).expect("Failed to encode message");
    let decoded = decode_message(&bytes).expect("Failed to decode message");
    assert_eq!(decoded.version, msg.version);
}

/// Verify framed message roundtrip (encode_framed -> decode_framed)
fn verify_framed_roundtrip(msg: &Message) {
    let framed = encode_framed(msg).expect("Failed to encode framed message");
    let decoded = decode_framed(&framed).expect("Failed to decode framed message");
    assert_eq!(decoded.version, msg.version);
}

// ============================================================================
// ListDevices Request/Response Roundtrip Tests
// ============================================================================

#[test]
fn test_list_devices_request_roundtrip() {
    let msg = create_message(MessagePayload::ListDevicesRequest);
    verify_roundtrip(&msg);
    verify_framed_roundtrip(&msg);
}

#[test]
fn test_list_devices_response_empty_roundtrip() {
    let msg = create_message(MessagePayload::ListDevicesResponse { devices: vec![] });
    verify_roundtrip(&msg);
    verify_framed_roundtrip(&msg);
}

#[test]
fn test_list_devices_response_single_device_roundtrip() {
    let devices = vec![create_test_device(1, 0x1234, 0x5678)];
    let msg = create_message(MessagePayload::ListDevicesResponse { devices });

    let bytes = encode_message(&msg).expect("Failed to encode");
    let decoded = decode_message(&bytes).expect("Failed to decode");

    if let MessagePayload::ListDevicesResponse { devices } = decoded.payload {
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].id.0, 1);
        assert_eq!(devices[0].vendor_id, 0x1234);
        assert_eq!(devices[0].product_id, 0x5678);
        assert_eq!(
            devices[0].manufacturer.as_deref(),
            Some("Test Manufacturer 1")
        );
    } else {
        panic!("Wrong payload type after decode");
    }
}

#[test]
fn test_list_devices_response_multiple_devices_roundtrip() {
    let devices: Vec<DeviceInfo> = (1..=10)
        .map(|i| create_test_device(i, 0x1000 + i as u16, 0x2000 + i as u16))
        .collect();

    let msg = create_message(MessagePayload::ListDevicesResponse {
        devices: devices.clone(),
    });

    let bytes = encode_message(&msg).expect("Failed to encode");
    let decoded = decode_message(&bytes).expect("Failed to decode");

    if let MessagePayload::ListDevicesResponse {
        devices: decoded_devices,
    } = decoded.payload
    {
        assert_eq!(decoded_devices.len(), 10);
        for (i, device) in decoded_devices.iter().enumerate() {
            assert_eq!(device.id.0, (i + 1) as u32);
            assert_eq!(device.vendor_id, 0x1000 + (i + 1) as u16);
        }
    } else {
        panic!("Wrong payload type after decode");
    }
}

#[test]
fn test_list_devices_response_all_speeds() {
    let speeds = [
        DeviceSpeed::Low,
        DeviceSpeed::Full,
        DeviceSpeed::High,
        DeviceSpeed::Super,
        DeviceSpeed::SuperPlus,
    ];

    for (i, speed) in speeds.iter().enumerate() {
        let mut device = create_test_device(i as u32 + 1, 0x1234, 0x5678);
        device.speed = *speed;

        let msg = create_message(MessagePayload::ListDevicesResponse {
            devices: vec![device],
        });

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        if let MessagePayload::ListDevicesResponse { devices } = decoded.payload {
            assert_eq!(devices[0].speed, *speed);
        } else {
            panic!("Wrong payload type");
        }
    }
}

#[test]
fn test_list_devices_response_optional_fields() {
    // Device with no optional fields
    let mut device = create_test_device(1, 0x1234, 0x5678);
    device.manufacturer = None;
    device.product = None;
    device.serial_number = None;

    let msg = create_message(MessagePayload::ListDevicesResponse {
        devices: vec![device],
    });

    let bytes = encode_message(&msg).expect("Failed to encode");
    let decoded = decode_message(&bytes).expect("Failed to decode");

    if let MessagePayload::ListDevicesResponse { devices } = decoded.payload {
        assert!(devices[0].manufacturer.is_none());
        assert!(devices[0].product.is_none());
        assert!(devices[0].serial_number.is_none());
    } else {
        panic!("Wrong payload type");
    }
}

// ============================================================================
// AttachDevice Request/Response Roundtrip Tests
// ============================================================================

#[test]
fn test_attach_device_request_roundtrip() {
    let msg = create_message(MessagePayload::AttachDeviceRequest {
        device_id: DeviceId(42),
    });

    let bytes = encode_message(&msg).expect("Failed to encode");
    let decoded = decode_message(&bytes).expect("Failed to decode");

    if let MessagePayload::AttachDeviceRequest { device_id } = decoded.payload {
        assert_eq!(device_id.0, 42);
    } else {
        panic!("Wrong payload type");
    }
}

#[test]
fn test_attach_device_response_success_roundtrip() {
    let msg = create_message(MessagePayload::AttachDeviceResponse {
        result: Ok(DeviceHandle(100)),
    });

    let bytes = encode_message(&msg).expect("Failed to encode");
    let decoded = decode_message(&bytes).expect("Failed to decode");

    if let MessagePayload::AttachDeviceResponse { result } = decoded.payload {
        assert!(result.is_ok());
        assert_eq!(result.unwrap().0, 100);
    } else {
        panic!("Wrong payload type");
    }
}

#[test]
fn test_attach_device_response_errors_roundtrip() {
    let errors = [
        AttachError::DeviceNotFound,
        AttachError::AlreadyAttached,
        AttachError::PermissionDenied,
        AttachError::Other {
            message: "Test error message".to_string(),
        },
    ];

    for error in errors {
        let msg = create_message(MessagePayload::AttachDeviceResponse {
            result: Err(error.clone()),
        });

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        if let MessagePayload::AttachDeviceResponse { result } = decoded.payload {
            assert!(result.is_err());
            assert_eq!(result.unwrap_err(), error);
        } else {
            panic!("Wrong payload type");
        }
    }
}

// ============================================================================
// DetachDevice Request/Response Roundtrip Tests
// ============================================================================

#[test]
fn test_detach_device_request_roundtrip() {
    let msg = create_message(MessagePayload::DetachDeviceRequest {
        handle: DeviceHandle(42),
    });

    let bytes = encode_message(&msg).expect("Failed to encode");
    let decoded = decode_message(&bytes).expect("Failed to decode");

    if let MessagePayload::DetachDeviceRequest { handle } = decoded.payload {
        assert_eq!(handle.0, 42);
    } else {
        panic!("Wrong payload type");
    }
}

#[test]
fn test_detach_device_response_success_roundtrip() {
    let msg = create_message(MessagePayload::DetachDeviceResponse { result: Ok(()) });

    let bytes = encode_message(&msg).expect("Failed to encode");
    let decoded = decode_message(&bytes).expect("Failed to decode");

    if let MessagePayload::DetachDeviceResponse { result } = decoded.payload {
        assert!(result.is_ok());
    } else {
        panic!("Wrong payload type");
    }
}

#[test]
fn test_detach_device_response_errors_roundtrip() {
    let errors = [
        DetachError::HandleNotFound,
        DetachError::Other {
            message: "Detach failed".to_string(),
        },
    ];

    for error in errors {
        let msg = create_message(MessagePayload::DetachDeviceResponse {
            result: Err(error.clone()),
        });

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        if let MessagePayload::DetachDeviceResponse { result } = decoded.payload {
            assert!(result.is_err());
            assert_eq!(result.unwrap_err(), error);
        } else {
            panic!("Wrong payload type");
        }
    }
}

// ============================================================================
// USB Transfer Request/Response Roundtrip Tests
// ============================================================================

#[test]
fn test_submit_transfer_control_roundtrip() {
    let request = UsbRequest {
        id: RequestId(12345),
        handle: DeviceHandle(1),
        transfer: TransferType::Control {
            request_type: 0x80,
            request: 0x06,
            value: 0x0100,
            index: 0x0000,
            data: vec![],
        },
    };

    let msg = create_message(MessagePayload::SubmitTransfer { request });

    let bytes = encode_message(&msg).expect("Failed to encode");
    let decoded = decode_message(&bytes).expect("Failed to decode");

    if let MessagePayload::SubmitTransfer { request } = decoded.payload {
        assert_eq!(request.id.0, 12345);
        assert_eq!(request.handle.0, 1);
        if let TransferType::Control {
            request_type,
            request: req,
            value,
            index,
            data,
        } = request.transfer
        {
            assert_eq!(request_type, 0x80);
            assert_eq!(req, 0x06);
            assert_eq!(value, 0x0100);
            assert_eq!(index, 0x0000);
            assert!(data.is_empty());
        } else {
            panic!("Wrong transfer type");
        }
    } else {
        panic!("Wrong payload type");
    }
}

#[test]
fn test_submit_transfer_bulk_roundtrip() {
    let request = UsbRequest {
        id: RequestId(99999),
        handle: DeviceHandle(5),
        transfer: TransferType::Bulk {
            endpoint: 0x81,
            data: vec![0xAB; 512],
            timeout_ms: 5000,
        },
    };

    let msg = create_message(MessagePayload::SubmitTransfer { request });

    let bytes = encode_message(&msg).expect("Failed to encode");
    let decoded = decode_message(&bytes).expect("Failed to decode");

    if let MessagePayload::SubmitTransfer { request } = decoded.payload {
        if let TransferType::Bulk {
            endpoint,
            data,
            timeout_ms,
        } = request.transfer
        {
            assert_eq!(endpoint, 0x81);
            assert_eq!(data.len(), 512);
            assert_eq!(data[0], 0xAB);
            assert_eq!(timeout_ms, 5000);
        } else {
            panic!("Wrong transfer type");
        }
    } else {
        panic!("Wrong payload type");
    }
}

#[test]
fn test_submit_transfer_interrupt_roundtrip() {
    let request = UsbRequest {
        id: RequestId(54321),
        handle: DeviceHandle(2),
        transfer: TransferType::Interrupt {
            endpoint: 0x82,
            data: vec![1, 2, 3, 4, 5, 6, 7, 8],
            timeout_ms: 100,
        },
    };

    let msg = create_message(MessagePayload::SubmitTransfer { request });
    verify_roundtrip(&msg);
    verify_framed_roundtrip(&msg);
}

#[test]
fn test_transfer_complete_success_roundtrip() {
    let response = UsbResponse {
        id: RequestId(12345),
        result: TransferResult::Success {
            data: vec![0x12, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 0x40],
        },
    };

    let msg = create_message(MessagePayload::TransferComplete { response });

    let bytes = encode_message(&msg).expect("Failed to encode");
    let decoded = decode_message(&bytes).expect("Failed to decode");

    if let MessagePayload::TransferComplete { response } = decoded.payload {
        assert_eq!(response.id.0, 12345);
        if let TransferResult::Success { data } = response.result {
            assert_eq!(data.len(), 8);
            assert_eq!(data[0], 0x12); // Device descriptor length
        } else {
            panic!("Wrong result type");
        }
    } else {
        panic!("Wrong payload type");
    }
}

#[test]
fn test_transfer_complete_all_errors_roundtrip() {
    let errors = [
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
            message: "Unknown error occurred".to_string(),
        },
    ];

    for (i, error) in errors.iter().enumerate() {
        let response = UsbResponse {
            id: RequestId(i as u64),
            result: TransferResult::Error {
                error: error.clone(),
            },
        };

        let msg = create_message(MessagePayload::TransferComplete { response });

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        if let MessagePayload::TransferComplete { response } = decoded.payload {
            if let TransferResult::Error {
                error: decoded_error,
            } = response.result
            {
                assert_eq!(decoded_error, *error);
            } else {
                panic!("Expected error result");
            }
        } else {
            panic!("Wrong payload type");
        }
    }
}

// ============================================================================
// Large Transfer Message Handling Tests (64KB+)
// ============================================================================

#[test]
fn test_large_transfer_64kb_payload() {
    let data_64kb = vec![0xCD; 64 * 1024];

    let request = UsbRequest {
        id: RequestId(1),
        handle: DeviceHandle(1),
        transfer: TransferType::Bulk {
            endpoint: 0x02,
            data: data_64kb.clone(),
            timeout_ms: 30000,
        },
    };

    let msg = create_message(MessagePayload::SubmitTransfer { request });

    let bytes = encode_message(&msg).expect("Failed to encode 64KB message");
    let decoded = decode_message(&bytes).expect("Failed to decode 64KB message");

    if let MessagePayload::SubmitTransfer { request } = decoded.payload {
        if let TransferType::Bulk { data, .. } = request.transfer {
            assert_eq!(data.len(), 64 * 1024);
            assert!(data.iter().all(|&b| b == 0xCD));
        } else {
            panic!("Wrong transfer type");
        }
    } else {
        panic!("Wrong payload type");
    }
}

#[test]
fn test_large_transfer_256kb_payload() {
    let data_256kb = vec![0xEF; 256 * 1024];

    let response = UsbResponse {
        id: RequestId(2),
        result: TransferResult::Success { data: data_256kb },
    };

    let msg = create_message(MessagePayload::TransferComplete { response });

    let framed = encode_framed(&msg).expect("Failed to encode 256KB framed message");
    let decoded = decode_framed(&framed).expect("Failed to decode 256KB framed message");

    if let MessagePayload::TransferComplete { response } = decoded.payload {
        if let TransferResult::Success { data } = response.result {
            assert_eq!(data.len(), 256 * 1024);
        } else {
            panic!("Expected success result");
        }
    } else {
        panic!("Wrong payload type");
    }
}

#[test]
fn test_large_device_list_100_devices() {
    let devices: Vec<DeviceInfo> = (1..=100)
        .map(|i| {
            let mut device = create_test_device(i, 0x1000 + i as u16, 0x2000 + i as u16);
            // Add long strings to increase payload size
            device.manufacturer = Some("A".repeat(100));
            device.product = Some("B".repeat(100));
            device.serial_number = Some("C".repeat(50));
            device
        })
        .collect();

    let msg = create_message(MessagePayload::ListDevicesResponse { devices });

    let framed = encode_framed(&msg).expect("Failed to encode large device list");
    let decoded = decode_framed(&framed).expect("Failed to decode large device list");

    if let MessagePayload::ListDevicesResponse { devices } = decoded.payload {
        assert_eq!(devices.len(), 100);
    } else {
        panic!("Wrong payload type");
    }
}

// ============================================================================
// Version Negotiation Tests
// ============================================================================

#[test]
fn test_version_current_is_compatible() {
    assert!(validate_version(&CURRENT_VERSION).is_ok());
}

#[test]
fn test_version_same_major_newer_minor_compatible() {
    let newer_minor = ProtocolVersion {
        major: CURRENT_VERSION.major,
        minor: CURRENT_VERSION.minor + 5,
        patch: 0,
    };
    assert!(validate_version(&newer_minor).is_ok());
}

#[test]
fn test_version_same_major_older_minor_compatible() {
    // Test with minor version 1, which should be compatible with current version
    let older_minor = ProtocolVersion {
        major: CURRENT_VERSION.major,
        minor: 1,
        patch: 0,
    };
    // Both minor 0 and 1 should be compatible with the same major version
    assert!(validate_version(&older_minor).is_ok());
}

#[test]
fn test_version_different_major_incompatible() {
    let incompatible = ProtocolVersion {
        major: CURRENT_VERSION.major + 1,
        minor: 0,
        patch: 0,
    };
    assert!(validate_version(&incompatible).is_err());

    if CURRENT_VERSION.major > 0 {
        let older_major = ProtocolVersion {
            major: CURRENT_VERSION.major - 1,
            minor: 99,
            patch: 0,
        };
        assert!(validate_version(&older_major).is_err());
    }
}

#[test]
fn test_version_patch_ignored_for_compatibility() {
    let different_patch = ProtocolVersion {
        major: CURRENT_VERSION.major,
        minor: CURRENT_VERSION.minor,
        patch: CURRENT_VERSION.patch + 100,
    };
    assert!(validate_version(&different_patch).is_ok());
}

#[test]
fn test_message_with_incompatible_version() {
    let incompatible_msg = Message {
        version: ProtocolVersion {
            major: 99,
            minor: 0,
            patch: 0,
        },
        payload: MessagePayload::Ping,
    };

    // Should encode successfully
    let bytes = encode_message(&incompatible_msg).expect("Failed to encode");

    // Should decode successfully
    let decoded = decode_message(&bytes).expect("Failed to decode");

    // But validation should fail
    assert!(validate_version(&decoded.version).is_err());
}

// ============================================================================
// Framing Tests
// ============================================================================

#[test]
fn test_framed_write_read_via_cursor() {
    let msg = create_message(MessagePayload::Ping);

    let mut buffer = Vec::new();
    protocol::write_framed(&mut buffer, &msg).expect("Failed to write framed");

    let mut cursor = Cursor::new(buffer);
    let decoded = protocol::read_framed(&mut cursor).expect("Failed to read framed");

    assert_eq!(decoded.version, CURRENT_VERSION);
    matches!(decoded.payload, MessagePayload::Ping);
}

#[test]
fn test_framed_multiple_messages_in_stream() {
    let messages = [
        create_message(MessagePayload::Ping),
        create_message(MessagePayload::Pong),
        create_message(MessagePayload::ListDevicesRequest),
        create_message(MessagePayload::Error {
            message: "Test error".to_string(),
        }),
    ];

    let mut buffer = Vec::new();
    for msg in &messages {
        protocol::write_framed(&mut buffer, msg).expect("Failed to write");
    }

    let mut cursor = Cursor::new(buffer);
    for _ in 0..messages.len() {
        let decoded = protocol::read_framed(&mut cursor).expect("Failed to read");
        assert_eq!(decoded.version, CURRENT_VERSION);
    }
}

// ============================================================================
// Edge Cases and Error Conditions
// ============================================================================

#[test]
fn test_ping_pong_roundtrip() {
    let ping = create_message(MessagePayload::Ping);
    verify_roundtrip(&ping);
    verify_framed_roundtrip(&ping);

    let pong = create_message(MessagePayload::Pong);
    verify_roundtrip(&pong);
    verify_framed_roundtrip(&pong);
}

#[test]
fn test_error_message_roundtrip() {
    let messages = [
        "Simple error",
        "Error with unicode: \u{1F600} \u{1F4BB}",
        "Error with newlines:\nLine 2\nLine 3",
        &"A".repeat(1000), // Long error message
        "",                // Empty error message
    ];

    for error_msg in messages {
        let msg = create_message(MessagePayload::Error {
            message: error_msg.to_string(),
        });

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        if let MessagePayload::Error { message } = decoded.payload {
            assert_eq!(message, error_msg);
        } else {
            panic!("Wrong payload type");
        }
    }
}

#[test]
fn test_device_id_boundary_values() {
    for id in [0, 1, u32::MAX / 2, u32::MAX - 1, u32::MAX] {
        let msg = create_message(MessagePayload::AttachDeviceRequest {
            device_id: DeviceId(id),
        });
        verify_roundtrip(&msg);
    }
}

#[test]
fn test_request_id_boundary_values() {
    for id in [0, 1, u64::MAX / 2, u64::MAX - 1, u64::MAX] {
        let request = UsbRequest {
            id: RequestId(id),
            handle: DeviceHandle(1),
            transfer: TransferType::Control {
                request_type: 0x80,
                request: 0x06,
                value: 0x0100,
                index: 0,
                data: vec![],
            },
        };

        let msg = create_message(MessagePayload::SubmitTransfer { request });
        verify_roundtrip(&msg);
    }
}

#[test]
fn test_empty_bulk_transfer() {
    let request = UsbRequest {
        id: RequestId(1),
        handle: DeviceHandle(1),
        transfer: TransferType::Bulk {
            endpoint: 0x01,
            data: vec![],
            timeout_ms: 1000,
        },
    };

    let msg = create_message(MessagePayload::SubmitTransfer { request });
    verify_roundtrip(&msg);
}

#[test]
fn test_control_transfer_with_data() {
    // Control OUT transfer with 8 bytes of data
    let request = UsbRequest {
        id: RequestId(1),
        handle: DeviceHandle(1),
        transfer: TransferType::Control {
            request_type: 0x21, // Class, Interface, Host-to-device
            request: 0x09,      // SET_REPORT
            value: 0x0200,      // Report type and ID
            index: 0x0000,
            data: vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08],
        },
    };

    let msg = create_message(MessagePayload::SubmitTransfer { request });

    let bytes = encode_message(&msg).expect("Failed to encode");
    let decoded = decode_message(&bytes).expect("Failed to decode");

    if let MessagePayload::SubmitTransfer { request } = decoded.payload {
        if let TransferType::Control { data, .. } = request.transfer {
            assert_eq!(data.len(), 8);
            assert_eq!(data, vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08]);
        } else {
            panic!("Wrong transfer type");
        }
    } else {
        panic!("Wrong payload type");
    }
}

// ============================================================================
// Async Framing Tests (requires tokio feature)
// ============================================================================

// ============================================================================
// Malformed Message Error Handling Tests
// ============================================================================

#[test]
fn test_decode_empty_bytes() {
    let empty: &[u8] = &[];
    let result = decode_message(empty);
    assert!(result.is_err());
}

#[test]
fn test_decode_random_garbage() {
    let garbage: [u8; 32] = [
        0xFF, 0xFE, 0xFD, 0xFC, 0xFB, 0xFA, 0xF9, 0xF8, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06,
        0x07, 0xAB, 0xCD, 0xEF, 0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0, 0x11, 0x22, 0x33,
        0x44, 0x55,
    ];
    let result = decode_message(&garbage);
    assert!(result.is_err());
}

#[test]
fn test_decode_truncated_message() {
    // First encode a valid message
    let msg = create_message(MessagePayload::ListDevicesResponse {
        devices: vec![create_test_device(1, 0x1234, 0x5678)],
    });
    let bytes = encode_message(&msg).expect("Failed to encode");

    // Truncate the message at various points
    for truncate_at in [1, 2, 5, 10, bytes.len() / 2, bytes.len() - 1] {
        if truncate_at < bytes.len() {
            let truncated = &bytes[..truncate_at];
            let result = decode_message(truncated);
            assert!(
                result.is_err(),
                "Expected error when truncated at {} bytes",
                truncate_at
            );
        }
    }
}

#[test]
fn test_decode_corrupted_middle() {
    // Encode a valid message
    let msg = create_message(MessagePayload::SubmitTransfer {
        request: UsbRequest {
            id: RequestId(12345),
            handle: DeviceHandle(1),
            transfer: TransferType::Bulk {
                endpoint: 0x81,
                data: vec![0; 64],
                timeout_ms: 5000,
            },
        },
    });
    let mut bytes = encode_message(&msg).expect("Failed to encode");

    // Corrupt some bytes in the middle
    if bytes.len() > 10 {
        let mid = bytes.len() / 2;
        bytes[mid] = 0xFF;
        bytes[mid + 1] = 0xFE;
        bytes[mid + 2] = 0xFD;

        let result = decode_message(&bytes);
        // May succeed or fail depending on corruption location, but shouldn't panic
        let _ = result;
    }
}

#[test]
fn test_decode_framed_zero_length() {
    // Frame with zero length
    let zero_len = [0u8, 0, 0, 0];
    let result = decode_framed(&zero_len);
    // Should fail because message bytes are empty
    assert!(result.is_err());
}

#[test]
fn test_decode_framed_huge_length_prefix() {
    // Frame claiming to be 1GB
    let huge_len = [0x40u8, 0x00, 0x00, 0x00]; // 1GB in big-endian
    let result = decode_framed(&huge_len);
    assert!(result.is_err());
    // Should be FrameTooLarge error
    assert!(matches!(
        result,
        Err(ProtocolError::FrameTooLarge { .. })
    ));
}

#[test]
fn test_decode_framed_length_mismatch() {
    // Frame says 100 bytes but only has 10
    let mut frame = vec![0u8, 0, 0, 100]; // 100 bytes length
    frame.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]); // Only 10 bytes

    let result = decode_framed(&frame);
    assert!(result.is_err());
    assert!(matches!(result, Err(ProtocolError::IncompleteFrame { .. })));
}

#[test]
fn test_decode_framed_extra_trailing_bytes() {
    // Valid framed message with extra trailing bytes
    let msg = create_message(MessagePayload::Ping);
    let mut framed = encode_framed(&msg).expect("Failed to encode");

    // Add extra trailing garbage
    framed.extend_from_slice(&[0xFF, 0xFE, 0xFD, 0xFC]);

    // decode_framed should still succeed - it reads exact length
    let result = decode_framed(&framed);
    assert!(result.is_ok());
}

#[test]
fn test_decode_invalid_utf8_in_string() {
    // Try to decode bytes that would contain invalid UTF-8 in string fields
    // This tests postcard's handling of string deserialization
    let invalid_utf8: [u8; 20] = [
        0x01, 0x00, 0x00, // Version bytes
        0x0A, // Some payload discriminant
        0xFF, 0xFE, 0x80, 0x81, // Invalid UTF-8 bytes
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];

    let result = decode_message(&invalid_utf8);
    // Should fail to deserialize
    assert!(result.is_err());
}

#[test]
fn test_read_framed_from_empty_reader() {
    let empty: &[u8] = &[];
    let mut cursor = Cursor::new(empty);
    let result = read_framed(&mut cursor);
    assert!(result.is_err());
}

#[test]
fn test_read_framed_partial_length() {
    // Only 2 bytes of 4-byte length prefix
    let partial: &[u8] = &[0x00, 0x10];
    let mut cursor = Cursor::new(partial);
    let result = read_framed(&mut cursor);
    assert!(result.is_err());
}

// ============================================================================
// Protocol Message Mock Stream Tests
// ============================================================================

#[test]
fn test_mock_stream_multiple_messages() {
    // Simulate a stream with multiple messages
    let messages = [
        create_message(MessagePayload::Ping),
        create_message(MessagePayload::ListDevicesRequest),
        create_message(MessagePayload::ListDevicesResponse {
            devices: vec![create_test_device(1, 0x1234, 0x5678)],
        }),
        create_message(MessagePayload::AttachDeviceRequest {
            device_id: DeviceId(1),
        }),
        create_message(MessagePayload::AttachDeviceResponse {
            result: Ok(DeviceHandle(100)),
        }),
        create_message(MessagePayload::Pong),
    ];

    // Write all messages to buffer (simulating send over stream)
    let mut buffer = Vec::new();
    for msg in &messages {
        write_framed(&mut buffer, msg).expect("Failed to write");
    }

    // Read them all back (simulating receive from stream)
    let mut cursor = Cursor::new(buffer);
    let mut decoded_count = 0;

    for original in &messages {
        let decoded = read_framed(&mut cursor).expect("Failed to read");
        assert_eq!(decoded.version, original.version);
        decoded_count += 1;
    }

    assert_eq!(decoded_count, messages.len());
}

#[test]
fn test_mock_request_response_flow() {
    // Simulate client -> server -> client message flow

    // Client sends request
    let request = create_message(MessagePayload::AttachDeviceRequest {
        device_id: DeviceId(42),
    });
    let request_bytes = encode_framed(&request).expect("Failed to encode request");

    // "Server" receives and decodes request
    let server_received = decode_framed(&request_bytes).expect("Failed to decode");
    let device_id = if let MessagePayload::AttachDeviceRequest { device_id } = server_received.payload
    {
        device_id
    } else {
        panic!("Expected AttachDeviceRequest");
    };
    assert_eq!(device_id.0, 42);

    // "Server" sends response
    let response = create_message(MessagePayload::AttachDeviceResponse {
        result: Ok(DeviceHandle(100)),
    });
    let response_bytes = encode_framed(&response).expect("Failed to encode response");

    // Client receives and decodes response
    let client_received = decode_framed(&response_bytes).expect("Failed to decode");
    if let MessagePayload::AttachDeviceResponse { result } = client_received.payload {
        assert!(result.is_ok());
        assert_eq!(result.unwrap().0, 100);
    } else {
        panic!("Expected AttachDeviceResponse");
    }
}

#[test]
fn test_mock_usb_transfer_flow() {
    // Simulate complete USB transfer request/response flow

    // Client submits transfer
    let transfer_request = create_message(MessagePayload::SubmitTransfer {
        request: UsbRequest {
            id: RequestId(99999),
            handle: DeviceHandle(1),
            transfer: TransferType::Control {
                request_type: 0x80,
                request: 0x06, // GET_DESCRIPTOR
                value: 0x0100, // Device descriptor
                index: 0,
                data: vec![],
            },
        },
    });

    let request_bytes = encode_framed(&transfer_request).expect("Failed to encode");
    let server_request = decode_framed(&request_bytes).expect("Failed to decode");

    // Verify request details
    if let MessagePayload::SubmitTransfer { request } = server_request.payload {
        assert_eq!(request.id.0, 99999);
        assert_eq!(request.handle.0, 1);
    } else {
        panic!("Expected SubmitTransfer");
    }

    // Server sends transfer complete response
    let transfer_response = create_message(MessagePayload::TransferComplete {
        response: UsbResponse {
            id: RequestId(99999),
            result: TransferResult::Success {
                data: vec![
                    0x12, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 0x40, // Device descriptor
                    0x34, 0x12, 0x78, 0x56, 0x00, 0x01, 0x01, 0x02, 0x03, 0x01,
                ],
            },
        },
    });

    let response_bytes = encode_framed(&transfer_response).expect("Failed to encode");
    let client_response = decode_framed(&response_bytes).expect("Failed to decode");

    // Verify response
    if let MessagePayload::TransferComplete { response } = client_response.payload {
        assert_eq!(response.id.0, 99999);
        if let TransferResult::Success { data } = response.result {
            assert_eq!(data.len(), 18);
            assert_eq!(data[0], 0x12); // Device descriptor length
            assert_eq!(data[1], 0x01); // Descriptor type
        } else {
            panic!("Expected success result");
        }
    } else {
        panic!("Expected TransferComplete");
    }
}

#[cfg(feature = "async")]
mod async_tests {
    use super::*;

    #[tokio::test]
    async fn test_async_framed_write_read() {
        let msg = create_message(MessagePayload::ListDevicesRequest);
        let framed = encode_framed(&msg).expect("Failed to encode");

        // Write to async buffer
        let mut buffer = Vec::new();
        protocol::write_framed_async(&mut buffer, &framed)
            .await
            .expect("Failed to async write");

        // Read from async buffer
        let mut cursor = std::io::Cursor::new(buffer);
        let read_bytes = protocol::read_framed_async(&mut cursor)
            .await
            .expect("Failed to async read");

        let decoded = decode_framed(&read_bytes).expect("Failed to decode");
        assert_eq!(decoded.version, CURRENT_VERSION);
    }
}
