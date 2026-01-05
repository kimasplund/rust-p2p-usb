//! USB Bridge Integration Tests
//!
//! Tests for the async channel bridge between Tokio runtime and USB thread.
//!
//! # Test Scenarios
//! - Channel creation and basic communication
//! - Command/event message flow
//! - Worker thread lifecycle
//! - Concurrent access patterns
//! - Channel capacity and backpressure
//!
//! Run with: `cargo test -p common --test usb_bridge_tests`

use common::test_utils::{
    create_mock_device_info, create_mock_device_list, with_timeout, DEFAULT_TEST_TIMEOUT,
};
use common::{UsbCommand, UsbEvent, create_usb_bridge};
use protocol::{
    DeviceHandle, DeviceId, RequestId, TransferResult, TransferType, UsbRequest, UsbResponse,
};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tokio::sync::oneshot;

// ============================================================================
// Bridge Creation Tests
// ============================================================================

#[test]
fn test_create_usb_bridge() {
    let (bridge, worker) = create_usb_bridge();

    // Verify both ends are created
    // We can't inspect internals, but we can verify they exist
    drop(bridge);
    drop(worker);
}

#[tokio::test]
async fn test_bridge_channels_are_connected() {
    let (bridge, worker) = create_usb_bridge();

    // Spawn worker thread that echoes back device list
    let handle = thread::spawn(move || {
        if let Ok(cmd) = worker.recv_command() {
            if let UsbCommand::ListDevices { response } = cmd {
                let devices = create_mock_device_list(3);
                let _ = response.send(devices);
            }
        }
    });

    // Send command from async context
    let (tx, rx) = oneshot::channel();
    bridge
        .send_command(UsbCommand::ListDevices { response: tx })
        .await
        .expect("Failed to send command");

    // Receive response
    let result = with_timeout(DEFAULT_TEST_TIMEOUT, rx).await;
    assert!(result.is_ok());

    let devices = result.unwrap().expect("Failed to receive response");
    assert_eq!(devices.len(), 3);

    handle.join().expect("Worker thread panicked");
}

// ============================================================================
// UsbCommand Message Flow Tests
// ============================================================================

#[tokio::test]
async fn test_list_devices_command_flow() {
    let (bridge, worker) = create_usb_bridge();

    let expected_devices = create_mock_device_list(5);
    let expected_count = expected_devices.len();

    // Worker thread
    let handle = thread::spawn(move || {
        let cmd = worker.recv_command().expect("Failed to receive command");
        if let UsbCommand::ListDevices { response } = cmd {
            response.send(expected_devices).expect("Failed to send response");
            true
        } else {
            false
        }
    });

    // Send ListDevices command
    let (tx, rx) = oneshot::channel();
    bridge
        .send_command(UsbCommand::ListDevices { response: tx })
        .await
        .expect("Failed to send command");

    let devices = rx.await.expect("Failed to receive devices");
    assert_eq!(devices.len(), expected_count);

    assert!(handle.join().unwrap());
}

#[tokio::test]
async fn test_attach_device_command_flow() {
    let (bridge, worker) = create_usb_bridge();
    let test_device_id = DeviceId(42);
    let expected_handle = DeviceHandle(100);

    // Worker thread
    let handle = thread::spawn(move || {
        let cmd = worker.recv_command().expect("Failed to receive command");
        if let UsbCommand::AttachDevice {
            device_id,
            client_id,
            response,
        } = cmd
        {
            assert_eq!(device_id.0, 42);
            assert_eq!(client_id, "test-client");
            response.send(Ok(expected_handle)).expect("Failed to send");
            true
        } else {
            false
        }
    });

    // Send AttachDevice command
    let (tx, rx) = oneshot::channel();
    bridge
        .send_command(UsbCommand::AttachDevice {
            device_id: test_device_id,
            client_id: "test-client".to_string(),
            response: tx,
        })
        .await
        .expect("Failed to send command");

    let result = rx.await.expect("Failed to receive");
    assert!(result.is_ok());
    assert_eq!(result.unwrap().0, 100);

    assert!(handle.join().unwrap());
}

