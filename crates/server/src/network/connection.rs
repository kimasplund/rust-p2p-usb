//! Client Connection Handler
//!
//! Manages per-client state, handles protocol messages, and maintains
//! the communication bridge between the client and USB subsystem.

use anyhow::{Context, Result, anyhow};
use common::UsbBridge;
use common::{RateLimitResult, SharedRateLimiter, UsbCommand, UsbEvent};
use iroh::PublicKey as EndpointId;
use iroh::endpoint::{Connection, RecvStream, SendStream};

use protocol::{
    AttachError, CURRENT_VERSION, DeviceHandle, DeviceId, DeviceRemovalReason, ForceDetachReason,
    Message, MessagePayload, RequestId, TransferType, UsbRequest, decode_framed, encode_framed,
    validate_version,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, broadcast};
use tokio::time;
use tracing::{debug, error, info, trace, warn};

use crate::audit::{AuditResult, SharedAuditLogger};
use crate::network::notification_aggregator::{NotificationAggregator, PendingNotification};
use crate::policy::{PolicyDecision, PolicyDenialReason, PolicyEngine};

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
    /// Audit logger for compliance logging
    audit_logger: SharedAuditLogger,
    /// Rate limiter for bandwidth control (optional)
    rate_limiter: Option<SharedRateLimiter>,
    /// Notification aggregator for batching rapid device events
    notification_aggregator: NotificationAggregator,
    /// Policy engine for access control
    policy_engine: Arc<PolicyEngine>,
    /// Device info cache for policy checks (device_id -> device_info)
    device_info_cache: HashMap<DeviceId, protocol::DeviceInfo>,
}

