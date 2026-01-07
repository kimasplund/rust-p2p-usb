//! Client Connection Handler
//!
//! Manages per-client state, handles protocol messages, and maintains
//! the communication bridge between the client and USB subsystem.

use anyhow::{Context, Result, anyhow};
use common::UsbBridge;
use common::{UsbCommand, UsbEvent};
use iroh::PublicKey as EndpointId;
use iroh::endpoint::{Connection, RecvStream, SendStream};

use protocol::{
    CURRENT_VERSION, DeviceHandle, DeviceId, Message, MessagePayload, RequestId, UsbRequest,
    decode_framed, encode_framed, validate_version,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, Mutex};
use tokio::time;
use tracing::{debug, error, info, trace, warn};

/// Timeout for receiving messages (2 minutes)
const MESSAGE_TIMEOUT: Duration = Duration::from_secs(120);

/// Keep-alive ping interval (30 seconds)
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);

/// Pending USB transfer awaiting completion or cancellation
struct PendingTransfer {
    /// Request ID for matching responses
    request_id: RequestId,
    /// Sender to signal cancellation
    cancel_tx: broadcast::Sender<()>,
}

/// Tracks pending transfers per device handle
type PendingTransfersMap = Arc<Mutex<HashMap<DeviceHandle, Vec<PendingTransfer>>>>;

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
    /// Pending transfers per device handle (for cancellation on hot-unplug)
    pending_transfers: PendingTransfersMap,
    /// Last activity timestamp (for keep-alive)
    last_activity: Instant,
    /// Client supports push notifications (determined during capability exchange)
    client_supports_push: bool,
}

impl ClientConnection {
    /// Create a new client connection handler
    pub fn new(endpoint_id: EndpointId, connection: Connection, usb_bridge: UsbBridge) -> Self {
        Self {
            endpoint_id,
            connection,
            usb_bridge,
            attached_devices: HashMap::new(),
            pending_transfers: Arc::new(Mutex::new(HashMap::new())),
            last_activity: Instant::now(),
            client_supports_push: false,
        }
    }

    /// Exchange capabilities with client
    async fn exchange_capabilities(&mut self) -> Result<()> {
        // Wait for client capabilities on a bidirectional stream
        let (mut send, mut recv) =
            tokio::time::timeout(Duration::from_secs(10), self.connection.accept_bi())
                .await
                .context("Timeout waiting for capability exchange")?
                .context("Failed to accept capability exchange stream")?;

        let message_bytes = protocol::read_framed_async(&mut recv)
            .await
            .context("Failed to read client capabilities")?;
        let message: Message = decode_framed(&message_bytes)?;

        if let MessagePayload::ClientCapabilities {
            supports_push_notifications,
        } = message.payload
        {
            self.client_supports_push = supports_push_notifications;
            info!(
                "Client capabilities: push_notifications={}",
                supports_push_notifications
            );
        } else {
            return Err(anyhow!(
                "Expected ClientCapabilities, got {:?}",
                message.payload
            ));
        }

        // Send server capabilities
        let response = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::ServerCapabilities {
                will_send_notifications: true,
            },
        };
        let response_bytes = encode_framed(&response)?;
        protocol::write_framed_async(&mut send, &response_bytes).await?;
        send.finish()
            .context("Failed to finish capability response")?;

