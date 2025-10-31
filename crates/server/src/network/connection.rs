//! Client Connection Handler
//!
//! Manages per-client state, handles protocol messages, and maintains
//! the communication bridge between the client and USB subsystem.

use anyhow::{Context, Result};
use common::UsbBridge;
use common::{UsbCommand, UsbEvent};
use iroh::{PublicKey as EndpointId};
use iroh::endpoint::{Connection, RecvStream, SendStream};
use protocol::{
    CURRENT_VERSION, DeviceHandle, DeviceId, Message, MessagePayload, UsbRequest, decode_framed,
    encode_framed, validate_version,
};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::time;
use tracing::{debug, error, info, trace, warn};

/// Timeout for receiving messages (2 minutes)
const MESSAGE_TIMEOUT: Duration = Duration::from_secs(120);

/// Keep-alive ping interval (30 seconds)
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);

/// Per-client connection handler
///
/// Manages the state and message flow for a single connected client.
/// Handles QUIC streams, routes messages to USB subsystem, and maintains
/// device attachment state.
pub struct ClientConnection {
    /// Client's EndpointId
    endpoint_id: EndpointId,
    /// QUIC connection
    connection: Connection,
    /// Bridge to USB subsystem
    usb_bridge: UsbBridge,
    /// Attached devices (handle -> device_id mapping)
    attached_devices: HashMap<DeviceHandle, DeviceId>,
    /// Last activity timestamp (for keep-alive)
    last_activity: Instant,
}

impl ClientConnection {
    /// Create a new client connection handler
    pub fn new(endpoint_id: EndpointId, connection: Connection, usb_bridge: UsbBridge) -> Self {
        Self {
            endpoint_id,
            connection,
            usb_bridge,
            attached_devices: HashMap::new(),
            last_activity: Instant::now(),
        }
    }

    /// Run the connection handler
    ///
    /// Processes incoming QUIC streams and USB events until the connection
    /// closes or an error occurs.
    pub async fn run(&mut self) -> Result<()> {
        info!("Starting connection handler for {}", self.endpoint_id);

        // Spawn keep-alive task
        let connection_clone = self.connection.clone();
        let endpoint_id = self.endpoint_id;
        tokio::spawn(async move {
            Self::keepalive_task(connection_clone, endpoint_id).await;
        });

        loop {
            tokio::select! {
                // Accept incoming QUIC bi-directional streams
                stream_result = self.connection.accept_bi() => {
                    match stream_result {
                        Ok((send, recv)) => {
                            self.last_activity = Instant::now();
                            if let Err(e) = self.handle_stream(send, recv).await {
                                error!("Stream handler error: {:#}", e);
                                // Don't break on stream error, continue with other streams
                            }
                        }
                        Err(e) => {
                            warn!("Connection closed: {}", e);
                            break;
                        }
                    }
                }

                // Handle USB events from the USB subsystem
                event_result = self.usb_bridge.recv_event() => {
                    match event_result {
                        Ok(event) => {
                            if let Err(e) = self.handle_usb_event(event).await {
                                error!("USB event handler error: {:#}", e);
                            }
                        }
                        Err(e) => {
                            error!("USB bridge error: {:#}", e);
                            break;
                        }
                    }
                }

                // Timeout check (connection idle too long)
                _ = time::sleep(Duration::from_secs(60)) => {
                    let idle_time = self.last_activity.elapsed();
                    if idle_time > Duration::from_secs(180) {
                        warn!("Connection idle for {:?}, closing", idle_time);
                        break;
                    }
                }
            }
        }

        // Cleanup: detach all devices
        self.cleanup().await;

        info!("Connection handler stopped for {}", self.endpoint_id);
        Ok(())
    }

