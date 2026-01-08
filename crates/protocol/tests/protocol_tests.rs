//! Integration tests for protocol message serialization/deserialization
//!
//! Tests all message types defined in crates/protocol/src/messages.rs,
//! verifying codec round-trips and version compatibility.

use protocol::{
    AggregatedNotification, AttachError, ClientMetrics, DetachError, DeviceHandle, DeviceId,
    DeviceInfo, DeviceMetrics, DeviceRemovalReason, DeviceSharingStatus, DeviceSpeed,
    DeviceStatusChangeReason, ForceDetachReason, IsoPacketDescriptor, LockResult, Message,
    MessagePayload, ProtocolLatencyStats, ProtocolMetrics, ProtocolVersion, QueuePositionUpdate,
    RequestId, ServerMetricsSummary, SharingMode, TransferResult, TransferType, UnlockResult,
    UsbError, UsbRequest, UsbResponse, CURRENT_VERSION,
};
use protocol::{decode_framed, decode_message, encode_framed, encode_message, validate_version};
use std::io::Cursor;

fn make_test_device_info(id: u32) -> DeviceInfo {
    DeviceInfo {
        id: DeviceId(id),
        vendor_id: 0x1234,
        product_id: 0x5678,
        bus_number: 1,
        device_address: id as u8,
        manufacturer: Some("Test Manufacturer".to_string()),
        product: Some("Test Product".to_string()),
        serial_number: Some(format!("SN{:08}", id)),
        class: 0x08,
        subclass: 0x06,
        protocol: 0x50,
        speed: DeviceSpeed::High,
        num_configurations: 1,
    }
}

mod message_roundtrip {
    use super::*;

    #[test]
    fn test_ping_pong_roundtrip() {
        let messages = vec![MessagePayload::Ping, MessagePayload::Pong];

        for payload in messages {
            let msg = Message {
                version: CURRENT_VERSION,
                payload,
            };

            let bytes = encode_message(&msg).expect("Failed to encode");
            let decoded = decode_message(&bytes).expect("Failed to decode");

            assert_eq!(decoded.version, CURRENT_VERSION);
        }
    }

