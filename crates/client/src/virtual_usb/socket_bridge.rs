//! Socket bridge between vhci_hcd and USB device proxy
//!
//! This module creates a Unix socketpair bridge and handles USB/IP protocol
//! messages between the vhci_hcd kernel driver and our DeviceProxy over QUIC.
//!
//! # Architecture
//!
//! vhci_hcd (Virtual Host Controller Interface) is a Linux kernel module that
//! provides a virtual USB host controller. It expects a socket FD through which
//! it communicates using the USB/IP wire protocol.
//!
//! ## Connection Flow
//!
//! 1. Create Unix socketpair (we control both ends)
//! 2. Pass vhci_fd to vhci_hcd via sysfs attach
//! 3. Keep bridge_fd alive for ongoing CMD_SUBMIT/RET_SUBMIT communication
//! 4. Kernel sends CMD_SUBMIT for USB transfers, we respond with RET_SUBMIT
//! 5. Kernel sends CMD_UNLINK to cancel pending transfers, we respond with RET_UNLINK
//!
//! ## Message Types
//!
//! - `CMD_SUBMIT` (0x0001): Kernel submits a USB request (URB)
//! - `RET_SUBMIT` (0x0003): We return the result of a USB request
//! - `CMD_UNLINK` (0x0002): Kernel requests cancellation of a pending transfer
//! - `RET_UNLINK` (0x0004): We acknowledge the cancellation request
//!
//! ## Wire Protocol
//!
//! All messages have a 20-byte header (UsbIpHeader) followed by command-specific
//! payload. All integers are big-endian (network byte order).
//!
//! See `usbip_protocol.rs` for detailed message format documentation.

use super::usbip_protocol::*;
use crate::network::device_proxy::DeviceProxy;
use anyhow::{Context, Result, anyhow};
use std::collections::HashMap;
use std::io::Write as StdWrite;
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream as TokioUnixStream;
use tokio::sync::{Mutex, RwLock, oneshot};
use tracing::{debug, error, info, trace};

/// Socket bridge for USB/IP protocol
///
/// Bridges vhci_hcd kernel driver (via Unix socketpair) to DeviceProxy (via QUIC)
pub struct SocketBridge {
    /// Device proxy for communicating with remote USB device
    device_proxy: Arc<DeviceProxy>,
    /// Unix socket (our end of socketpair) connected to vhci_hcd
    socket: Arc<Mutex<TokioUnixStream>>,
    /// vhci FD stream (kept alive to prevent socketpair from closing)
    _vhci_stream: std::os::unix::net::UnixStream,
    /// Device ID for USB/IP protocol
    devid: u32,
    /// Port number on vhci_hcd
    port: u8,
    /// Running flag
    running: Arc<AtomicBool>,
    /// Pending transfers tracker for CMD_UNLINK cancellation support
    /// Maps seqnum -> cancellation sender (sending () triggers cancellation)
    pending_transfers: Arc<RwLock<HashMap<u32, oneshot::Sender<()>>>>,
}