    /// Handle a single QUIC stream (request-response)
    async fn handle_stream(&mut self, mut send: SendStream, mut recv: RecvStream) -> Result<()> {
        // Read framed message with timeout
        let message_bytes =
            tokio::time::timeout(MESSAGE_TIMEOUT, protocol::read_framed_async(&mut recv))
                .await
                .context("Timeout reading message")?
                .context("Failed to read framed message")?;

        // Decode message
        let message: Message = decode_framed(&message_bytes).context("Failed to decode message")?;

        trace!("Received message: {:?}", message.payload);

        // Validate protocol version
        if let Err(e) = validate_version(&message.version) {
            warn!("Protocol version mismatch: {}", e);
            let error_response = Message {
                version: CURRENT_VERSION,
                payload: MessagePayload::Error {
                    message: format!("Incompatible protocol version: {}", e),
                },
            };
            let response_bytes = encode_framed(&error_response)?;
            protocol::write_framed_async(&mut send, &response_bytes).await?;
            return Ok(());
        }

        // Handle message and get response
        let response_payload = self.handle_message(message.payload).await?;

        // Send response
        let response = Message {
            version: CURRENT_VERSION,
            payload: response_payload,
        };
        let response_bytes = encode_framed(&response)?;
        protocol::write_framed_async(&mut send, &response_bytes).await?;

        Ok(())
    }

    /// Handle a protocol message and return response payload
    async fn handle_message(&mut self, payload: MessagePayload) -> Result<MessagePayload> {
        match payload {
            MessagePayload::ListDevicesRequest => self.handle_list_devices().await,

            MessagePayload::AttachDeviceRequest { device_id } => {
                self.handle_attach_device(device_id).await
            }

            MessagePayload::DetachDeviceRequest { handle } => {
                self.handle_detach_device(handle).await
            }

            MessagePayload::SubmitTransfer { request } => {
                self.handle_submit_transfer(request).await
            }

            MessagePayload::Ping => {
                debug!("Ping from {}", self.endpoint_id);
                Ok(MessagePayload::Pong)
            }

            _ => {
                warn!("Unexpected message type: {:?}", payload);
                Ok(MessagePayload::Error {
                    message: "Unsupported message type".to_string(),
                })
            }
        }
    }

    /// Handle ListDevicesRequest
    async fn handle_list_devices(&self) -> Result<MessagePayload> {
        debug!("Listing devices for {}", self.endpoint_id);

        // Send command to USB subsystem
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.usb_bridge
            .send_command(UsbCommand::ListDevices { response: tx })
            .await?;

        // Wait for response
        let devices = rx.await?;

        Ok(MessagePayload::ListDevicesResponse { devices })
    }

    /// Handle AttachDeviceRequest
    async fn handle_attach_device(&mut self, device_id: DeviceId) -> Result<MessagePayload> {
        info!(
            "Attach device request: {:?} from {}",
            device_id, self.endpoint_id
        );

        // Send command to USB subsystem
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.usb_bridge
            .send_command(UsbCommand::AttachDevice {
                device_id,
                client_id: self.endpoint_id.to_string(),
                response: tx,
            })
            .await?;

        // Wait for response
        let result = rx.await?;

        // Track attached device
        if let Ok(handle) = result {
            self.attached_devices.insert(handle, device_id);
            info!(
                "Device attached: handle={:?}, device={:?}",
                handle, device_id
            );
        }

        Ok(MessagePayload::AttachDeviceResponse { result })
    }

    /// Handle DetachDeviceRequest
    async fn handle_detach_device(&mut self, handle: DeviceHandle) -> Result<MessagePayload> {
        info!("Detach device request: {:?} from {}", handle, self.endpoint_id);

        // Send command to USB subsystem
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.usb_bridge
            .send_command(UsbCommand::DetachDevice {
                handle,
                response: tx,
            })
            .await?;

        // Wait for response
        let result = rx.await?;

        // Remove from tracked devices
        if result.is_ok() {
            self.attached_devices.remove(&handle);
            info!("Device detached: handle={:?}", handle);
        }

        Ok(MessagePayload::DetachDeviceResponse { result })
    }