    #[test]
    fn test_list_devices_request_roundtrip() {
        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::ListDevicesRequest,
        };

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        assert_eq!(decoded.version, CURRENT_VERSION);
        assert!(matches!(decoded.payload, MessagePayload::ListDevicesRequest));
    }

    #[test]
    fn test_list_devices_response_empty() {
        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::ListDevicesResponse {
                devices: Vec::new(),
            },
        };

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        match decoded.payload {
            MessagePayload::ListDevicesResponse { devices } => {
                assert!(devices.is_empty());
            }
            _ => panic!("Expected ListDevicesResponse"),
        }
    }

    #[test]
    fn test_list_devices_response_multiple() {
        let devices: Vec<DeviceInfo> = (0..10).map(make_test_device_info).collect();

        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::ListDevicesResponse {
                devices: devices.clone(),
            },
        };

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        match decoded.payload {
            MessagePayload::ListDevicesResponse {
                devices: decoded_devices,
            } => {
                assert_eq!(decoded_devices.len(), 10);
                for (orig, dec) in devices.iter().zip(decoded_devices.iter()) {
                    assert_eq!(orig.id, dec.id);
                    assert_eq!(orig.vendor_id, dec.vendor_id);
                    assert_eq!(orig.product_id, dec.product_id);
                    assert_eq!(orig.manufacturer, dec.manufacturer);
                    assert_eq!(orig.product, dec.product);
                    assert_eq!(orig.serial_number, dec.serial_number);
                }
            }
            _ => panic!("Expected ListDevicesResponse"),
        }
    }

    #[test]
    fn test_attach_device_request_roundtrip() {
        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::AttachDeviceRequest {
                device_id: DeviceId(42),
            },
        };

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        match decoded.payload {
            MessagePayload::AttachDeviceRequest { device_id } => {
                assert_eq!(device_id, DeviceId(42));
            }
            _ => panic!("Expected AttachDeviceRequest"),
        }
    }

    #[test]
    fn test_attach_device_response_success() {
        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::AttachDeviceResponse {
                result: Ok(DeviceHandle(123)),
            },
        };

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        match decoded.payload {
            MessagePayload::AttachDeviceResponse { result } => {
                assert_eq!(result, Ok(DeviceHandle(123)));
            }
            _ => panic!("Expected AttachDeviceResponse"),
        }
    }

    #[test]
    fn test_attach_device_response_errors() {
        let error_variants = vec![
            AttachError::DeviceNotFound,
            AttachError::AlreadyAttached,
            AttachError::PermissionDenied,
            AttachError::PolicyDenied {
                reason: "Test policy denial".to_string(),
            },
            AttachError::OutsideTimeWindow {
                current_time: "14:30".to_string(),
                allowed_windows: vec!["09:00-12:00".to_string(), "13:00-17:00".to_string()],
            },
            AttachError::DeviceClassRestricted { device_class: 8 },
            AttachError::Other {
                message: "Custom error".to_string(),
            },
        ];

        for error in error_variants {
            let msg = Message {
                version: CURRENT_VERSION,
                payload: MessagePayload::AttachDeviceResponse {
                    result: Err(error.clone()),
                },
            };

            let bytes = encode_message(&msg).expect("Failed to encode");
            let decoded = decode_message(&bytes).expect("Failed to decode");

            match decoded.payload {
                MessagePayload::AttachDeviceResponse { result } => {
                    assert_eq!(result, Err(error));
                }
                _ => panic!("Expected AttachDeviceResponse"),
            }
        }
    }

    #[test]
    fn test_detach_device_request_roundtrip() {
        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::DetachDeviceRequest {
                handle: DeviceHandle(99),
            },
        };

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        match decoded.payload {
            MessagePayload::DetachDeviceRequest { handle } => {
                assert_eq!(handle, DeviceHandle(99));
            }
            _ => panic!("Expected DetachDeviceRequest"),
        }
    }

    #[test]
    fn test_detach_device_response_variants() {
        let results = vec![
            Ok(()),
            Err(DetachError::HandleNotFound),
            Err(DetachError::Other {
                message: "Test error".to_string(),
            }),
        ];

        for result in results {
            let msg = Message {
                version: CURRENT_VERSION,
                payload: MessagePayload::DetachDeviceResponse {
                    result: result.clone(),
                },
            };

            let bytes = encode_message(&msg).expect("Failed to encode");
            let decoded = decode_message(&bytes).expect("Failed to decode");

            match decoded.payload {
                MessagePayload::DetachDeviceResponse {
                    result: decoded_result,
                } => {
                    assert_eq!(decoded_result, result);
                }
                _ => panic!("Expected DetachDeviceResponse"),
            }
        }
    }

    #[test]
    fn test_error_message_roundtrip() {
        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::Error {
                message: "Something went wrong with special chars: <>&\"'".to_string(),
            },
        };

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        match decoded.payload {
            MessagePayload::Error { message } => {
                assert_eq!(message, "Something went wrong with special chars: <>&\"'");
            }
            _ => panic!("Expected Error"),
        }
    }

    #[test]
    fn test_heartbeat_roundtrip() {
        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::Heartbeat {
                sequence: 12345,
                timestamp_ms: 1704067200000,
            },
        };

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        match decoded.payload {
            MessagePayload::Heartbeat {
                sequence,
                timestamp_ms,
            } => {
                assert_eq!(sequence, 12345);
                assert_eq!(timestamp_ms, 1704067200000);
            }
            _ => panic!("Expected Heartbeat"),
        }
    }

    #[test]
    fn test_heartbeat_ack_roundtrip() {
        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::HeartbeatAck {
                sequence: 12345,
                client_timestamp_ms: 1704067200000,
                server_timestamp_ms: 1704067200050,
            },
        };

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        match decoded.payload {
            MessagePayload::HeartbeatAck {
                sequence,
                client_timestamp_ms,
                server_timestamp_ms,
            } => {
                assert_eq!(sequence, 12345);
                assert_eq!(client_timestamp_ms, 1704067200000);
                assert_eq!(server_timestamp_ms, 1704067200050);
            }
            _ => panic!("Expected HeartbeatAck"),
        }
    }

    #[test]
    fn test_client_capabilities_roundtrip() {
        for supports in [true, false] {
            let msg = Message {
                version: CURRENT_VERSION,
                payload: MessagePayload::ClientCapabilities {
                    supports_push_notifications: supports,
                },
            };

            let bytes = encode_message(&msg).expect("Failed to encode");
            let decoded = decode_message(&bytes).expect("Failed to decode");

            match decoded.payload {
                MessagePayload::ClientCapabilities {
                    supports_push_notifications,
                } => {
                    assert_eq!(supports_push_notifications, supports);
                }
                _ => panic!("Expected ClientCapabilities"),
            }
        }
    }

    #[test]
    fn test_server_capabilities_roundtrip() {
        for will_send in [true, false] {
            let msg = Message {
                version: CURRENT_VERSION,
                payload: MessagePayload::ServerCapabilities {
                    will_send_notifications: will_send,
                },
            };

            let bytes = encode_message(&msg).expect("Failed to encode");
            let decoded = decode_message(&bytes).expect("Failed to decode");

            match decoded.payload {
                MessagePayload::ServerCapabilities {
                    will_send_notifications,
                } => {
                    assert_eq!(will_send_notifications, will_send);
                }
                _ => panic!("Expected ServerCapabilities"),
            }
        }
    }
}

mod transfer_messages {
    use super::*;