impl SocketBridge {
    /// Create a new socketpair-based socket bridge
    ///
    /// Returns (SocketBridge, raw_fd_for_vhci)
    /// The raw FD should be passed to vhci_hcd via sysfs attach
    ///
    /// IMPORTANT: The USB/IP import handshake (OP_REP_IMPORT) MUST be written to the socket
    /// BEFORE passing the FD to the kernel. The kernel reads device info from the socket
    /// when attaching.
    pub async fn new(
        device_proxy: Arc<DeviceProxy>,
        devid: u32,
        port: u8,
    ) -> Result<(Self, RawFd)> {
        // 1. Create Unix socketpair (we control both ends)
        let (vhci_stream, mut bridge_stream) =
            UnixStream::pair().context("Failed to create Unix socketpair")?;

        debug!(
            "Created Unix socketpair for device {} port {}: vhci_fd={}, bridge_fd={}",
            devid,
            port,
            vhci_stream.as_raw_fd(),
            bridge_stream.as_raw_fd()
        );

        // 2. Extract vhci FD before any operations
        let vhci_fd = vhci_stream.as_raw_fd();

        // 3. Write OP_REP_IMPORT to the bridge socket BEFORE kernel attachment
        // The kernel will read this data when the socket is attached to vhci_hcd
        let device_info = device_proxy.device_info();
        let busid = format!("{}-{}", device_info.bus_number, device_info.device_address);
        let import_reply = UsbIpRepImport::from_device_info(device_info, &busid);

        let mut import_buf = Vec::new();
        import_reply
            .write_to(&mut import_buf)
            .context("Failed to serialize OP_REP_IMPORT")?;

        debug!(
            "Writing OP_REP_IMPORT ({} bytes) for device {} (busid: {})",
            import_buf.len(),
            devid,
            busid
        );

        bridge_stream
            .write_all(&import_buf)
            .context("Failed to write OP_REP_IMPORT to socket")?;
        bridge_stream
            .flush()
            .context("Failed to flush OP_REP_IMPORT")?;

        info!(
            "Sent USB/IP import reply for device {} (VID:{:04x} PID:{:04x})",
            devid, device_info.vendor_id, device_info.product_id
        );

        // 4. Convert bridge_stream to tokio for async operations
        // We need to set non-blocking mode first
        bridge_stream
            .set_nonblocking(true)
            .context("Failed to set bridge_stream to non-blocking")?;

        let bridge_stream_tokio = TokioUnixStream::from_std(bridge_stream)
            .context("Failed to convert bridge_stream to tokio")?;

        // 5. Keep BOTH socketpair ends alive for ongoing communication
        // vhci_stream is stored to prevent socketpair from closing when kernel closes its dup'd FD
        let bridge = Self {
            device_proxy,
            socket: Arc::new(Mutex::new(bridge_stream_tokio)),
            _vhci_stream: vhci_stream,
            devid,
            port,
            running: Arc::new(AtomicBool::new(true)),
            pending_transfers: Arc::new(RwLock::new(HashMap::new())),
        };

        debug!(
            "Created Unix socketpair bridge: devid={}, port={}, vhci_fd={} (import handshake complete)",
            devid, port, vhci_fd
        );

        Ok((bridge, vhci_fd))
    }

    /// Start the bridge task
    ///
    /// This spawns a tokio task that handles USB/IP protocol translation
    pub fn start(self: Arc<Self>) {
        let bridge = self.clone();
        tokio::spawn(async move {
            if let Err(e) = bridge.run().await {
                error!("Socket bridge error: {:#}", e);
            }
        });
    }

    /// Stop the bridge
    pub fn stop(&self) {
        self.running.store(false, Ordering::Release);
    }

    /// Main bridge loop
    async fn run(&self) -> Result<()> {
        info!(
            "Starting USB/IP socket bridge for device {} on port {}",
            self.devid, self.port
        );

        // The kernel expects the socket to be ready for CMD_SUBMIT/RET_SUBMIT after attach.
        // The OP_REP_IMPORT handshake was already completed in SocketBridge::new() before
        // the socket FD was passed to vhci_hcd.

        info!(
            "Socket bridge ready for device {} on port {}, entering main loop",
            self.devid, self.port
        );

        // Enter the main loop for CMD_SUBMIT/RET_SUBMIT
        while self.running.load(Ordering::Acquire) {
            // Read USB/IP message from vhci_hcd
            let message = match self.read_usbip_message().await {
                Ok(msg) => msg,
                Err(e) => {
                    // Check if connection was closed gracefully
                    if e.to_string().contains("unexpected end of file") {
                        info!("vhci_hcd closed connection for port {}", self.port);
                        break;
                    }
                    error!("Failed to read USB/IP message: {:#}", e);
                    continue;
                }
            };

            // Handle the message based on type
            match message {
                UsbIpMessage::Submit { header, cmd, data } => {
                    trace!(
                        "Received CMD_SUBMIT: seqnum={}, ep={}, direction={}",
                        header.seqnum, header.ep, header.direction
                    );
                    self.handle_cmd_submit(header, cmd, data).await?;
                }
                UsbIpMessage::Unlink { header, cmd } => {
                    trace!(
                        "Received CMD_UNLINK: seqnum={}, seqnum_unlink={}",
                        header.seqnum, cmd.seqnum_unlink
                    );
                    self.handle_cmd_unlink(header, cmd).await?;
                }
            }
        }

        info!("Socket bridge stopped for port {}", self.port);
        Ok(())
    }

