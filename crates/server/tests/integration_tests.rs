//! Server Integration Tests
//!
//! Comprehensive tests for the server crate covering:
//! - Configuration loading and validation
//! - USB bridge integration
//! - Protocol message handling
//!
//! Note: These tests replicate config structures for testing since
//! the server crate is a binary-only crate.
//!
//! Run with: `cargo test -p server --test integration_tests`

use common::test_utils::{
    DEFAULT_TEST_TIMEOUT, create_mock_device_info, create_mock_device_list, with_timeout,
};
use common::{UsbCommand, UsbEvent, create_usb_bridge};
use protocol::{
    AttachError, CURRENT_VERSION, DetachError, DeviceHandle, DeviceId, Message, MessagePayload,
    RequestId, TransferResult, TransferType, UsbRequest, UsbResponse,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use tempfile::tempdir;

// ============================================================================
// Config Structures (duplicated for testing since server is binary crate)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ServerConfig {
    server: ServerSettings,
    usb: UsbSettings,
    security: SecuritySettings,
    iroh: IrohSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ServerSettings {
    bind_addr: Option<String>,
    service_mode: bool,
    log_level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UsbSettings {
    auto_share: bool,
    filters: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SecuritySettings {
    approved_clients: Vec<String>,
    require_approval: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IrohSettings {
    relay_servers: Option<Vec<String>>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            server: ServerSettings {
                bind_addr: Some("127.0.0.1:8080".to_string()),
                service_mode: false,
                log_level: "info".to_string(),
            },
            usb: UsbSettings {
                auto_share: false,
                filters: Vec::new(),
            },
            security: SecuritySettings {
                approved_clients: Vec::new(),
                require_approval: true,
            },
            iroh: IrohSettings {
                relay_servers: None,
            },
        }
    }
}

impl ServerConfig {
    fn default_path() -> PathBuf {
        if let Some(config_dir) = dirs::config_dir() {
            config_dir.join("p2p-usb").join("server.toml")
        } else {
            PathBuf::from(".config/p2p-usb/server.toml")
        }
    }
}

// ============================================================================
// Server Configuration Tests
// ============================================================================

#[test]
fn test_server_config_default() {
    let config = ServerConfig::default();

    assert_eq!(config.server.log_level, "info");
    assert!(!config.usb.auto_share);
    assert!(config.security.require_approval);
    assert!(config.security.approved_clients.is_empty());
    assert!(config.usb.filters.is_empty());
}

#[test]
fn test_server_config_serialization_roundtrip() {
    let config = ServerConfig::default();
    let toml_str = toml::to_string(&config).expect("Failed to serialize");
    let parsed: ServerConfig = toml::from_str(&toml_str).expect("Failed to parse");

    assert_eq!(config.server.log_level, parsed.server.log_level);
    assert_eq!(config.usb.auto_share, parsed.usb.auto_share);
    assert_eq!(
        config.security.require_approval,
        parsed.security.require_approval
    );
}

#[test]
fn test_server_config_with_custom_values() {
    let toml_content = r#"
[server]
bind_addr = "0.0.0.0:9090"
service_mode = true
log_level = "debug"

[usb]
auto_share = true
filters = ["0x1234:0x5678", "0xABCD:*"]

[security]
approved_clients = ["abc123def456"]
require_approval = true

[iroh]
relay_servers = ["relay.example.com"]
"#;

    let config: ServerConfig = toml::from_str(toml_content).expect("Failed to parse");

    assert_eq!(config.server.bind_addr, Some("0.0.0.0:9090".to_string()));
    assert!(config.server.service_mode);
    assert_eq!(config.server.log_level, "debug");
    assert!(config.usb.auto_share);
    assert_eq!(config.usb.filters.len(), 2);
    assert_eq!(config.security.approved_clients.len(), 1);
}

#[test]
fn test_server_config_log_levels() {
    let valid_levels = ["trace", "debug", "info", "warn", "error"];

    for level in valid_levels {
        let mut config = ServerConfig::default();
        config.server.log_level = level.to_string();
        assert_eq!(config.server.log_level, level);
    }
}

#[test]
fn test_server_config_save_and_load() {
    let dir = tempdir().expect("Failed to create temp dir");
    let config_path = dir.path().join("server.toml");

    let mut config = ServerConfig::default();
    config.server.log_level = "debug".to_string();
    config.usb.auto_share = true;

    // Save config
    let toml_str = toml::to_string(&config).expect("Failed to serialize");
    std::fs::write(&config_path, toml_str).expect("Failed to write");

    // Verify file exists
    assert!(config_path.exists());

    // Load config
    let content = std::fs::read_to_string(&config_path).expect("Failed to read");
    let loaded: ServerConfig = toml::from_str(&content).expect("Failed to parse");

    assert_eq!(loaded.server.log_level, "debug");
    assert!(loaded.usb.auto_share);
}

#[test]
fn test_server_config_default_path() {
    let path = ServerConfig::default_path();

    // Should contain "p2p-usb" and "server.toml"
    let path_str = path.to_string_lossy();
    assert!(path_str.contains("p2p-usb"));
    assert!(path_str.contains("server.toml"));
}

#[test]
fn test_server_config_filter_patterns() {
    // Valid filter patterns
    let valid_filters = [
        "0x1234:0x5678",
        "0x1234:*",
        "*:0x5678",
        "*:*",
        "0xABCD:0xEF01",
    ];

    for filter in valid_filters {
        let mut config = ServerConfig::default();
        config.usb.filters.push(filter.to_string());
        assert_eq!(config.usb.filters.len(), 1);
    }
}

// ============================================================================
// USB Bridge Integration Tests
// ============================================================================

#[tokio::test]
async fn test_usb_bridge_list_devices_flow() {
    let (bridge, worker) = create_usb_bridge();
    let devices = create_mock_device_list(5);
    let expected_count = devices.len();

    // Simulate USB worker thread
    let handle = thread::spawn(move || {
        if let Ok(UsbCommand::ListDevices { response }) = worker.recv_command() {
            response.send(devices).expect("Failed to send");
            true
        } else {
            false
        }
    });

    // Send list devices command
    let (tx, rx) = tokio::sync::oneshot::channel();
    bridge
        .send_command(UsbCommand::ListDevices { response: tx })
        .await
        .expect("Failed to send");

    let result = rx.await.expect("Failed to receive");
    assert_eq!(result.len(), expected_count);

    assert!(handle.join().unwrap());
}

#[tokio::test]
async fn test_usb_bridge_attach_device_flow() {
    let (bridge, worker) = create_usb_bridge();

    let handle = thread::spawn(move || {
        if let Ok(UsbCommand::AttachDevice {
            device_id,
            client_id,
            response,
        }) = worker.recv_command()
        {
            assert_eq!(device_id.0, 42);
            assert!(!client_id.is_empty());
            response
                .send(Ok(DeviceHandle(100)))
                .expect("Failed to send");
            true
        } else {
            false
        }
    });

    let (tx, rx) = tokio::sync::oneshot::channel();
    bridge
        .send_command(UsbCommand::AttachDevice {
            device_id: DeviceId(42),
            client_id: "test-client-node-id".to_string(),
            response: tx,
        })
        .await
        .expect("Failed to send");

    let result = rx.await.expect("Failed to receive");
    assert!(result.is_ok());
    assert_eq!(result.unwrap().0, 100);

    assert!(handle.join().unwrap());
}

#[tokio::test]
async fn test_usb_bridge_attach_device_not_found() {
    let (bridge, worker) = create_usb_bridge();

    let handle = thread::spawn(move || {
        if let Ok(UsbCommand::AttachDevice { response, .. }) = worker.recv_command() {
            response
                .send(Err(AttachError::DeviceNotFound))
                .expect("Failed to send");
            true
        } else {
            false
        }
    });

    let (tx, rx) = tokio::sync::oneshot::channel();
    bridge
        .send_command(UsbCommand::AttachDevice {
            device_id: DeviceId(999),
            client_id: "test-client".to_string(),
            response: tx,
        })
        .await
        .expect("Failed to send");

    let result = rx.await.expect("Failed to receive");
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), AttachError::DeviceNotFound);

    assert!(handle.join().unwrap());
}