    #[test]
    fn test_control_transfer_roundtrip() {
        let request = UsbRequest {
            id: RequestId(100),
            handle: DeviceHandle(1),
            transfer: TransferType::Control {
                request_type: 0x80,
                request: 0x06,
                value: 0x0100,
                index: 0,
                data: vec![0; 18],
            },
        };

        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::SubmitTransfer { request },
        };

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        match decoded.payload {
            MessagePayload::SubmitTransfer { request } => {
                assert_eq!(request.id, RequestId(100));
                assert_eq!(request.handle, DeviceHandle(1));
                match request.transfer {
                    TransferType::Control {
                        request_type,
                        request,
                        value,
                        index,
                        data,
                    } => {
                        assert_eq!(request_type, 0x80);
                        assert_eq!(request, 0x06);
                        assert_eq!(value, 0x0100);
                        assert_eq!(index, 0);
                        assert_eq!(data.len(), 18);
                    }
                    _ => panic!("Expected Control transfer"),
                }
            }
            _ => panic!("Expected SubmitTransfer"),
        }
    }

    #[test]
    fn test_bulk_transfer_roundtrip() {
        let data = vec![0xAB; 4096];
        let request = UsbRequest {
            id: RequestId(200),
            handle: DeviceHandle(2),
            transfer: TransferType::Bulk {
                endpoint: 0x81,
                data: data.clone(),
                timeout_ms: 5000,
            },
        };

        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::SubmitTransfer { request },
        };

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        match decoded.payload {
            MessagePayload::SubmitTransfer { request } => match request.transfer {
                TransferType::Bulk {
                    endpoint,
                    data: decoded_data,
                    timeout_ms,
                } => {
                    assert_eq!(endpoint, 0x81);
                    assert_eq!(decoded_data.len(), 4096);
                    assert_eq!(decoded_data, data);
                    assert_eq!(timeout_ms, 5000);
                }
                _ => panic!("Expected Bulk transfer"),
            },
            _ => panic!("Expected SubmitTransfer"),
        }
    }

    #[test]
    fn test_interrupt_transfer_roundtrip() {
        let request = UsbRequest {
            id: RequestId(300),
            handle: DeviceHandle(3),
            transfer: TransferType::Interrupt {
                endpoint: 0x82,
                data: vec![0x01, 0x02, 0x03, 0x04],
                timeout_ms: 100,
            },
        };

        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::SubmitTransfer { request },
        };

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        match decoded.payload {
            MessagePayload::SubmitTransfer { request } => match request.transfer {
                TransferType::Interrupt {
                    endpoint,
                    data,
                    timeout_ms,
                } => {
                    assert_eq!(endpoint, 0x82);
                    assert_eq!(data, vec![0x01, 0x02, 0x03, 0x04]);
                    assert_eq!(timeout_ms, 100);
                }
                _ => panic!("Expected Interrupt transfer"),
            },
            _ => panic!("Expected SubmitTransfer"),
        }
    }

    #[test]
    fn test_isochronous_transfer_roundtrip() {
        let iso_descriptors = vec![
            IsoPacketDescriptor {
                offset: 0,
                length: 192,
                actual_length: 0,
                status: 0,
            },
            IsoPacketDescriptor {
                offset: 192,
                length: 192,
                actual_length: 0,
                status: 0,
            },
        ];

        let request = UsbRequest {
            id: RequestId(400),
            handle: DeviceHandle(4),
            transfer: TransferType::Isochronous {
                endpoint: 0x83,
                data: vec![0; 384],
                iso_packet_descriptors: iso_descriptors.clone(),
                start_frame: 1000,
                interval: 1,
                timeout_ms: 1000,
            },
        };

        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::SubmitTransfer { request },
        };

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        match decoded.payload {
            MessagePayload::SubmitTransfer { request } => match request.transfer {
                TransferType::Isochronous {
                    endpoint,
                    data,
                    iso_packet_descriptors,
                    start_frame,
                    interval,
                    timeout_ms,
                } => {
                    assert_eq!(endpoint, 0x83);
                    assert_eq!(data.len(), 384);
                    assert_eq!(iso_packet_descriptors.len(), 2);
                    assert_eq!(iso_packet_descriptors[0].offset, 0);
                    assert_eq!(iso_packet_descriptors[1].offset, 192);
                    assert_eq!(start_frame, 1000);
                    assert_eq!(interval, 1);
                    assert_eq!(timeout_ms, 1000);
                }
                _ => panic!("Expected Isochronous transfer"),
            },
            _ => panic!("Expected SubmitTransfer"),
        }
    }

    #[test]
    fn test_transfer_complete_success() {
        let response = UsbResponse {
            id: RequestId(500),
            result: TransferResult::Success {
                data: vec![0x12, 0x01, 0x00, 0x02],
            },
        };

        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::TransferComplete { response },
        };

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        match decoded.payload {
            MessagePayload::TransferComplete { response } => {
                assert_eq!(response.id, RequestId(500));
                match response.result {
                    TransferResult::Success { data } => {
                        assert_eq!(data, vec![0x12, 0x01, 0x00, 0x02]);
                    }
                    _ => panic!("Expected Success result"),
                }
            }
            _ => panic!("Expected TransferComplete"),
        }
    }

    #[test]
    fn test_transfer_complete_errors() {
        let errors = vec![
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
                message: "Custom USB error".to_string(),
            },
        ];

        for error in errors {
            let response = UsbResponse {
                id: RequestId(600),
                result: TransferResult::Error {
                    error: error.clone(),
                },
            };

            let msg = Message {
                version: CURRENT_VERSION,
                payload: MessagePayload::TransferComplete { response },
            };

            let bytes = encode_message(&msg).expect("Failed to encode");
            let decoded = decode_message(&bytes).expect("Failed to decode");

            match decoded.payload {
                MessagePayload::TransferComplete { response } => match response.result {
                    TransferResult::Error {
                        error: decoded_error,
                    } => {
                        assert_eq!(decoded_error, error);
                    }
                    _ => panic!("Expected Error result"),
                },
                _ => panic!("Expected TransferComplete"),
            }
        }
    }

    #[test]
    fn test_transfer_complete_isochronous_success() {
        let iso_descriptors = vec![
            IsoPacketDescriptor {
                offset: 0,
                length: 192,
                actual_length: 192,
                status: 0,
            },
            IsoPacketDescriptor {
                offset: 192,
                length: 192,
                actual_length: 180,
                status: 0,
            },
        ];

        let response = UsbResponse {
            id: RequestId(700),
            result: TransferResult::IsochronousSuccess {
                iso_packet_descriptors: iso_descriptors,
                start_frame: 1000,
                error_count: 0,
                data: vec![0xAA; 372],
            },
        };

        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::TransferComplete { response },
        };

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        match decoded.payload {
            MessagePayload::TransferComplete { response } => match response.result {
                TransferResult::IsochronousSuccess {
                    iso_packet_descriptors,
                    start_frame,
                    error_count,
                    data,
                } => {
                    assert_eq!(iso_packet_descriptors.len(), 2);
                    assert_eq!(iso_packet_descriptors[1].actual_length, 180);
                    assert_eq!(start_frame, 1000);
                    assert_eq!(error_count, 0);
                    assert_eq!(data.len(), 372);
                }
                _ => panic!("Expected IsochronousSuccess result"),
            },
            _ => panic!("Expected TransferComplete"),
        }
    }
}