#[tokio::test]
async fn test_attach_device_error_flow() {
    let (bridge, worker) = create_usb_bridge();

    // Worker thread returns error
    let handle = thread::spawn(move || {
        let cmd = worker.recv_command().expect("Failed to receive command");
        if let UsbCommand::AttachDevice { response, .. } = cmd {
            response
                .send(Err(protocol::AttachError::DeviceNotFound))
                .expect("Failed to send");
            true
        } else {
            false
        }
    });

    // Send AttachDevice command
    let (tx, rx) = oneshot::channel();
    bridge
        .send_command(UsbCommand::AttachDevice {
            device_id: DeviceId(999),
            client_id: "test-client".to_string(),
            response: tx,
        })
        .await
        .expect("Failed to send command");

    let result = rx.await.expect("Failed to receive");
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), protocol::AttachError::DeviceNotFound);

    assert!(handle.join().unwrap());
}

#[tokio::test]
async fn test_detach_device_command_flow() {
    let (bridge, worker) = create_usb_bridge();
    let test_handle = DeviceHandle(50);

    // Worker thread
    let handle = thread::spawn(move || {
        let cmd = worker.recv_command().expect("Failed to receive command");
        if let UsbCommand::DetachDevice { handle, response } = cmd {
            assert_eq!(handle.0, 50);
            response.send(Ok(())).expect("Failed to send");
            true
        } else {
            false
        }
    });

    // Send DetachDevice command
    let (tx, rx) = oneshot::channel();
    bridge
        .send_command(UsbCommand::DetachDevice {
            handle: test_handle,
            response: tx,
        })
        .await
        .expect("Failed to send command");

    let result = rx.await.expect("Failed to receive");
    assert!(result.is_ok());

    assert!(handle.join().unwrap());
}