impl ClientConnection {
    /// Create a new client connection handler
    pub fn new(
        endpoint_id: EndpointId,
        connection: Connection,
        usb_bridge: UsbBridge,
        audit_logger: SharedAuditLogger,
        rate_limiter: Option<SharedRateLimiter>,
        policy_engine: Arc<PolicyEngine>,
    ) -> Self {
        Self {
            endpoint_id,
            connection,
            usb_bridge,
            attached_devices: HashMap::new(),
            pending_transfers: Arc::new(Mutex::new(HashMap::new())),
            last_activity: Instant::now(),
            client_supports_push: false,
            audit_logger,
            rate_limiter,
            notification_aggregator: NotificationAggregator::new(),
            policy_engine,
            device_info_cache: HashMap::new(),
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

        // Use interval for periodic checks to avoid timer reset issues in select!
        let mut session_check_interval = time::interval(Duration::from_secs(30));
        session_check_interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);

        loop {
            let flush_delay = self
                .notification_aggregator
                .time_until_flush()
                .unwrap_or(Duration::from_secs(60));

            tokio::select! {
                // Accept incoming QUIC bi-directional streams
                stream_result = self.connection.accept_bi() => {
                    match stream_result {
                        Ok((send, recv)) => {
                            self.last_activity = Instant::now();
                            if let Err(e) = self.handle_stream(send, recv).await {
                                error!("Stream handler error: {:#}", e);
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

                // Flush aggregated notifications when window expires
                _ = time::sleep(flush_delay), if self.notification_aggregator.has_pending() => {
                    if self.notification_aggregator.should_flush() {
                        if let Err(e) = self.flush_aggregated_notifications().await {
                            warn!("Failed to flush aggregated notifications: {:#}", e);
                        }
                    }
                }

                // Check for expired sessions (every 30 seconds)
                _ = session_check_interval.tick() => {
                    let idle_time = self.last_activity.elapsed();
                    if idle_time > Duration::from_secs(180) {
                        warn!("Connection idle for {:?}, closing", idle_time);
                        break;
                    }

                    // Check for expired sessions belonging to this client
                    if let Err(e) = self.handle_expired_sessions().await {
                        warn!("Failed to handle expired sessions: {:#}", e);
                    }
                }
            }

            // Check for immediate flush after processing events (e.g., max notifications reached)
            if self.notification_aggregator.should_flush() {
                if let Err(e) = self.flush_aggregated_notifications().await {
                    warn!("Failed to flush aggregated notifications: {:#}", e);
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

            MessagePayload::Heartbeat {
                sequence,
                timestamp_ms,
            } => {
                debug!(
                    "Heartbeat from {}: seq={}, timestamp={}",
                    self.endpoint_id, sequence, timestamp_ms
                );
                self.last_activity = Instant::now();

                // Get current server timestamp
                let server_timestamp_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);

                Ok(MessagePayload::HeartbeatAck {
                    sequence,
                    client_timestamp_ms: timestamp_ms,
                    server_timestamp_ms,
                })
            }

            MessagePayload::GetSharingStatusRequest { device_id } => {
                self.handle_get_sharing_status(device_id).await
            }

            MessagePayload::LockDeviceRequest {
                handle,
                write_access,
                timeout_secs: _,
            } => self.handle_lock_device(handle, write_access).await,

            MessagePayload::UnlockDeviceRequest { handle } => {
                self.handle_unlock_device(handle).await
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

        // Get device info for policy check (from cache or fetch)
        let device_info = if let Some(info) = self.device_info_cache.get(&device_id) {
            info.clone()
        } else {
            // Fetch device list to get info
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.usb_bridge
                .send_command(UsbCommand::ListDevices { response: tx })
                .await?;
            let devices = rx.await?;

            // Update cache with all devices
            for dev in &devices {
                self.device_info_cache.insert(dev.id, dev.clone());
            }

            match devices.into_iter().find(|d| d.id == device_id) {
                Some(info) => info,
                None => {
                    return Ok(MessagePayload::AttachDeviceResponse {
                        result: Err(AttachError::DeviceNotFound),
                    });
                }
            }
        };

        // Check policy before attaching
        let policy_decision = self.policy_engine.check_access(&self.endpoint_id, &device_info);
        match policy_decision {
            PolicyDecision::Allow => {
                // Policy allows access, continue with attach
            }
            PolicyDecision::Deny(reason) => {
                let endpoint_id_str = self.endpoint_id.to_string();
                let attach_error = Self::policy_denial_to_attach_error(&reason);

                warn!(
                    "Policy denied attach for device {:?} from {}: {}",
                    device_id, endpoint_id_str, reason
                );

                // Audit log: policy denied attach
                if let Some(ref logger) = *self.audit_logger {
                    logger.log_device_attach(
                        &endpoint_id_str,
                        device_id,
                        None,
                        None,
                        AuditResult::Failure,
                        Some(format!("Policy denied: {}", reason)),
                    );
                }

                return Ok(MessagePayload::AttachDeviceResponse {
                    result: Err(attach_error),
                });
            }
        }

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

        // Track attached device and audit log
        let endpoint_id_str = self.endpoint_id.to_string();
        match &result {
            Ok(handle) => {
                self.attached_devices.insert(*handle, device_id);
                info!(
                    "Device attached: handle={:?}, device={:?}",
                    handle, device_id
                );

                // Register session with policy engine for duration/time window monitoring
                self.policy_engine
                    .register_session(*handle, device_id, &device_info, self.endpoint_id)
                    .await;

                // Audit log: successful attach
                if let Some(ref logger) = *self.audit_logger {
                    logger.log_device_attach(
                        &endpoint_id_str,
                        device_id,
                        Some(*handle),
                        None,
                        AuditResult::Success,
                        None,
                    );
                }
            }
            Err(e) => {
                // Audit log: failed attach
                if let Some(ref logger) = *self.audit_logger {
                    logger.log_device_attach(
                        &endpoint_id_str,
                        device_id,
                        None,
                        None,
                        AuditResult::Failure,
                        Some(format!("{:?}", e)),
                    );
                }
            }
        }

        Ok(MessagePayload::AttachDeviceResponse { result })
    }

    /// Convert policy denial reason to AttachError
    fn policy_denial_to_attach_error(reason: &PolicyDenialReason) -> AttachError {
        match reason {
            PolicyDenialReason::ClientNotAllowed => AttachError::PolicyDenied {
                reason: "Client not in allowed list for this device".to_string(),
            },
            PolicyDenialReason::OutsideTimeWindow {
                current_time,
                allowed_windows,
            } => AttachError::OutsideTimeWindow {
                current_time: current_time.clone(),
                allowed_windows: allowed_windows.clone(),
            },
            PolicyDenialReason::SessionDurationExceeded { max_duration } => {
                AttachError::PolicyDenied {
                    reason: format!(
                        "Session would exceed maximum duration of {:?}",
                        max_duration
                    ),
                }
            }
            PolicyDenialReason::DeviceClassRestricted { device_class } => {
                AttachError::DeviceClassRestricted {
                    device_class: *device_class,
                }
            }
            PolicyDenialReason::NoMatchingPolicy => AttachError::PolicyDenied {
                reason: "No matching policy found for this device".to_string(),
            },
        }
    }

    /// Handle DetachDeviceRequest
    async fn handle_detach_device(&mut self, handle: DeviceHandle) -> Result<MessagePayload> {
        info!(
            "Detach device request: {:?} from {}",
            handle, self.endpoint_id
        );

        // Get device_id before we remove it from tracking
        let device_id = self.attached_devices.get(&handle).copied();

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

        // Remove from tracked devices and audit log
        let endpoint_id_str = self.endpoint_id.to_string();
        if result.is_ok() {
            self.attached_devices.remove(&handle);
            info!("Device detached: handle={:?}", handle);

            // Unregister session from policy engine
            self.policy_engine.unregister_session(handle).await;

            // Audit log: successful detach
            if let Some(ref logger) = *self.audit_logger {
                logger.log_device_detach(
                    &endpoint_id_str,
                    handle,
                    device_id,
                    AuditResult::Success,
                    None,
                );
            }
        } else {
            // Audit log: failed detach
            if let Some(ref logger) = *self.audit_logger {
                logger.log_device_detach(
                    &endpoint_id_str,
                    handle,
                    device_id,
                    AuditResult::Failure,
                    result.as_ref().err().map(|e| format!("{:?}", e)),
                );
            }
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

        // Calculate transfer data size for rate limiting
        let transfer_bytes = Self::get_transfer_data_size(&request.transfer);

        // Apply rate limiting if enabled
        if let Some(ref limiter) = self.rate_limiter {
            let client_id = self.endpoint_id.to_string();
            let device_id = self.attached_devices.get(&request.handle).map(|d| d.0);

            // Check rate limit and wait if necessary
            let result = limiter
                .check(Some(&client_id), device_id, transfer_bytes)
                .await;

            match result {
                RateLimitResult::Allowed => {
                    // Use try_acquire to atomically check and consume tokens
                    if !limiter
                        .try_acquire(Some(&client_id), device_id, transfer_bytes)
                        .await
                    {
                        trace!(
                            "Rate limit: transfer {:?} delayed, tokens unavailable",
                            request.id
                        );
                    }
                }
                RateLimitResult::Wait(duration) => {
                    debug!(
                        "Rate limit: transfer {:?} waiting {:?} for {} bytes",
                        request.id, duration, transfer_bytes
                    );
                    tokio::time::sleep(duration).await;
                    // Record the transfer after waiting
                    limiter
                        .record(Some(&client_id), device_id, transfer_bytes)
                        .await;
                }
            }
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

    /// Get the data size of a transfer for rate limiting
    fn get_transfer_data_size(transfer: &TransferType) -> u64 {
        match transfer {
            TransferType::Control { data, .. } => data.len() as u64,
            TransferType::Interrupt { data, .. } => data.len() as u64,
            TransferType::Bulk { data, .. } => data.len() as u64,
            TransferType::Isochronous { data, .. } => data.len() as u64,
        }
    }

    /// Handle GetSharingStatusRequest
    async fn handle_get_sharing_status(&self, device_id: DeviceId) -> Result<MessagePayload> {
        debug!(
            "Get sharing status request: {:?} from {}",
            device_id, self.endpoint_id
        );

        // Find the handle for this device if attached
        let handle = self
            .attached_devices
            .iter()
            .find(|(_, id)| **id == device_id)
            .map(|(h, _)| *h);

        // Send command to USB subsystem
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.usb_bridge
            .send_command(UsbCommand::GetSharingStatus {
                device_id,
                handle,
                response: tx,
            })
            .await?;

        // Wait for response
        let result = rx.await?;

        Ok(MessagePayload::GetSharingStatusResponse { result })
    }

    /// Handle LockDeviceRequest
    async fn handle_lock_device(
        &self,
        handle: DeviceHandle,
        write_access: bool,
    ) -> Result<MessagePayload> {
        info!(
            "Lock device request: {:?} (write={}) from {}",
            handle, write_access, self.endpoint_id
        );

        // Verify device is attached
        if !self.attached_devices.contains_key(&handle) {
            return Ok(MessagePayload::LockDeviceResponse {
                result: protocol::LockResult::NotAvailable {
                    reason: "Device not attached".to_string(),
                },
            });
        }

        // Send command to USB subsystem
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.usb_bridge
            .send_command(UsbCommand::LockDevice {
                handle,
                write_access,
                response: tx,
            })
            .await?;

        // Wait for response
        let result = rx.await?;

        Ok(MessagePayload::LockDeviceResponse { result })
    }

    /// Handle UnlockDeviceRequest
    async fn handle_unlock_device(&self, handle: DeviceHandle) -> Result<MessagePayload> {
        info!(
            "Unlock device request: {:?} from {}",
            handle, self.endpoint_id
        );

        // Verify device is attached
        if !self.attached_devices.contains_key(&handle) {
            return Ok(MessagePayload::UnlockDeviceResponse {
                result: protocol::UnlockResult::NotHeld,
            });
        }

        // Send command to USB subsystem
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.usb_bridge
            .send_command(UsbCommand::UnlockDevice {
                handle,
                response: tx,
            })
            .await?;

        // Wait for response
        let result = rx.await?;

        Ok(MessagePayload::UnlockDeviceResponse { result })
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

    /// Flush aggregated notifications to the client
    async fn flush_aggregated_notifications(&mut self) -> Result<()> {
        if let Some(notifications) = self.notification_aggregator.flush() {
            if notifications.is_empty() {
                return Ok(());
            }

            debug!(
                "Flushing {} aggregated notifications to client",
                notifications.len()
            );

            self.send_push_notification(MessagePayload::AggregatedNotifications { notifications })
                .await
        } else {
            Ok(())
        }
    }

    /// Queue a device notification through the aggregator
    fn queue_device_notification(&mut self, notification: PendingNotification) {
        let device_id = notification.device_id();
        debug!("Queuing notification for device {:?}", device_id);
        self.notification_aggregator.add(notification);
    }

    /// Handle USB events from the USB subsystem
    async fn handle_usb_event(&mut self, event: UsbEvent) -> Result<()> {
        match event {
            UsbEvent::DeviceArrived { device } => {
                debug!("Device arrived: {:?}", device.id);
                self.queue_device_notification(PendingNotification::Arrived(device));
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
                let cancelled_count = self.cancel_pending_transfers(&invalidated_handles).await;
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

                // Queue aggregated notification
                self.queue_device_notification(PendingNotification::Removed {
                    device_id,
                    invalidated_handles,
                    reason: DeviceRemovalReason::Unplugged,
                });
            }

            UsbEvent::DeviceAvailable {
                device_id,
                handle,
                client_id,
                sharing_mode,
            } => {
                // Only notify if this is our client
                if client_id == self.endpoint_id.to_string() {
                    info!(
                        "Device {:?} became available for client {} with handle {:?}",
                        device_id, client_id, handle
                    );

                    if let Err(e) = self
                        .send_push_notification(MessagePayload::DeviceAvailableNotification {
                            device_id,
                            handle,
                            sharing_mode,
                        })
                        .await
                    {
                        warn!("Failed to send device available notification: {:#}", e);
                    }
                }
            }

            UsbEvent::QueuePositionChanged {
                device_id,
                handle: _,
                client_id,
                new_position,
            } => {
                // Only notify if this is our client
                if client_id == self.endpoint_id.to_string() {
                    debug!(
                        "Queue position changed for client {} on device {:?}: position={}",
                        client_id, device_id, new_position
                    );

                    if let Err(e) = self
                        .send_push_notification(MessagePayload::QueuePositionNotification {
                            update: protocol::QueuePositionUpdate {
                                device_id,
                                position: new_position,
                                queue_length: 0, // Unknown at notification time
                            },
                        })
                        .await
                    {
                        warn!("Failed to send queue position notification: {:#}", e);
                    }
                }
            }

            UsbEvent::LockExpired {
                device_id,
                handle,
                client_id,
            } => {
                // Only notify if this is our client
                if client_id == self.endpoint_id.to_string() {
                    info!(
                        "Lock expired for client {} on device {:?} (handle {:?})",
                        client_id, device_id, handle
                    );

                    // Queue as status change notification through aggregator
                    self.queue_device_notification(PendingNotification::StatusChanged {
                        device_id,
                        device_info: None,
                        reason: protocol::DeviceStatusChangeReason::SharingStatusChanged {
                            shared: true,
                        },
                    });
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

    /// Handle expired sessions for this client
    ///
    /// Checks all attached devices for session expiration and force-detaches
    /// any that have exceeded their time limits or are outside time windows.
    async fn handle_expired_sessions(&mut self) -> Result<()> {
        // Check all expired sessions - the policy engine tracks them
        let expired = self.policy_engine.check_expired_sessions().await;

        if expired.is_empty() {
            return Ok(());
        }

        // Process expired sessions
        for event in expired {
                // Only handle events for this client
                if event.client_id != self.endpoint_id {
                    continue;
                }

                // Only handle events for handles we're tracking
                if !self.attached_devices.contains_key(&event.handle) {
                    continue;
                }

                info!(
                    "Session expired for device {:?} (handle {:?}): {:?}",
                    event.device_id, event.handle, event.reason
                );

                // Force detach the device
                let device_id = self.attached_devices.remove(&event.handle);

                // Send detach command to USB subsystem
                let (tx, rx) = tokio::sync::oneshot::channel();
                if let Err(e) = self
                    .usb_bridge
                    .send_command(UsbCommand::DetachDevice {
                        handle: event.handle,
                        response: tx,
                    })
                    .await
                {
                    warn!("Failed to send detach command for expired session: {:#}", e);
                    continue;
                }

                // Wait for response (don't block too long)
                match tokio::time::timeout(Duration::from_secs(5), rx).await {
                    Ok(Ok(Ok(()))) => {
                        info!("Force-detached expired device {:?}", event.handle);
                    }
                    Ok(Ok(Err(e))) => {
                        warn!("Force-detach failed: {:?}", e);
                    }
                    Ok(Err(_)) => {
                        warn!("Force-detach response channel closed");
                    }
                    Err(_) => {
                        warn!("Force-detach response timeout");
                    }
                }

                // Unregister from policy engine
                self.policy_engine.unregister_session(event.handle).await;

                // Convert reason to ForceDetachReason
                let reason = match event.reason {
                    crate::policy::SessionExpiredReason::DurationLimitReached => {
                        ForceDetachReason::SessionDurationLimitReached {
                            duration_secs: 0, // Session duration not tracked in event
                            max_duration_secs: 0, // Max duration not tracked in event
                        }
                    }
                    crate::policy::SessionExpiredReason::TimeWindowExpired => {
                        ForceDetachReason::TimeWindowExpired {
                            current_time: "expired".to_string(),
                            next_window: None, // Next window not tracked in event
                        }
                    }
                };

                // Send notification to client
                if self.client_supports_push {
                    let notification = MessagePayload::ForcedDetachNotification {
                        handle: event.handle,
                        device_id: event.device_id,
                        reason,
                    };

                    if let Err(e) = self.send_push_notification(notification).await {
                        warn!("Failed to send force-detach notification: {:#}", e);
                    }
                }

                // Audit log
                let endpoint_id_str = self.endpoint_id.to_string();
                if let Some(ref logger) = *self.audit_logger {
                    logger.log_device_detach(
                        &endpoint_id_str,
                        event.handle,
                        device_id,
                        AuditResult::Success,
                        Some(format!("Session expired: {:?}", event.reason)),
                    );
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

            // Ensure session is removed from policy engine to prevent memory leak
            self.policy_engine.unregister_session(handle).await;
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