mod notification_messages {
    use super::*;

    #[test]
    fn test_device_arrived_notification() {
        let device = make_test_device_info(99);

        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::DeviceArrivedNotification {
                device: device.clone(),
            },
        };

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        match decoded.payload {
            MessagePayload::DeviceArrivedNotification {
                device: decoded_device,
            } => {
                assert_eq!(decoded_device.id, device.id);
                assert_eq!(decoded_device.vendor_id, device.vendor_id);
            }
            _ => panic!("Expected DeviceArrivedNotification"),
        }
    }

    #[test]
    fn test_device_removed_notification_variants() {
        let reasons = vec![
            DeviceRemovalReason::Unplugged,
            DeviceRemovalReason::ServerShutdown,
            DeviceRemovalReason::AdminAction,
            DeviceRemovalReason::DeviceError {
                message: "Reset failed".to_string(),
            },
        ];

        for reason in reasons {
            let msg = Message {
                version: CURRENT_VERSION,
                payload: MessagePayload::DeviceRemovedNotification {
                    device_id: DeviceId(42),
                    invalidated_handles: vec![DeviceHandle(1), DeviceHandle(2)],
                    reason: reason.clone(),
                },
            };

            let bytes = encode_message(&msg).expect("Failed to encode");
            let decoded = decode_message(&bytes).expect("Failed to decode");

            match decoded.payload {
                MessagePayload::DeviceRemovedNotification {
                    device_id,
                    invalidated_handles,
                    reason: decoded_reason,
                } => {
                    assert_eq!(device_id, DeviceId(42));
                    assert_eq!(invalidated_handles.len(), 2);
                    assert_eq!(decoded_reason, reason);
                }
                _ => panic!("Expected DeviceRemovedNotification"),
            }
        }
    }

    #[test]
    fn test_force_detach_warning() {
        let reasons = vec![
            ForceDetachReason::SessionDurationLimitReached {
                duration_secs: 3600,
                max_duration_secs: 3600,
            },
            ForceDetachReason::TimeWindowExpired {
                current_time: "17:00".to_string(),
                next_window: Some("09:00-17:00".to_string()),
            },
            ForceDetachReason::AdminAction {
                reason: Some("Maintenance".to_string()),
            },
            ForceDetachReason::AdminAction { reason: None },
            ForceDetachReason::ServerShutdown,
            ForceDetachReason::DeviceDisconnected,
        ];

        for reason in reasons {
            let msg = Message {
                version: CURRENT_VERSION,
                payload: MessagePayload::ForceDetachWarning {
                    handle: DeviceHandle(10),
                    device_id: DeviceId(5),
                    reason: reason.clone(),
                    seconds_until_detach: 30,
                },
            };

            let bytes = encode_message(&msg).expect("Failed to encode");
            let decoded = decode_message(&bytes).expect("Failed to decode");

            match decoded.payload {
                MessagePayload::ForceDetachWarning {
                    handle,
                    device_id,
                    reason: decoded_reason,
                    seconds_until_detach,
                } => {
                    assert_eq!(handle, DeviceHandle(10));
                    assert_eq!(device_id, DeviceId(5));
                    assert_eq!(decoded_reason, reason);
                    assert_eq!(seconds_until_detach, 30);
                }
                _ => panic!("Expected ForceDetachWarning"),
            }
        }
    }

    #[test]
    fn test_forced_detach_notification() {
        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::ForcedDetachNotification {
                handle: DeviceHandle(10),
                device_id: DeviceId(5),
                reason: ForceDetachReason::ServerShutdown,
            },
        };

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        match decoded.payload {
            MessagePayload::ForcedDetachNotification {
                handle,
                device_id,
                reason,
            } => {
                assert_eq!(handle, DeviceHandle(10));
                assert_eq!(device_id, DeviceId(5));
                assert_eq!(reason, ForceDetachReason::ServerShutdown);
            }
            _ => panic!("Expected ForcedDetachNotification"),
        }
    }

    #[test]
    fn test_device_status_changed_notification() {
        let reasons = vec![
            DeviceStatusChangeReason::DeviceReset,
            DeviceStatusChangeReason::SharingStatusChanged { shared: true },
            DeviceStatusChangeReason::ConfigurationChanged,
            DeviceStatusChangeReason::CapabilitiesUpdated,
            DeviceStatusChangeReason::InterfaceChange,
            DeviceStatusChangeReason::PowerStateChange,
            DeviceStatusChangeReason::Other {
                description: "Custom status change".to_string(),
            },
        ];

        for reason in reasons {
            let msg = Message {
                version: CURRENT_VERSION,
                payload: MessagePayload::DeviceStatusChangedNotification {
                    device_id: DeviceId(7),
                    device_info: Some(make_test_device_info(7)),
                    reason: reason.clone(),
                },
            };

            let bytes = encode_message(&msg).expect("Failed to encode");
            let decoded = decode_message(&bytes).expect("Failed to decode");

            match decoded.payload {
                MessagePayload::DeviceStatusChangedNotification {
                    device_id,
                    device_info,
                    reason: decoded_reason,
                } => {
                    assert_eq!(device_id, DeviceId(7));
                    assert!(device_info.is_some());
                    assert_eq!(decoded_reason, reason);
                }
                _ => panic!("Expected DeviceStatusChangedNotification"),
            }
        }
    }

    #[test]
    fn test_aggregated_notifications() {
        let notifications = vec![
            AggregatedNotification::Arrived(make_test_device_info(1)),
            AggregatedNotification::Removed {
                device_id: DeviceId(2),
                invalidated_handles: vec![DeviceHandle(10)],
                reason: DeviceRemovalReason::Unplugged,
            },
            AggregatedNotification::StatusChanged {
                device_id: DeviceId(3),
                device_info: None,
                reason: DeviceStatusChangeReason::DeviceReset,
            },
        ];

        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::AggregatedNotifications {
                notifications: notifications.clone(),
            },
        };

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        match decoded.payload {
            MessagePayload::AggregatedNotifications {
                notifications: decoded_notifications,
            } => {
                assert_eq!(decoded_notifications.len(), 3);
            }
            _ => panic!("Expected AggregatedNotifications"),
        }
    }
}