        Ok(())
    }

    /// Run the connection handler
    ///
    /// Processes incoming QUIC streams and USB events until the connection
    /// closes or an error occurs.
    pub async fn run(&mut self) -> Result<()> {
        info!("Starting connection handler for {}", self.endpoint_id);

        // Exchange capabilities with client
        if let Err(e) = self.exchange_capabilities().await {
            warn!(
                "Capability exchange failed: {:#}, continuing without push notifications",
                e
            );
        }

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
        info!(
            "Detach device request: {:?} from {}",
            handle, self.endpoint_id
        );

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

        // Create cancellation channel for this transfer
        let (cancel_tx, mut cancel_rx) = broadcast::channel::<()>(1);
        let pending = PendingTransfer {
            request_id: request.id,
            cancel_tx,
        };

        // Register pending transfer
        let handle = request.handle;
        let request_id = request.id;
        {
            let mut pending_map = self.pending_transfers.lock().await;
            pending_map.entry(handle).or_default().push(pending);
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

        // Wait for either transfer completion or cancellation
        let response = tokio::select! {
            result = rx => {
                match result {
                    Ok(response) => response,
                    Err(_) => {
                        warn!("Transfer response channel closed for {:?}", request_id);
                        protocol::UsbResponse {
                            id: request_id,
                            result: protocol::TransferResult::Error {
                                error: protocol::UsbError::NoDevice,
                            },
                        }
                    }
                }
            }
            _ = cancel_rx.recv() => {
                info!("Transfer {:?} cancelled due to device hot-unplug", request_id);
                protocol::UsbResponse {
                    id: request_id,
                    result: protocol::TransferResult::Error {
                        error: protocol::UsbError::NoDevice,
                    },
                }
            }
        };

        // Remove this transfer from pending map
        {
            let mut pending_map = self.pending_transfers.lock().await;
            if let Some(transfers) = pending_map.get_mut(&handle) {
                transfers.retain(|t| t.request_id != request_id);
                if transfers.is_empty() {
                    pending_map.remove(&handle);
                }
            }
        }

        Ok(MessagePayload::TransferComplete { response })
    }

    /// Send a push notification via unidirectional QUIC stream
    async fn send_push_notification(&self, payload: MessagePayload) -> Result<()> {
        if !self.client_supports_push {
            debug!("Client does not support push notifications, skipping");
            return Ok(());
        }

        // Open unidirectional stream (server -> client)
        let mut send = self
            .connection
            .open_uni()
            .await
            .context("Failed to open unidirectional stream for notification")?;

        let message = Message {
            version: CURRENT_VERSION,
            payload,
        };

        let framed = encode_framed(&message)?;
        protocol::write_framed_async(&mut send, &framed).await?;
        send.finish()
            .context("Failed to finish notification stream")?;

        debug!("Push notification sent successfully");
        Ok(())
    }

    /// Handle USB events from the USB subsystem
    async fn handle_usb_event(&mut self, event: UsbEvent) -> Result<()> {
        match event {
            UsbEvent::DeviceArrived { device } => {
                debug!("Device arrived: {:?}", device.id);
                // Send push notification to client
                if let Err(e) = self
                    .send_push_notification(MessagePayload::DeviceArrivedNotification { device })
                    .await
                {
                    warn!("Failed to send device arrived notification: {:#}", e);
                }
            }

            UsbEvent::DeviceLeft {
                device_id,
                invalidated_handles,
                affected_clients,
            } => {
                info!(
                    "Device left: {:?}, invalidated_handles={:?}, affected_clients={:?}",
                    device_id, invalidated_handles, affected_clients
                );

                // Cancel all pending transfers for invalidated handles
                let cancelled_count =
                    self.cancel_pending_transfers(&invalidated_handles).await;
                if cancelled_count > 0 {
                    info!(
                        "Cancelled {} pending transfers for device {:?}",
                        cancelled_count, device_id
                    );
                }

                // Remove invalidated handles from our attached devices map
                for handle in &invalidated_handles {
                    if self.attached_devices.remove(handle).is_some() {
                        info!("Auto-detached device: handle={:?}", handle);
                    }
                }

                // Also check for any handles we track that weren't in invalidated_handles
                // (fallback for consistency)
                let remaining_handles: Vec<DeviceHandle> = self
                    .attached_devices
                    .iter()
                    .filter(|(_, id)| **id == device_id)
                    .map(|(handle, _)| *handle)
                    .collect();

                // Cancel pending transfers for remaining handles too
                if !remaining_handles.is_empty() {
                    let additional_cancelled =
                        self.cancel_pending_transfers(&remaining_handles).await;
                    if additional_cancelled > 0 {
                        info!(
                            "Cancelled {} additional pending transfers (fallback)",
                            additional_cancelled
                        );
                    }
                }

                for handle in remaining_handles {
                    self.attached_devices.remove(&handle);
                    info!("Auto-detached device (fallback): handle={:?}", handle);
                }

                // Send push notification to client
                if let Err(e) = self
                    .send_push_notification(MessagePayload::DeviceRemovedNotification {
                        device_id,
                        invalidated_handles,
                        reason: protocol::DeviceRemovalReason::Unplugged,
                    })
                    .await
                {
                    warn!("Failed to send device removed notification: {:#}", e);
                }
            }
        }

        Ok(())
    }

    /// Cancel all pending transfers for the given device handles
    ///
    /// Returns the number of transfers cancelled. Each pending transfer will
    /// receive a cancellation signal and respond with UsbError::NoDevice.
    async fn cancel_pending_transfers(&self, handles: &[DeviceHandle]) -> usize {
        let mut cancelled = 0;
        let mut pending_map = self.pending_transfers.lock().await;

        for handle in handles {
            if let Some(transfers) = pending_map.remove(handle) {
                for transfer in transfers {
                    debug!(
                        "Cancelling pending transfer {:?} for device {:?}",
                        transfer.request_id, handle
                    );
                    let _ = transfer.cancel_tx.send(());
                    cancelled += 1;
                }
            }
        }

        cancelled
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
        // Cancel all pending transfers first
        let handles: Vec<DeviceHandle> = self.attached_devices.keys().copied().collect();
        if !handles.is_empty() {
            let cancelled = self.cancel_pending_transfers(&handles).await;
            if cancelled > 0 {
                info!(
                    "Cancelled {} pending transfers during cleanup for {}",
                    cancelled, self.endpoint_id
                );
            }
        }

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