#[tokio::test]
async fn test_usb_bridge_detach_device_flow() {
    let (bridge, worker) = create_usb_bridge();

    let handle = thread::spawn(move || {
        if let Ok(UsbCommand::DetachDevice { handle, response }) = worker.recv_command() {
            assert_eq!(handle.0, 100);
            response.send(Ok(())).expect("Failed to send");
            true
        } else {
            false
        }
    });

    let (tx, rx) = tokio::sync::oneshot::channel();
    bridge
        .send_command(UsbCommand::DetachDevice {
            handle: DeviceHandle(100),
            response: tx,
        })
        .await
        .expect("Failed to send");

    let result = rx.await.expect("Failed to receive");
    assert!(result.is_ok());

    assert!(handle.join().unwrap());
}

#[tokio::test]
async fn test_usb_bridge_submit_transfer_flow() {
    let (bridge, worker) = create_usb_bridge();

    let handle = thread::spawn(move || {
        if let Ok(UsbCommand::SubmitTransfer {
            handle,
            request,
            response,
        }) = worker.recv_command()
        {
            assert_eq!(handle.0, 1);
            assert_eq!(request.id.0, 12345);

            // Return mock device descriptor
            let usb_response = UsbResponse {
                id: request.id,
                result: TransferResult::Success {
                    data: vec![
                        0x12, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 0x40, 0x34, 0x12, 0x78, 0x56,
                        0x00, 0x01, 0x01, 0x02, 0x03, 0x01,
                    ],
                },
            };
            response.send(usb_response).expect("Failed to send");
            true
        } else {
            false
        }
    });

    let request = UsbRequest {
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

    let (tx, rx) = tokio::sync::oneshot::channel();
    bridge
        .send_command(UsbCommand::SubmitTransfer {
            handle: DeviceHandle(1),
            request,
            response: tx,
        })
        .await
        .expect("Failed to send");

    let response = rx.await.expect("Failed to receive");
    assert_eq!(response.id.0, 12345);

    if let TransferResult::Success { data } = response.result {
        assert_eq!(data.len(), 18); // Device descriptor size
        assert_eq!(data[0], 0x12); // bLength
        assert_eq!(data[1], 0x01); // bDescriptorType (Device)
    } else {
        panic!("Expected success result");
    }

    assert!(handle.join().unwrap());
}

#[tokio::test]
async fn test_usb_bridge_hotplug_event_flow() {
    let (bridge, worker) = create_usb_bridge();
    let device = create_mock_device_info(1, 0x1234, 0x5678);

    // Worker sends device arrived event
    let handle = thread::spawn({
        let device = device.clone();
        move || {
            worker
                .send_event(UsbEvent::DeviceArrived { device })
                .expect("Failed to send");
        }
    });

    // Receive event
    let result = with_timeout(DEFAULT_TEST_TIMEOUT, bridge.recv_event()).await;
    assert!(result.is_ok());

    let event = result.unwrap().expect("Failed to receive");
    if let UsbEvent::DeviceArrived { device: d } = event {
        assert_eq!(d.id.0, 1);
        assert_eq!(d.vendor_id, 0x1234);
    } else {
        panic!("Wrong event type");
    }

    handle.join().expect("Worker panicked");
}

#[tokio::test]
async fn test_usb_bridge_device_left_event() {
    let (bridge, worker) = create_usb_bridge();

    // Worker sends device left event
    let handle = thread::spawn(move || {
        worker
            .send_event(UsbEvent::DeviceLeft {
                device_id: DeviceId(42),
                invalidated_handles: Vec::new(),
                affected_clients: Vec::new(),
            })
            .expect("Failed to send");
    });

    // Receive event
    let result = with_timeout(DEFAULT_TEST_TIMEOUT, bridge.recv_event()).await;
    assert!(result.is_ok());

    let event = result.unwrap().expect("Failed to receive");
    if let UsbEvent::DeviceLeft { device_id, .. } = event {
        assert_eq!(device_id.0, 42);
    } else {
        panic!("Wrong event type");
    }

    handle.join().expect("Worker panicked");
}

// ============================================================================
// Protocol Message Construction Tests
// ============================================================================

#[test]
fn test_protocol_message_construction() {
    let ping = Message {
        version: CURRENT_VERSION,
        payload: MessagePayload::Ping,
    };

    let pong = Message {
        version: CURRENT_VERSION,
        payload: MessagePayload::Pong,
    };

    let list_request = Message {
        version: CURRENT_VERSION,
        payload: MessagePayload::ListDevicesRequest,
    };

    let list_response = Message {
        version: CURRENT_VERSION,
        payload: MessagePayload::ListDevicesResponse {
            devices: create_mock_device_list(3),
        },
    };

    assert_eq!(ping.version, CURRENT_VERSION);
    assert_eq!(pong.version, CURRENT_VERSION);
    assert_eq!(list_request.version, CURRENT_VERSION);
    assert_eq!(list_response.version, CURRENT_VERSION);
}

#[test]
fn test_attach_response_construction() {
    let success = Message {
        version: CURRENT_VERSION,
        payload: MessagePayload::AttachDeviceResponse {
            result: Ok(DeviceHandle(42)),
        },
    };

    let not_found = Message {
        version: CURRENT_VERSION,
        payload: MessagePayload::AttachDeviceResponse {
            result: Err(AttachError::DeviceNotFound),
        },
    };

    let already_attached = Message {
        version: CURRENT_VERSION,
        payload: MessagePayload::AttachDeviceResponse {
            result: Err(AttachError::AlreadyAttached),
        },
    };

    let permission_denied = Message {
        version: CURRENT_VERSION,
        payload: MessagePayload::AttachDeviceResponse {
            result: Err(AttachError::PermissionDenied),
        },
    };

    assert_eq!(success.version, CURRENT_VERSION);
    assert_eq!(not_found.version, CURRENT_VERSION);
    assert_eq!(already_attached.version, CURRENT_VERSION);
    assert_eq!(permission_denied.version, CURRENT_VERSION);
}

#[test]
fn test_detach_response_construction() {
    let success = Message {
        version: CURRENT_VERSION,
        payload: MessagePayload::DetachDeviceResponse { result: Ok(()) },
    };

    let not_found = Message {
        version: CURRENT_VERSION,
        payload: MessagePayload::DetachDeviceResponse {
            result: Err(DetachError::HandleNotFound),
        },
    };

    assert_eq!(success.version, CURRENT_VERSION);
    assert_eq!(not_found.version, CURRENT_VERSION);
}

// ============================================================================
// Concurrent Command Processing Tests
// ============================================================================

#[tokio::test]
async fn test_multiple_concurrent_commands() {
    let (bridge, worker) = create_usb_bridge();
    let command_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let worker_count = command_count.clone();

    let handle = thread::spawn(move || {
        loop {
            match worker.recv_command() {
                Ok(UsbCommand::ListDevices { response }) => {
                    worker_count.fetch_add(1, Ordering::SeqCst);
                    response.send(vec![]).expect("Failed to send");
                }
                Ok(UsbCommand::Shutdown) => break,
                Ok(_) => continue,
                Err(_) => break,
            }
        }
    });

    let mut tasks = vec![];
    for _ in 0..10 {
        let bridge_clone = bridge.clone();
        tasks.push(tokio::spawn(async move {
            let (tx, rx) = tokio::sync::oneshot::channel();
            bridge_clone
                .send_command(UsbCommand::ListDevices { response: tx })
                .await
                .expect("Failed to send");
            rx.await.expect("Failed to receive")
        }));
    }

    for task in tasks {
        task.await.expect("Task failed");
    }

    bridge
        .send_command(UsbCommand::Shutdown)
        .await
        .expect("Shutdown failed");

    handle.join().expect("Worker panicked");
    assert_eq!(command_count.load(Ordering::SeqCst), 10);
}

// ============================================================================
// Error Recovery Tests
// ============================================================================

#[tokio::test]
async fn test_worker_shutdown_on_bridge_drop() {
    let (bridge, worker) = create_usb_bridge();
    let worker_finished = Arc::new(AtomicBool::new(false));
    let finished_flag = worker_finished.clone();

    let handle = thread::spawn(move || {
        let result = worker.recv_command();
        finished_flag.store(true, Ordering::Release);
        result.is_err()
    });

    drop(bridge);

    let exited_gracefully = handle.join().unwrap();
    assert!(exited_gracefully);
    assert!(worker_finished.load(Ordering::Acquire));
}