mod sharing_messages {
    use super::*;

    #[test]
    fn test_get_sharing_status_request() {
        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::GetSharingStatusRequest {
                device_id: DeviceId(15),
            },
        };

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        match decoded.payload {
            MessagePayload::GetSharingStatusRequest { device_id } => {
                assert_eq!(device_id, DeviceId(15));
            }
            _ => panic!("Expected GetSharingStatusRequest"),
        }
    }

    #[test]
    fn test_get_sharing_status_response_success() {
        let status = DeviceSharingStatus {
            device_id: DeviceId(15),
            sharing_mode: SharingMode::Shared,
            attached_clients: 3,
            has_write_lock: false,
            queue_position: 2,
            queue_length: 5,
        };

        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::GetSharingStatusResponse {
                result: Ok(status.clone()),
            },
        };

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        match decoded.payload {
            MessagePayload::GetSharingStatusResponse { result } => {
                let decoded_status = result.expect("Expected Ok result");
                assert_eq!(decoded_status.device_id, DeviceId(15));
                assert_eq!(decoded_status.sharing_mode, SharingMode::Shared);
                assert_eq!(decoded_status.attached_clients, 3);
                assert_eq!(decoded_status.queue_position, 2);
                assert_eq!(decoded_status.queue_length, 5);
            }
            _ => panic!("Expected GetSharingStatusResponse"),
        }
    }

    #[test]
    fn test_lock_device_request() {
        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::LockDeviceRequest {
                handle: DeviceHandle(20),
                write_access: true,
                timeout_secs: 60,
            },
        };

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        match decoded.payload {
            MessagePayload::LockDeviceRequest {
                handle,
                write_access,
                timeout_secs,
            } => {
                assert_eq!(handle, DeviceHandle(20));
                assert!(write_access);
                assert_eq!(timeout_secs, 60);
            }
            _ => panic!("Expected LockDeviceRequest"),
        }
    }

    #[test]
    fn test_lock_device_response_variants() {
        let results = vec![
            LockResult::Acquired,
            LockResult::AlreadyHeld,
            LockResult::Queued { position: 3 },
            LockResult::NotAvailable {
                reason: "Device busy".to_string(),
            },
        ];

        for result in results {
            let msg = Message {
                version: CURRENT_VERSION,
                payload: MessagePayload::LockDeviceResponse {
                    result: result.clone(),
                },
            };

            let bytes = encode_message(&msg).expect("Failed to encode");
            let decoded = decode_message(&bytes).expect("Failed to decode");

            match decoded.payload {
                MessagePayload::LockDeviceResponse {
                    result: decoded_result,
                } => {
                    assert_eq!(decoded_result, result);
                }
                _ => panic!("Expected LockDeviceResponse"),
            }
        }
    }

    #[test]
    fn test_unlock_device_request() {
        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::UnlockDeviceRequest {
                handle: DeviceHandle(25),
            },
        };

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        match decoded.payload {
            MessagePayload::UnlockDeviceRequest { handle } => {
                assert_eq!(handle, DeviceHandle(25));
            }
            _ => panic!("Expected UnlockDeviceRequest"),
        }
    }

    #[test]
    fn test_unlock_device_response_variants() {
        let results = vec![
            UnlockResult::Released,
            UnlockResult::NotHeld,
            UnlockResult::Error {
                message: "Cannot release in exclusive mode".to_string(),
            },
        ];

        for result in results {
            let msg = Message {
                version: CURRENT_VERSION,
                payload: MessagePayload::UnlockDeviceResponse {
                    result: result.clone(),
                },
            };

            let bytes = encode_message(&msg).expect("Failed to encode");
            let decoded = decode_message(&bytes).expect("Failed to decode");

            match decoded.payload {
                MessagePayload::UnlockDeviceResponse {
                    result: decoded_result,
                } => {
                    assert_eq!(decoded_result, result);
                }
                _ => panic!("Expected UnlockDeviceResponse"),
            }
        }
    }

    #[test]
    fn test_queue_position_notification() {
        let update = QueuePositionUpdate {
            device_id: DeviceId(30),
            position: 1,
            queue_length: 3,
        };

        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::QueuePositionNotification {
                update: update.clone(),
            },
        };

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        match decoded.payload {
            MessagePayload::QueuePositionNotification {
                update: decoded_update,
            } => {
                assert_eq!(decoded_update.device_id, DeviceId(30));
                assert_eq!(decoded_update.position, 1);
                assert_eq!(decoded_update.queue_length, 3);
            }
            _ => panic!("Expected QueuePositionNotification"),
        }
    }

    #[test]
    fn test_device_available_notification() {
        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::DeviceAvailableNotification {
                device_id: DeviceId(35),
                handle: DeviceHandle(100),
                sharing_mode: SharingMode::ReadOnly,
            },
        };

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        match decoded.payload {
            MessagePayload::DeviceAvailableNotification {
                device_id,
                handle,
                sharing_mode,
            } => {
                assert_eq!(device_id, DeviceId(35));
                assert_eq!(handle, DeviceHandle(100));
                assert_eq!(sharing_mode, SharingMode::ReadOnly);
            }
            _ => panic!("Expected DeviceAvailableNotification"),
        }
    }
}