    /// Read a USB/IP message from the socket
    ///
    /// Returns a parsed UsbIpMessage (either Submit or Unlink)
    async fn read_usbip_message(&self) -> Result<UsbIpMessage> {
        let mut socket = self.socket.lock().await;

        // Read header (20 bytes)
        let mut header_buf = vec![0u8; UsbIpHeader::SIZE];
        if let Err(e) = socket.read_exact(&mut header_buf).await {
            error!("Failed to read header: {:#}", e);
            return Err(anyhow::anyhow!("Failed to read USB/IP header: {}", e));
        }

        debug!(
            "Read header bytes: {:02x?} (first 16 bytes)",
            &header_buf[..16.min(header_buf.len())]
        );

        let mut cursor = std::io::Cursor::new(&header_buf);
        let header = UsbIpHeader::read_from(&mut cursor)?;

        debug!(
            "Parsed header: command={:#06x}, seqnum={}, devid={}, direction={}, ep={}",
            header.command, header.seqnum, header.devid, header.direction, header.ep
        );

        // Check command type and read appropriate payload
        let cmd_type = header.command_type()?;

        match cmd_type {
            UsbIpCommand::CmdSubmit => {
                // Read CMD_SUBMIT payload (28 bytes)
                let mut cmd_buf = vec![0u8; UsbIpCmdSubmit::SIZE];
                socket
                    .read_exact(&mut cmd_buf)
                    .await
                    .context("Failed to read CMD_SUBMIT payload")?;

                let mut cursor = std::io::Cursor::new(&cmd_buf);
                let cmd = UsbIpCmdSubmit::read_from(&mut cursor)?;

                // Read data if OUT transfer (direction = 0)
                let mut data = Vec::new();
                if header.direction == 0 && cmd.transfer_buffer_length > 0 {
                    data.resize(cmd.transfer_buffer_length as usize, 0);
                    socket
                        .read_exact(&mut data)
                        .await
                        .context("Failed to read transfer data")?;
                }

                Ok(UsbIpMessage::Submit { header, cmd, data })
            }
            UsbIpCommand::CmdUnlink => {
                // Read CMD_UNLINK payload (4 bytes: seqnum_unlink)
                let mut unlink_buf = vec![0u8; UsbIpCmdUnlink::SIZE];
                socket
                    .read_exact(&mut unlink_buf)
                    .await
                    .context("Failed to read CMD_UNLINK payload")?;

                let mut cursor = std::io::Cursor::new(&unlink_buf);
                let cmd = UsbIpCmdUnlink::read_from(&mut cursor)?;

                trace!(
                    "Parsed CMD_UNLINK: seqnum={}, seqnum_unlink={}",
                    header.seqnum, cmd.seqnum_unlink
                );

                Ok(UsbIpMessage::Unlink { header, cmd })
            }
            _ => Err(anyhow!(
                "Unexpected command type in read_usbip_message: {:?}",
                cmd_type
            )),
        }
    }