    /// Handle SubmitTransfer
    async fn handle_submit_transfer(&self, request: UsbRequest) -> Result<MessagePayload> {
        trace!("Submit transfer request: id={:?}", request.id);

        // Verify device is attached
        if !self.attached_devices.contains_key(&request.handle) {
            warn!("Transfer to unattached device: {:?}", request.handle);
            return Ok(MessagePayload::TransferComplete {
                response: protocol::UsbResponse {
                    id: request.id,
                    result: protocol::TransferResult::Error {
                        error: protocol::UsbError::NotFound,
                    },
                },
            });
        }

        // Send command to USB subsystem
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.usb_bridge
            .send_command(UsbCommand::SubmitTransfer {
                handle: request.handle,
                request: request.clone(),
                response: tx,
            })
            .await?;

        // Wait for response
        let response = rx.await?;

        Ok(MessagePayload::TransferComplete { response })
    }

    /// Handle USB events from the USB subsystem
    async fn handle_usb_event(&mut self, event: UsbEvent) -> Result<()> {
        match event {
            UsbEvent::DeviceArrived { device } => {
                debug!("Device arrived: {:?}", device.id);
                // Could notify client if needed
            }

            UsbEvent::DeviceLeft { device_id } => {
                info!("Device left: {:?}", device_id);

                // Find and remove all handles for this device
                let handles: Vec<DeviceHandle> = self
                    .attached_devices
                    .iter()
                    .filter(|(_, id)| **id == device_id)
                    .map(|(handle, _)| *handle)
                    .collect();

                for handle in handles {
                    self.attached_devices.remove(&handle);
                    info!("Auto-detached device: handle={:?}", handle);
                }

                // Could send notification to client
            }
        }

        Ok(())
    }

    /// Keep-alive task: sends periodic pings
    async fn keepalive_task(connection: Connection, endpoint_id: EndpointId) {
        let mut interval = time::interval(KEEPALIVE_INTERVAL);

        loop {
            interval.tick().await;

            // Try to open a stream and send ping
            match connection.open_bi().await {
                Ok((mut send, mut recv)) => {
                    let ping = Message {
                        version: CURRENT_VERSION,
                        payload: MessagePayload::Ping,
                    };

                    if let Ok(bytes) = encode_framed(&ping) {
                        if protocol::write_framed_async(&mut send, &bytes)
                            .await
                            .is_err()
                        {
                            debug!("Keep-alive ping failed for {}", endpoint_id);
                            break;
                        }

                        // Wait for pong (with timeout)
                        match tokio::time::timeout(
                            Duration::from_secs(5),
                            protocol::read_framed_async(&mut recv),
                        )
                        .await
                        {
                            Ok(Ok(_)) => {
                                trace!("Keep-alive pong received from {}", endpoint_id);
                            }
                            _ => {
                                debug!("Keep-alive pong timeout for {}", endpoint_id);
                                break;
                            }
                        }
                    }
                }
                Err(_) => {
                    debug!("Keep-alive stream open failed for {}", endpoint_id);
                    break;
                }
            }
        }

        info!("Keep-alive task stopped for {}", endpoint_id);
    }

    /// Cleanup when connection closes
    async fn cleanup(&mut self) {
        if self.attached_devices.is_empty() {
            return;
        }

        info!(
            "Cleaning up {} attached devices for {}",
            self.attached_devices.len(),
            self.endpoint_id
        );

        // Detach all devices
        for handle in self.attached_devices.keys().copied().collect::<Vec<_>>() {
            let (tx, rx) = tokio::sync::oneshot::channel();
            if self
                .usb_bridge
                .send_command(UsbCommand::DetachDevice {
                    handle,
                    response: tx,
                })
                .await
                .is_ok()
            {
                let _ = rx.await;
            }
        }

        self.attached_devices.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_creation() {
        // Note: Can't easily test with real Iroh connection without network setup
        // Integration tests will cover this
        assert_eq!(MESSAGE_TIMEOUT, Duration::from_secs(120));
        assert_eq!(KEEPALIVE_INTERVAL, Duration::from_secs(30));
    }
}