mod metrics_messages {
    use super::*;

    #[test]
    fn test_get_metrics_request() {
        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::GetMetricsRequest,
        };

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        assert!(matches!(decoded.payload, MessagePayload::GetMetricsRequest));
    }

    #[test]
    fn test_get_metrics_response() {
        let metrics = ServerMetricsSummary {
            total: ProtocolMetrics {
                bytes_sent: 1000000,
                bytes_received: 500000,
                transfers_completed: 100,
                transfers_failed: 2,
                retries: 5,
                active_transfers: 3,
                latency: ProtocolLatencyStats {
                    min_us: 1000,
                    max_us: 50000,
                    avg_us: 10000,
                    sample_count: 100,
                },
                throughput_tx_bps: 1000000.0,
                throughput_rx_bps: 500000.0,
                loss_rate: 0.02,
                retry_rate: 0.05,
                uptime_secs: Some(3600),
            },
            devices: vec![DeviceMetrics {
                device_id: DeviceId(1),
                metrics: ProtocolMetrics::default(),
            }],
            clients: vec![ClientMetrics {
                client_id: "client1".to_string(),
                metrics: ProtocolMetrics::default(),
            }],
        };

        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::GetMetricsResponse {
                metrics: metrics.clone(),
            },
        };

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        match decoded.payload {
            MessagePayload::GetMetricsResponse {
                metrics: decoded_metrics,
            } => {
                assert_eq!(decoded_metrics.total.bytes_sent, 1000000);
                assert_eq!(decoded_metrics.total.transfers_completed, 100);
                assert_eq!(decoded_metrics.devices.len(), 1);
                assert_eq!(decoded_metrics.clients.len(), 1);
            }
            _ => panic!("Expected GetMetricsResponse"),
        }
    }

    #[test]
    fn test_client_metrics_update() {
        let metrics = ProtocolMetrics {
            bytes_sent: 50000,
            bytes_received: 25000,
            transfers_completed: 10,
            transfers_failed: 0,
            retries: 1,
            active_transfers: 1,
            latency: ProtocolLatencyStats::default(),
            throughput_tx_bps: 100000.0,
            throughput_rx_bps: 50000.0,
            loss_rate: 0.0,
            retry_rate: 0.1,
            uptime_secs: Some(600),
        };

        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::ClientMetricsUpdate {
                metrics: metrics.clone(),
            },
        };

        let bytes = encode_message(&msg).expect("Failed to encode");
        let decoded = decode_message(&bytes).expect("Failed to decode");

        match decoded.payload {
            MessagePayload::ClientMetricsUpdate {
                metrics: decoded_metrics,
            } => {
                assert_eq!(decoded_metrics.bytes_sent, 50000);
                assert_eq!(decoded_metrics.transfers_completed, 10);
            }
            _ => panic!("Expected ClientMetricsUpdate"),
        }
    }
}