#[tokio::test]
async fn test_submit_transfer_command_flow() {
    let (bridge, worker) = create_usb_bridge();

    let test_request = UsbRequest {
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

    let expected_response = UsbResponse {
        id: RequestId(12345),
        result: TransferResult::Success {
            data: vec![0x12, 0x01], // Partial device descriptor
        },
    };

    // Worker thread
    let handle = thread::spawn({
        let expected = expected_response.clone();
        move || {
            let cmd = worker.recv_command().expect("Failed to receive command");
            if let UsbCommand::SubmitTransfer {
                handle,
                request,
                response,
            } = cmd
            {
                assert_eq!(handle.0, 1);
                assert_eq!(request.id.0, 12345);
                response.send(expected).expect("Failed to send");
                true
            } else {
                false
            }
        }
    });

    // Send SubmitTransfer command
    let (tx, rx) = oneshot::channel();
    bridge
        .send_command(UsbCommand::SubmitTransfer {
            handle: DeviceHandle(1),
            request: test_request,
            response: tx,
        })
        .await
        .expect("Failed to send command");

    let response = rx.await.expect("Failed to receive");
    assert_eq!(response.id.0, 12345);
    if let TransferResult::Success { data } = response.result {
        assert_eq!(data.len(), 2);
    } else {
        panic!("Expected success result");
    }

    assert!(handle.join().unwrap());
}

#[tokio::test]
async fn test_shutdown_command_flow() {
    let (bridge, worker) = create_usb_bridge();
    let shutdown_received = Arc::new(AtomicBool::new(false));
    let shutdown_flag = shutdown_received.clone();

    // Worker thread
    let handle = thread::spawn(move || {
        loop {
            match worker.recv_command() {
                Ok(UsbCommand::Shutdown) => {
                    shutdown_flag.store(true, Ordering::Release);
                    break;
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
    });

    // Send Shutdown command
    bridge
        .send_command(UsbCommand::Shutdown)
        .await
        .expect("Failed to send shutdown");

    handle.join().expect("Worker thread panicked");
    assert!(shutdown_received.load(Ordering::Acquire));
}

// ============================================================================
// UsbEvent Message Flow Tests
// ============================================================================

#[tokio::test]
async fn test_device_arrived_event_flow() {
    let (bridge, worker) = create_usb_bridge();
    let device_info = create_mock_device_info(1, 0x1234, 0x5678);

    // Worker thread sends event
    let handle = thread::spawn({
        let device = device_info.clone();
        move || {
            worker
                .send_event(UsbEvent::DeviceArrived { device })
                .expect("Failed to send event");
        }
    });

    // Receive event in async context
    let result = with_timeout(DEFAULT_TEST_TIMEOUT, bridge.recv_event()).await;
    assert!(result.is_ok());

    let event = result.unwrap().expect("Failed to receive event");
    if let UsbEvent::DeviceArrived { device } = event {
        assert_eq!(device.id.0, 1);
        assert_eq!(device.vendor_id, 0x1234);
        assert_eq!(device.product_id, 0x5678);
    } else {
        panic!("Wrong event type");
    }

    handle.join().expect("Worker thread panicked");
}

#[tokio::test]
async fn test_device_left_event_flow() {
    let (bridge, worker) = create_usb_bridge();
    let device_id = DeviceId(42);

    // Worker thread sends event
    let handle = thread::spawn(move || {
        worker
            .send_event(UsbEvent::DeviceLeft { device_id })
            .expect("Failed to send event");
    });

    // Receive event in async context
    let result = with_timeout(DEFAULT_TEST_TIMEOUT, bridge.recv_event()).await;
    assert!(result.is_ok());

    let event = result.unwrap().expect("Failed to receive event");
    if let UsbEvent::DeviceLeft { device_id } = event {
        assert_eq!(device_id.0, 42);
    } else {
        panic!("Wrong event type");
    }

    handle.join().expect("Worker thread panicked");
}

#[tokio::test]
async fn test_multiple_events_in_sequence() {
    let (bridge, worker) = create_usb_bridge();

    // Worker thread sends multiple events
    let handle = thread::spawn(move || {
        for i in 1..=5 {
            let device = create_mock_device_info(i, 0x1000 + i as u16, 0x2000 + i as u16);
            worker
                .send_event(UsbEvent::DeviceArrived { device })
                .expect("Failed to send event");
        }

        // Small delay then send device left events
        thread::sleep(Duration::from_millis(10));

        for i in 1..=3 {
            worker
                .send_event(UsbEvent::DeviceLeft {
                    device_id: DeviceId(i),
                })
                .expect("Failed to send event");
        }
    });

    // Receive all events
    let mut arrived_count = 0;
    let mut left_count = 0;

    for _ in 0..8 {
        let result = with_timeout(DEFAULT_TEST_TIMEOUT, bridge.recv_event()).await;
        if let Ok(Ok(event)) = result {
            match event {
                UsbEvent::DeviceArrived { .. } => arrived_count += 1,
                UsbEvent::DeviceLeft { .. } => left_count += 1,
            }
        }
    }

    assert_eq!(arrived_count, 5);
    assert_eq!(left_count, 3);

    handle.join().expect("Worker thread panicked");
}

// ============================================================================
// Worker Thread Lifecycle Tests
// ============================================================================

#[tokio::test]
async fn test_worker_try_recv_non_blocking() {
    let (bridge, worker) = create_usb_bridge();

    // Worker thread checks for commands without blocking
    let handle = thread::spawn(move || {
        // Should return None immediately when no commands
        let result = worker.try_recv_command();
        result.is_none()
    });

    let no_command = handle.join().unwrap();
    assert!(no_command);

    drop(bridge);
}

#[tokio::test]
async fn test_worker_graceful_shutdown_on_bridge_drop() {
    let (bridge, worker) = create_usb_bridge();
    let worker_finished = Arc::new(AtomicBool::new(false));
    let finished_flag = worker_finished.clone();

    // Worker thread waits for commands
    let handle = thread::spawn(move || {
        // This should error when bridge is dropped
        let result = worker.recv_command();
        finished_flag.store(true, Ordering::Release);
        result.is_err()
    });

    // Drop bridge, which should close the channel
    drop(bridge);

    // Worker should exit
    let closed_gracefully = handle.join().unwrap();
    assert!(closed_gracefully);
    assert!(worker_finished.load(Ordering::Acquire));
}

#[tokio::test]
async fn test_bridge_graceful_shutdown_on_worker_drop() {
    let (bridge, worker) = create_usb_bridge();

    // Drop worker immediately
    drop(worker);

    // Send command should fail
    let (tx, _rx) = oneshot::channel();
    let result = bridge
        .send_command(UsbCommand::ListDevices { response: tx })
        .await;

    assert!(result.is_err());
}

// ============================================================================
// Concurrent Access Pattern Tests
// ============================================================================

#[tokio::test]
async fn test_multiple_commands_from_single_bridge() {
    let (bridge, worker) = create_usb_bridge();
    let command_count = Arc::new(AtomicU32::new(0));
    let worker_count = command_count.clone();

    // Worker thread handles multiple commands
    let handle = thread::spawn(move || {
        loop {
            match worker.recv_command() {
                Ok(UsbCommand::ListDevices { response }) => {
                    worker_count.fetch_add(1, Ordering::SeqCst);
                    let _ = response.send(vec![]);
                }
                Ok(UsbCommand::Shutdown) => break,
                Ok(_) => continue,
                Err(_) => break,
            }
        }
    });

    // Send multiple commands sequentially
    for _ in 0..10 {
        let (tx, rx) = oneshot::channel();
        bridge
            .send_command(UsbCommand::ListDevices { response: tx })
            .await
            .expect("Failed to send");
        rx.await.expect("Failed to receive");
    }

    bridge
        .send_command(UsbCommand::Shutdown)
        .await
        .expect("Failed to shutdown");

    handle.join().expect("Worker panicked");
    assert_eq!(command_count.load(Ordering::SeqCst), 10);
}

#[tokio::test]
async fn test_concurrent_commands_from_cloned_bridges() {
    let (bridge, worker) = create_usb_bridge();
    let command_count = Arc::new(AtomicU32::new(0));
    let worker_count = command_count.clone();

    // Worker thread
    let handle = thread::spawn(move || {
        loop {
            match worker.recv_command() {
                Ok(UsbCommand::ListDevices { response }) => {
                    worker_count.fetch_add(1, Ordering::SeqCst);
                    let _ = response.send(vec![]);
                }
                Ok(UsbCommand::Shutdown) => break,
                Ok(_) => continue,
                Err(_) => break,
            }
        }
    });

    // Clone bridge and send from multiple tasks concurrently
    let mut tasks = vec![];
    for _ in 0..5 {
        let bridge_clone = bridge.clone();
        tasks.push(tokio::spawn(async move {
            for _ in 0..10 {
                let (tx, rx) = oneshot::channel();
                bridge_clone
                    .send_command(UsbCommand::ListDevices { response: tx })
                    .await
                    .expect("Failed to send");
                rx.await.expect("Failed to receive");
            }
        }));
    }

    // Wait for all tasks
    for task in tasks {
        task.await.expect("Task panicked");
    }

    bridge
        .send_command(UsbCommand::Shutdown)
        .await
        .expect("Failed to shutdown");

    handle.join().expect("Worker panicked");
    assert_eq!(command_count.load(Ordering::SeqCst), 50); // 5 tasks * 10 commands
}

// ============================================================================
// Error Handling Tests
// ============================================================================

#[tokio::test]
async fn test_command_with_dropped_response_channel() {
    let (bridge, worker) = create_usb_bridge();

    // Worker thread - tries to send but response channel is dropped
    let handle = thread::spawn(move || {
        if let Ok(UsbCommand::ListDevices { response }) = worker.recv_command() {
            // Simulate response channel already dropped
            let result = response.send(vec![]);
            // send returns Err if receiver is dropped
            result.is_err()
        } else {
            false
        }
    });

    // Send command but drop receiver immediately
    let (tx, rx) = oneshot::channel();
    bridge
        .send_command(UsbCommand::ListDevices { response: tx })
        .await
        .expect("Failed to send");

    // Drop receiver before worker responds
    drop(rx);

    // Give worker time to process
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Worker should handle the dropped channel gracefully
    let handled_gracefully = handle.join().unwrap();
    assert!(handled_gracefully);
}

// ============================================================================
// Channel Capacity Tests
// ============================================================================

#[tokio::test]
async fn test_event_channel_capacity() {
    let (bridge, worker) = create_usb_bridge();

    // Worker thread sends many events
    let handle = thread::spawn(move || {
        // The channel has capacity of 256, try to fill it
        for i in 0..200 {
            let device = create_mock_device_info(i, 0x1000, 0x2000);
            if worker.send_event(UsbEvent::DeviceArrived { device }).is_err() {
                return i; // Return how many we sent before error
            }
        }
        200
    });

    // Small delay to let events queue up
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Drain events
    let mut received = 0;
    loop {
        let result = with_timeout(Duration::from_millis(100), bridge.recv_event()).await;
        match result {
            Ok(Ok(_)) => received += 1,
            _ => break,
        }
    }

    let sent = handle.join().unwrap();
    assert_eq!(received, sent);
    assert!(sent > 0);
}