    /// Handle CMD_SUBMIT by forwarding to DeviceProxy
    ///
    /// This method tracks the pending transfer for cancellation support via CMD_UNLINK.
    /// A oneshot channel is used to signal cancellation from handle_cmd_unlink().
    async fn handle_cmd_submit(
        &self,
        header: UsbIpHeader,
        cmd: UsbIpCmdSubmit,
        data: Vec<u8>,
    ) -> Result<()> {
        let seqnum = header.seqnum;

        // Create cancellation channel for this transfer
        let (cancel_tx, cancel_rx) = oneshot::channel::<()>();

        // Register this transfer as pending (for CMD_UNLINK support)
        {
            let mut pending = self.pending_transfers.write().await;
            pending.insert(seqnum, cancel_tx);
            trace!(
                "Registered pending transfer: seqnum={}, total_pending={}",
                seqnum,
                pending.len()
            );
        }

        // Convert USB/IP to our protocol
        // Note: DeviceHandle will be obtained from device_proxy
        let usb_request = usbip_to_usb_request(&self.device_proxy, &header, &cmd, data).await?;

        trace!(
            "Submitting USB request: seqnum={}, id={}",
            seqnum, usb_request.id.0
        );

        // Submit to device proxy with cancellation support
        // Race between the actual transfer and the cancellation signal
        let result = tokio::select! {
            // Transfer completed (success or error)
            transfer_result = self.device_proxy.submit_transfer(usb_request) => {
                transfer_result
            }
            // Cancellation requested via CMD_UNLINK
            _ = cancel_rx => {
                trace!("Transfer cancelled via CMD_UNLINK: seqnum={}", seqnum);
                // Return early - the CMD_UNLINK handler already sent RET_UNLINK
                // Remove from pending (already removed by unlink handler, but be safe)
                self.pending_transfers.write().await.remove(&seqnum);
                return Ok(());
            }
        };

        // Remove from pending transfers (completed or errored, not cancelled)
        {
            let mut pending = self.pending_transfers.write().await;
            pending.remove(&seqnum);
            trace!(
                "Removed pending transfer: seqnum={}, remaining={}",
                seqnum,
                pending.len()
            );
        }

        // Handle transfer result
        let usb_response = result.context("Failed to submit transfer to device proxy")?;

        // Convert response back to USB/IP
        let (ret, response_data) = usb_response_to_usbip(&usb_response);

        trace!(
            "Completed USB request: seqnum={}, status={}, len={}",
            seqnum, ret.status, ret.actual_length
        );

        // Send RET_SUBMIT back to vhci_hcd
        self.send_ret_submit(&header, ret, response_data).await?;

        Ok(())
    }

    /// Send RET_SUBMIT back to vhci_hcd
    async fn send_ret_submit(
        &self,
        request_header: &UsbIpHeader,
        ret: UsbIpRetSubmit,
        data: Vec<u8>,
    ) -> Result<()> {
        let mut socket = self.socket.lock().await;

        // Write header - preserve direction and ep from request
        let mut header = UsbIpHeader::new(
            UsbIpCommand::RetSubmit,
            request_header.seqnum,
            request_header.devid,
        );
        header.direction = request_header.direction;
        header.ep = request_header.ep;
        let mut header_buf = Vec::new();
        header.write_to(&mut header_buf)?;

        debug!(
            "Writing RET_SUBMIT header: command={:#06x}, seqnum={}, devid={}, direction={}, ep={}, header_bytes={:02x?}",
            header.command,
            header.seqnum,
            header.devid,
            header.direction,
            header.ep,
            &header_buf[..16.min(header_buf.len())]
        );

        socket.write_all(&header_buf).await?;

        // Write RET_SUBMIT payload
        let mut ret_buf = Vec::new();
        ret.write_to(&mut ret_buf)?;

        debug!(
            "Writing RET_SUBMIT payload: status={}, actual_length={}, ret_bytes={:02x?}",
            ret.status,
            ret.actual_length,
            &ret_buf[..20.min(ret_buf.len())]
        );

        socket.write_all(&ret_buf).await?;

        // Write response data if any
        if !data.is_empty() {
            debug!("Writing RET_SUBMIT data: {} bytes", data.len());
            socket.write_all(&data).await?;
        }

        socket.flush().await?;

        Ok(())
    }