mod framed_codec {
    use super::*;
    use protocol::{read_framed, write_framed};

    #[test]
    fn test_framed_encode_decode() {
        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::Ping,
        };

        let framed = encode_framed(&msg).expect("Failed to encode framed");

        assert!(framed.len() >= 4);

        let length = u32::from_be_bytes([framed[0], framed[1], framed[2], framed[3]]) as usize;
        assert_eq!(length, framed.len() - 4);

        let decoded = decode_framed(&framed).expect("Failed to decode framed");
        assert_eq!(decoded.version, CURRENT_VERSION);
    }

    #[test]
    fn test_framed_large_message() {
        let devices: Vec<DeviceInfo> = (0..500).map(make_test_device_info).collect();

        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::ListDevicesResponse { devices },
        };

        let framed = encode_framed(&msg).expect("Failed to encode large message");
        let decoded = decode_framed(&framed).expect("Failed to decode large message");

        match decoded.payload {
            MessagePayload::ListDevicesResponse { devices } => {
                assert_eq!(devices.len(), 500);
            }
            _ => panic!("Expected ListDevicesResponse"),
        }
    }

    #[test]
    fn test_framed_incomplete_length() {
        let incomplete = vec![0, 0, 0];
        let result = decode_framed(&incomplete);
        assert!(result.is_err());
    }

    #[test]
    fn test_framed_incomplete_data() {
        let incomplete = vec![0, 0, 0, 100, 0, 0, 0, 0];
        let result = decode_framed(&incomplete);
        assert!(result.is_err());
    }

    #[test]
    fn test_write_read_framed() {
        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::Pong,
        };

        let mut buffer = Vec::new();
        write_framed(&mut buffer, &msg).expect("Failed to write framed");

        let mut cursor = Cursor::new(buffer);
        let decoded = read_framed(&mut cursor).expect("Failed to read framed");

        assert_eq!(decoded.version, CURRENT_VERSION);
        assert!(matches!(decoded.payload, MessagePayload::Pong));
    }
}

mod version_compatibility {
    use super::*;

    #[test]
    fn test_current_version_compatible() {
        assert!(validate_version(&CURRENT_VERSION).is_ok());
    }

    #[test]
    fn test_same_major_different_minor_compatible() {
        let older_minor = ProtocolVersion {
            major: CURRENT_VERSION.major,
            minor: 0,
            patch: 0,
        };
        assert!(validate_version(&older_minor).is_ok());

        let newer_minor = ProtocolVersion {
            major: CURRENT_VERSION.major,
            minor: CURRENT_VERSION.minor + 10,
            patch: 0,
        };
        assert!(validate_version(&newer_minor).is_ok());
    }

    #[test]
    fn test_different_major_incompatible() {
        let older_major = ProtocolVersion {
            major: 0,
            minor: 99,
            patch: 0,
        };
        assert!(validate_version(&older_major).is_err());

        let newer_major = ProtocolVersion {
            major: CURRENT_VERSION.major + 1,
            minor: 0,
            patch: 0,
        };
        assert!(validate_version(&newer_major).is_err());
    }

