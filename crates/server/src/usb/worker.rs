//! USB worker thread
//!
//! Dedicated thread for handling USB events and transfers.
//! Runs libusb_handle_events() loop and communicates with Tokio runtime via channels.
//!
//! This module implements the hybrid sync-async architecture where USB operations
//! run in a dedicated blocking thread and communicate with the Tokio runtime via
//! async channels.

use crate::usb::{manager::DeviceManager, transfers::execute_transfer};
use common::{UsbCommand, UsbWorker};
use rusb::UsbContext;
use std::time::Duration;
use tracing::{debug, error, info, warn};

/// USB worker thread
///
/// Manages the USB context, device manager, and event loop.
/// Processes commands from the Tokio runtime and sends events back.
pub struct UsbWorkerThread {
    /// Device manager for USB operations
    manager: DeviceManager,
    /// Communication channel with Tokio runtime
    worker: UsbWorker,
}

impl UsbWorkerThread {
    /// Create a new USB worker thread
    pub fn new(worker: UsbWorker, allowed_filters: Vec<String>) -> Result<Self, rusb::Error> {
        // Create device manager with event sender
        let mut manager = DeviceManager::new(worker.event_tx.clone(), allowed_filters)?;

        // Initialize device enumeration and hot-plug
        manager.initialize()?;

        Ok(Self { manager, worker })
    }

    /// Run the USB worker thread event loop
    ///
    /// This is the main loop that:
    /// 1. Checks for incoming commands from Tokio (non-blocking)
    /// 2. Processes USB events (with timeout)
    /// 3. Handles hot-plug notifications (debounced)
    /// 4. Processes any ready debounced events
    ///
    /// The loop continues until a Shutdown command is received.
    pub fn run(mut self) -> Result<(), rusb::Error> {
        info!("USB worker thread started");

        loop {
            // Process incoming commands (non-blocking)
            match self.worker.try_recv_command() {
                Some(UsbCommand::Shutdown) => {
                    info!("USB worker shutting down");
                    break;
                }
                Some(cmd) => {
                    self.handle_command(cmd);
                }
                None => {
                    // No command, continue to USB event processing
                }
            }

            // Process USB events with timeout
            // This allows us to check for commands regularly while handling USB events
            let timeout = Duration::from_millis(100);

            match self.manager.context().handle_events(Some(timeout)) {
                Ok(()) => {
                    // Events processed successfully
                }
                Err(rusb::Error::Interrupted) => {
                    // Interrupted, but not fatal - continue
                    debug!("USB event handling interrupted");
                }
                Err(e) => {
                    warn!("Error handling USB events: {}", e);
                    // Continue despite error - don't crash the thread
                    // For serious errors like NoDevice, we might want to shut down,
                    // but for event handling transient errors, it's safer to retry
                    std::thread::sleep(Duration::from_millis(100));
                }
            }

            // Process any debounced hotplug events that are ready to fire
            // This handles events after their 500ms debounce period has elapsed
            self.manager.process_debounced_events();
        }

        info!("USB worker thread stopped");
        Ok(())
    }

    /// Handle a command from the Tokio runtime
    fn handle_command(&mut self, cmd: UsbCommand) {
        // Wrap in catch_unwind to prevent panics from crashing the USB thread
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.handle_command_inner(cmd)
        }));

        if let Err(e) = result {
            error!("Panic in USB command handler: {:?}", e);
        }
    }

    /// Inner command handler (can panic, caught by handle_command)
    fn handle_command_inner(&mut self, cmd: UsbCommand) {
        match cmd {
            UsbCommand::ListDevices { response } => {
                let devices = self.manager.list_devices();
                debug!("Listing {} devices", devices.len());
                let _ = response.send(devices);
            }

            UsbCommand::AttachDevice {
                device_id,
                client_id,
                response,
            } => {
                debug!("Attaching device {:?} for client {}", device_id, client_id);
                let result = self.manager.attach_device(device_id, client_id);
                let _ = response.send(result);
            }

            UsbCommand::DetachDevice { handle, response } => {
                debug!("Detaching device handle {:?}", handle);
                let result = self.manager.detach_device(handle);
                let _ = response.send(result);
            }

            UsbCommand::SubmitTransfer {
                handle,
                request,
                response,
            } => {
                debug!(
                    "Submitting transfer for handle {:?}, request {:?}",
                    handle, request.id
                );

                // Get device by handle
                let usb_response = match self.manager.get_device_by_handle(handle) {
                    Some(device) => {
                        // Get mutable handle for transfer
                        match device.handle_mut() {
                            Some(device_handle) => {
                                // Execute the transfer
                                execute_transfer(device_handle, request.transfer, request.id)
                            }
                            None => {
                                // Device not open
                                warn!("Device handle {:?} not open for transfer", handle);
                                protocol::UsbResponse {
                                    id: request.id,
                                    result: protocol::TransferResult::Error {
                                        error: protocol::UsbError::NotFound,
                                    },
                                }
                            }
                        }
                    }
                    None => {
                        // Handle not found
                        warn!("Device handle {:?} not found", handle);
                        protocol::UsbResponse {
                            id: request.id,
                            result: protocol::TransferResult::Error {
                                error: protocol::UsbError::NotFound,
                            },
                        }
                    }
                };

                let _ = response.send(usb_response);
            }

            UsbCommand::ResetDevice { handle, response } => {
                debug!("Resetting device handle {:?}", handle);
                let result = match self.manager.get_device_by_handle(handle) {
                    Some(device) => match device.handle_mut() {
                        Some(device_handle) => match device_handle.reset() {
                            Ok(_) => Ok(()),
                            Err(e) => Err(crate::usb::transfers::map_rusb_error(e)),
                        },
                        None => Err(protocol::UsbError::NotFound),
                    },
                    None => Err(protocol::UsbError::NotFound),
                };
                let _ = response.send(result);
            }

            UsbCommand::Shutdown => {
                // Already handled in main loop
                unreachable!()
            }
        }
    }
}

/// Spawn the USB worker thread
///
/// This creates a new OS thread for USB operations and returns a join handle.
/// The thread will run until a Shutdown command is received or an error occurs.
pub fn spawn_usb_worker(
    worker: UsbWorker,
    filters: Vec<String>,
) -> std::thread::JoinHandle<Result<(), rusb::Error>> {
    std::thread::Builder::new()
        .name("usb-worker".to_string())
        .spawn(move || {
            let worker_thread = UsbWorkerThread::new(worker, filters)?;
            worker_thread.run()
        })
        .expect("Failed to spawn USB worker thread")
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::create_usb_bridge;

    #[test]
    fn test_usb_worker_creation() {
        let (_bridge, worker) = create_usb_bridge();

        // Try to create worker thread (may fail if no USB access)
        let result = UsbWorkerThread::new(worker, vec![]);

        // We don't assert success because USB context creation may fail without permissions
        // Just verify we can attempt to create it
        match result {
            Ok(_) => {
                // USB access available
            }
            Err(e) => {
                // Expected if no USB permissions
                eprintln!(
                    "USB worker creation failed (expected without permissions): {}",
                    e
                );
            }
        }
    }
}