    /// Handle CMD_UNLINK by cancelling a pending transfer
    ///
    /// This method looks up the pending transfer by seqnum_unlink and cancels it
    /// if still in progress. Per USB/IP protocol:
    /// - If transfer is found and cancelled: return status 0 (success)
    /// - If transfer already completed: return status -ENOENT (-2)
    async fn handle_cmd_unlink(&self, header: UsbIpHeader, cmd: UsbIpCmdUnlink) -> Result<()> {
        let seqnum_unlink = cmd.seqnum_unlink;

        trace!(
            "Processing CMD_UNLINK: seqnum={}, seqnum_unlink={}",
            header.seqnum, seqnum_unlink
        );

        // Try to find and cancel the pending transfer
        let cancel_tx = {
            let mut pending = self.pending_transfers.write().await;
            pending.remove(&seqnum_unlink)
        };

        let status = match cancel_tx {
            Some(tx) => {
                // Found the pending transfer - send cancellation signal
                // Note: send() may fail if receiver was already dropped (transfer completed
                // between our lookup and now), which is fine - we still report success
                let _ = tx.send(());
                info!(
                    "Cancelled pending transfer: seqnum_unlink={} (CMD_UNLINK seqnum={})",
                    seqnum_unlink, header.seqnum
                );
                0 // Success: transfer was cancelled
            }
            None => {
                // Transfer not found - already completed
                debug!(
                    "Transfer already completed: seqnum_unlink={} (CMD_UNLINK seqnum={})",
                    seqnum_unlink, header.seqnum
                );
                -2 // -ENOENT: not found (already completed)
            }
        };

        // Send RET_UNLINK response
        self.send_ret_unlink(header.seqnum, status).await?;

        trace!(
            "Sent RET_UNLINK: seqnum={}, status={}",
            header.seqnum, status
        );

        Ok(())
    }

    /// Send RET_UNLINK back to vhci_hcd
    ///
    /// Per USB/IP protocol, RET_UNLINK consists of:
    /// - Header (20 bytes): command=0x0004, seqnum, devid, direction=0, ep=0
    /// - Payload (4 bytes): status (i32)
    /// - Padding to match kernel struct alignment (16 bytes)
    async fn send_ret_unlink(&self, seqnum: u32, status: i32) -> Result<()> {
        let mut socket = self.socket.lock().await;

        // Write header (20 bytes)
        let header = UsbIpHeader::new(UsbIpCommand::RetUnlink, seqnum, self.devid);
        let mut header_buf = Vec::new();
        header.write_to(&mut header_buf)?;

        debug!(
            "Writing RET_UNLINK header: command={:#06x}, seqnum={}, devid={}",
            header.command, header.seqnum, header.devid
        );

        socket.write_all(&header_buf).await?;

        // Write RET_UNLINK payload using the proper struct
        let ret_unlink = UsbIpRetUnlink { status };
        let mut ret_buf = Vec::new();
        ret_unlink.write_to(&mut ret_buf)?;

        debug!(
            "Writing RET_UNLINK payload: status={} ({})",
            status,
            if status == 0 {
                "cancelled"
            } else {
                "not found"
            }
        );

        socket.write_all(&ret_buf).await?;

        // Padding: kernel expects same size as RET_SUBMIT payload (20 bytes total)
        // We wrote 4 bytes for status, so pad with 16 bytes
        socket.write_all(&[0u8; 16]).await?;

        socket.flush().await?;

        Ok(())
    }
}

impl Drop for SocketBridge {
    fn drop(&mut self) {
        self.stop();
        debug!("Socket bridge dropped for port {}", self.port);
    }
}