    #[test]
    fn test_version_is_compatible_with() {
        let v1_0 = ProtocolVersion {
            major: 1,
            minor: 0,
            patch: 0,
        };
        let v1_1 = ProtocolVersion {
            major: 1,
            minor: 1,
            patch: 0,
        };
        let v2_0 = ProtocolVersion {
            major: 2,
            minor: 0,
            patch: 0,
        };

        assert!(v1_1.is_compatible_with(&v1_0));
        assert!(!v1_0.is_compatible_with(&v1_1));
        assert!(!v2_0.is_compatible_with(&v1_0));
        assert!(!v1_0.is_compatible_with(&v2_0));
    }
}

mod device_speed_variants {
    use super::*;

    #[test]
    fn test_all_device_speeds_roundtrip() {
        let speeds = vec![
            DeviceSpeed::Low,
            DeviceSpeed::Full,
            DeviceSpeed::High,
            DeviceSpeed::Super,
            DeviceSpeed::SuperPlus,
        ];

        for speed in speeds {
            let device = DeviceInfo {
                id: DeviceId(1),
                vendor_id: 0x1234,
                product_id: 0x5678,
                bus_number: 1,
                device_address: 1,
                manufacturer: None,
                product: None,
                serial_number: None,
                class: 0,
                subclass: 0,
                protocol: 0,
                speed: speed.clone(),
                num_configurations: 1,
            };

            let msg = Message {
                version: CURRENT_VERSION,
                payload: MessagePayload::ListDevicesResponse {
                    devices: vec![device],
                },
            };

            let bytes = encode_message(&msg).expect("Failed to encode");
            let decoded = decode_message(&bytes).expect("Failed to decode");

            match decoded.payload {
                MessagePayload::ListDevicesResponse { devices } => {
                    assert_eq!(devices[0].speed, speed);
                }
                _ => panic!("Expected ListDevicesResponse"),
            }
        }
    }

    #[test]
    fn test_device_speed_superspeed_detection() {
        assert!(!DeviceSpeed::Low.is_superspeed());
        assert!(!DeviceSpeed::Full.is_superspeed());
        assert!(!DeviceSpeed::High.is_superspeed());
        assert!(DeviceSpeed::Super.is_superspeed());
        assert!(DeviceSpeed::SuperPlus.is_superspeed());
    }

    #[test]
    fn test_device_speed_bulk_transfer_size() {
        assert_eq!(DeviceSpeed::Low.max_bulk_transfer_size(), 4 * 1024);
        assert_eq!(DeviceSpeed::Full.max_bulk_transfer_size(), 4 * 1024);
        assert_eq!(DeviceSpeed::High.max_bulk_transfer_size(), 64 * 1024);
        assert_eq!(DeviceSpeed::Super.max_bulk_transfer_size(), 1024 * 1024);
        assert_eq!(DeviceSpeed::SuperPlus.max_bulk_transfer_size(), 1024 * 1024);
    }

    #[test]
    fn test_device_speed_optimal_chunk_size() {
        assert_eq!(DeviceSpeed::Low.optimal_chunk_size(), 8);
        assert_eq!(DeviceSpeed::Full.optimal_chunk_size(), 64);
        assert_eq!(DeviceSpeed::High.optimal_chunk_size(), 512);
        assert_eq!(DeviceSpeed::Super.optimal_chunk_size(), 1024);
        assert_eq!(DeviceSpeed::SuperPlus.optimal_chunk_size(), 1024);
    }
}

mod sharing_mode_variants {
    use super::*;

    #[test]
    fn test_all_sharing_modes_roundtrip() {
        let modes = vec![
            SharingMode::Exclusive,
            SharingMode::Shared,
            SharingMode::ReadOnly,
        ];

        for mode in modes {
            let status = DeviceSharingStatus {
                device_id: DeviceId(1),
                sharing_mode: mode,
                attached_clients: 1,
                has_write_lock: false,
                queue_position: 0,
                queue_length: 0,
            };

            let msg = Message {
                version: CURRENT_VERSION,
                payload: MessagePayload::GetSharingStatusResponse { result: Ok(status) },
            };

            let bytes = encode_message(&msg).expect("Failed to encode");
            let decoded = decode_message(&bytes).expect("Failed to decode");

            match decoded.payload {
                MessagePayload::GetSharingStatusResponse { result } => {
                    let decoded_status = result.unwrap();
                    assert_eq!(decoded_status.sharing_mode, mode);
                }
                _ => panic!("Expected GetSharingStatusResponse"),
            }
        }
    }

    #[test]
    fn test_sharing_mode_default() {
        assert_eq!(SharingMode::default(), SharingMode::Exclusive);
    }

    #[test]
    fn test_sharing_mode_display() {
        assert_eq!(format!("{}", SharingMode::Exclusive), "exclusive");
        assert_eq!(format!("{}", SharingMode::Shared), "shared");
        assert_eq!(format!("{}", SharingMode::ReadOnly), "read-only");
    }
}
