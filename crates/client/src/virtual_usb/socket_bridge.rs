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

use super::usbip_protocol::{
    UsbIpCmdSubmit, UsbIpCmdUnlink, UsbIpCommand, UsbIpHeader, UsbIpIsoPacketDescriptor,
    UsbIpMessage, UsbIpRetSubmit, UsbIpRetUnlink, usb_response_to_usbip_full, usbip_to_usb_request,
};
use crate::network::device_proxy::DeviceProxy;
use anyhow::{Context, Result, anyhow};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{RwLock, oneshot};
use tracing::{debug, error, info, trace};

/// Socket bridge for USB/IP protocol
///
/// Bridges vhci_hcd kernel driver (via Unix socketpair) to DeviceProxy (via QUIC)
pub struct SocketBridge {
    /// Device proxy for communicating with remote USB device
    device_proxy: Arc<DeviceProxy>,
    /// Unix socket (our end of socketpair) connected to vhci_hcd
    /// Using std::sync::Mutex for synchronous blocking I/O
    socket: Arc<std::sync::Mutex<UnixStream>>,
    /// vhci FD stream (kept alive to prevent socketpair from closing)
    _vhci_stream: UnixStream,
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
    /// Note: The socket should be clean - no import handshake is needed.
    /// vhci_hcd receives device info via sysfs attach parameters (devid, speed, etc.),
    /// and immediately starts CMD_SUBMIT/RET_SUBMIT communication on the socket.
    pub async fn new(
        device_proxy: Arc<DeviceProxy>,
        devid: u32,
        port: u8,
    ) -> Result<(Self, RawFd)> {
        // 1. Create Unix socketpair (we control both ends)
        let (vhci_stream, bridge_stream) =
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

        // 3. Keep bridge_stream in BLOCKING mode for synchronous I/O
        // The socket bridge will run in a blocking thread (spawn_blocking)
        // This avoids potential issues with tokio's async FD handling

        // 4. Keep BOTH socketpair ends alive for ongoing communication
        // vhci_stream is stored to prevent socketpair from closing when kernel closes its dup'd FD
        let bridge = Self {
            device_proxy,
            socket: Arc::new(std::sync::Mutex::new(bridge_stream)),
            _vhci_stream: vhci_stream,
            devid,
            port,
            running: Arc::new(AtomicBool::new(true)),
            pending_transfers: Arc::new(RwLock::new(HashMap::new())),
        };

        debug!(
            "Created Unix socketpair bridge: devid={}, port={}, vhci_fd={} (ready for CMD_SUBMIT)",
            devid, port, vhci_fd
        );

        Ok((bridge, vhci_fd))
    }

    /// Start the bridge task
    ///
    /// This spawns a blocking thread that handles USB/IP protocol translation.
    /// Using spawn_blocking ensures the blocking socket I/O doesn't starve
    /// the tokio runtime.
    pub fn start(self: Arc<Self>) {
        let bridge = self.clone();
        let devid = self.devid;
        let port = self.port;

        // Capture runtime handle BEFORE spawn_blocking
        // This ensures we have access to the tokio runtime from within the blocking thread
        let rt = tokio::runtime::Handle::current();

        info!(
            "Spawning socket bridge thread for device {} port {}",
            devid, port
        );

        // Use spawn_blocking for synchronous socket I/O
        tokio::task::spawn_blocking(move || {
            // Use eprintln for immediate output (not buffered through tracing)
            eprintln!(
                "[SocketBridge] spawn_blocking closure entered for device {} port {}",
                devid, port
            );
            info!(
                "Socket bridge thread started for device {} port {}",
                devid, port
            );

            if let Err(e) = bridge.run_blocking(&rt) {
                error!("Socket bridge error: {:#}", e);
                eprintln!("[SocketBridge] Error: {:#}", e);
            }

            eprintln!(
                "[SocketBridge] spawn_blocking closure exiting for device {} port {}",
                devid, port
            );
        });

        info!(
            "spawn_blocking task scheduled for device {} port {}",
            devid, port
        );
    }

    /// Stop the bridge
    pub fn stop(&self) {
        self.running.store(false, Ordering::Release);
    }

    /// Main bridge loop (blocking version)
    ///
    /// Runs in a spawn_blocking thread with synchronous socket I/O.
    /// Uses the tokio runtime handle to call async DeviceProxy methods.
    fn run_blocking(&self, rt: &tokio::runtime::Handle) -> Result<()> {
        info!(
            "Starting USB/IP socket bridge for device {} on port {}",
            self.devid, self.port
        );

        info!(
            "Socket bridge ready for device {} on port {}, entering main loop",
            self.devid, self.port
        );

        // Enter the main loop for CMD_SUBMIT/RET_SUBMIT
        while self.running.load(Ordering::Acquire) {
            // Read USB/IP message from vhci_hcd (blocking I/O)
            let message = match self.read_usbip_message_blocking() {
                Ok(msg) => msg,
                Err(e) => {
                    // Check if connection was closed gracefully
                    let err_str = e.to_string();
                    if err_str.contains("unexpected end of file")
                        || err_str.contains("end of file")
                        || err_str.contains("early eof")
                    {
                        info!("vhci_hcd closed connection for port {}", self.port);
                        break;
                    }
                    error!("Failed to read USB/IP message: {:#}", e);
                    // On error, sleep briefly to avoid tight loop
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
            };

            // Handle the message based on type
            match message {
                UsbIpMessage::Submit { header, cmd, data } => {
                    eprintln!(
                        "[SocketBridge] Received CMD_SUBMIT: seqnum={}, ep={}, direction={}, transfer_len={}, interval={}, setup={:02x?}",
                        header.seqnum,
                        header.ep,
                        header.direction,
                        cmd.transfer_buffer_length,
                        cmd.interval,
                        cmd.setup
                    );
                    debug!(
                        "Received CMD_SUBMIT: seqnum={}, ep={}, direction={}",
                        header.seqnum, header.ep, header.direction
                    );
                    match self.handle_cmd_submit_blocking(rt, header, cmd, data) {
                        Ok(()) => {
                            eprintln!("[SocketBridge] CMD_SUBMIT handled successfully");
                        }
                        Err(e) => {
                            eprintln!("[SocketBridge] CMD_SUBMIT error: {:#}", e);
                            error!("Failed to handle CMD_SUBMIT: {:#}", e);
                        }
                    }
                }
                UsbIpMessage::Unlink { header, cmd } => {
                    trace!(
                        "Received CMD_UNLINK: seqnum={}, seqnum_unlink={}",
                        header.seqnum, cmd.seqnum_unlink
                    );
                    if let Err(e) = self.handle_cmd_unlink_blocking(rt, header, cmd) {
                        error!("Failed to handle CMD_UNLINK: {:#}", e);
                    }
                }
            }
        }

        info!("Socket bridge stopped for port {}", self.port);
        Ok(())
    }

    /// Read a USB/IP message from the socket (blocking version)
    ///
    /// Returns a parsed UsbIpMessage (either Submit or Unlink)
    fn read_usbip_message_blocking(&self) -> Result<UsbIpMessage> {
        let mut socket = self
            .socket
            .lock()
            .map_err(|e| anyhow!("Failed to lock socket: {}", e))?;

        eprintln!(
            "[SocketBridge] Waiting to read from socket for device {} port {} (blocking)",
            self.devid, self.port
        );
        debug!(
            "Attempting to read from socket for device {} port {}",
            self.devid, self.port
        );

        // Set a read timeout so we can detect if kernel stops sending
        socket.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
        eprintln!("[SocketBridge] Set 10-second read timeout");

        // Read header (20 bytes)
        let mut header_buf = vec![0u8; UsbIpHeader::SIZE];
        eprintln!(
            "[SocketBridge] Calling read_exact for {} bytes...",
            UsbIpHeader::SIZE
        );
        match socket.read_exact(&mut header_buf) {
            Ok(()) => {
                eprintln!(
                    "[SocketBridge] Successfully read {} bytes for device {} port {}",
                    header_buf.len(),
                    self.devid,
                    self.port
                );
                debug!(
                    "Successfully read {} bytes for device {} port {}",
                    header_buf.len(),
                    self.devid,
                    self.port
                );
            }
            Err(e) => {
                eprintln!(
                    "[SocketBridge] Read failed for device {} port {}: kind={:?}, error={}",
                    self.devid,
                    self.port,
                    e.kind(),
                    e
                );
                debug!(
                    "Read failed for device {} port {}: kind={:?}, error={}",
                    self.devid,
                    self.port,
                    e.kind(),
                    e
                );
                return Err(anyhow!("Failed to read USB/IP header: {}", e));
            }
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
                    .context("Failed to read CMD_SUBMIT payload")?;

                let mut cursor = std::io::Cursor::new(&cmd_buf);
                let cmd = UsbIpCmdSubmit::read_from(&mut cursor)?;

                // Read data if OUT transfer (direction = 0)
                let mut data = Vec::new();
                if header.direction == 0 && cmd.transfer_buffer_length > 0 {
                    data.resize(cmd.transfer_buffer_length as usize, 0);
                    socket
                        .read_exact(&mut data)
                        .context("Failed to read transfer data")?;
                }

                Ok(UsbIpMessage::Submit { header, cmd, data })
            }
            UsbIpCommand::CmdUnlink => {
                // USB/IP header union is always 28 bytes (size of largest member: cmd_submit)
                // CMD_UNLINK only uses first 4 bytes (seqnum_unlink), rest is padding
                // We must read all 28 bytes to stay in sync with the protocol
                const UNION_SIZE: usize = 28;
                let mut unlink_buf = vec![0u8; UNION_SIZE];
                socket
                    .read_exact(&mut unlink_buf)
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

    /// Handle CMD_SUBMIT by forwarding to DeviceProxy (blocking version)
    ///
    /// Uses the tokio runtime handle to call async DeviceProxy methods.
    fn handle_cmd_submit_blocking(
        &self,
        rt: &tokio::runtime::Handle,
        header: UsbIpHeader,
        cmd: UsbIpCmdSubmit,
        data: Vec<u8>,
    ) -> Result<()> {
        let seqnum = header.seqnum;
        let max_data_len = cmd.transfer_buffer_length as usize;

        // Create cancellation channel for this transfer
        let (cancel_tx, _cancel_rx) = oneshot::channel::<()>();

        // Register this transfer as pending (for CMD_UNLINK support)
        rt.block_on(async {
            let mut pending = self.pending_transfers.write().await;
            pending.insert(seqnum, cancel_tx);
            trace!(
                "Registered pending transfer: seqnum={}, total_pending={}",
                seqnum,
                pending.len()
            );
        });

        // Convert USB/IP to our protocol using tokio runtime
        let usb_request = rt.block_on(async {
            usbip_to_usb_request(&self.device_proxy, &header, &cmd, data).await
        })?;

        trace!(
            "Submitting USB request: seqnum={}, id={}",
            seqnum, usb_request.id.0
        );

        // Submit to device proxy (blocking on async call)
        let result = rt.block_on(async { self.device_proxy.submit_transfer(usb_request).await });

        // Remove from pending transfers
        rt.block_on(async {
            let mut pending = self.pending_transfers.write().await;
            pending.remove(&seqnum);
            trace!(
                "Removed pending transfer: seqnum={}, remaining={}",
                seqnum,
                pending.len()
            );
        });

        // Handle transfer result
        let usb_response = result.context("Failed to submit transfer to device proxy")?;

        // Convert response back to USB/IP (with full ISO support)
        let mut converted = usb_response_to_usbip_full(&usb_response);

        // IMPORTANT: Clamp response data to the kernel's requested buffer size
        // The kernel allocates exactly transfer_buffer_length bytes for IN transfers.
        // If we return more data than requested, we'll corrupt memory or cause protocol errors.
        // This commonly happens with GET_CONFIG_DESCRIPTOR where kernel asks for 9 bytes
        // (to read wTotalLength) but device returns the full descriptor.
        if header.direction == 1 && converted.data.len() > max_data_len {
            eprintln!(
                "[SocketBridge] Clamping response data from {} to {} bytes (kernel buffer size)",
                converted.data.len(),
                max_data_len
            );
            converted.data.truncate(max_data_len);
            converted.ret.actual_length = max_data_len as u32;
        }

        trace!(
            "Completed USB request: seqnum={}, status={}, len={}, iso_packets={}",
            seqnum,
            converted.ret.status,
            converted.ret.actual_length,
            converted.iso_packets.len()
        );

        // Send RET_SUBMIT back to vhci_hcd (blocking)
        self.send_ret_submit_blocking(
            &header,
            converted.ret,
            converted.data,
            converted.iso_packets,
        )?;

        Ok(())
    }

    /// Send RET_SUBMIT back to vhci_hcd (blocking version)
    ///
    /// For isochronous transfers, the response includes ISO packet descriptors
    /// between the header and the data payload. The USB/IP wire format is:
    /// - Header (48 bytes: 20 basic + 28 payload)
    /// - ISO packet descriptors (16 bytes each, if number_of_packets > 0)
    /// - Transfer data (if any)
    fn send_ret_submit_blocking(
        &self,
        request_header: &UsbIpHeader,
        ret: UsbIpRetSubmit,
        data: Vec<u8>,
        iso_packets: Vec<UsbIpIsoPacketDescriptor>,
    ) -> Result<()> {
        let mut socket = self
            .socket
            .lock()
            .map_err(|e| anyhow!("Failed to lock socket: {}", e))?;

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

        eprintln!(
            "[SocketBridge] Sending RET_SUBMIT: seqnum={}, devid={}, status={}, actual_length={}, data_len={}, iso_packets={}",
            request_header.seqnum,
            request_header.devid,
            ret.status,
            ret.actual_length,
            data.len(),
            iso_packets.len()
        );
        debug!(
            "Writing RET_SUBMIT header: command={:#06x}, seqnum={}, devid={}, direction={}, ep={}, header_bytes={:02x?}",
            header.command,
            header.seqnum,
            header.devid,
            header.direction,
            header.ep,
            &header_buf[..16.min(header_buf.len())]
        );

        socket.write_all(&header_buf)?;

        // Write RET_SUBMIT payload
        let mut ret_buf = Vec::new();
        ret.write_to(&mut ret_buf)?;

        debug!(
            "Writing RET_SUBMIT payload: status={}, actual_length={}, start_frame={}, number_of_packets={}, error_count={}, ret_bytes={:02x?}",
            ret.status,
            ret.actual_length,
            ret.start_frame,
            ret.number_of_packets,
            ret.error_count,
            &ret_buf[..20.min(ret_buf.len())]
        );

        socket.write_all(&ret_buf)?;

        // USB/IP header is always 48 bytes (20 header + 28 union payload)
        // RET_SUBMIT payload is only 20 bytes, so we need 8 bytes of padding
        // to match the kernel's expected header size
        const RET_SUBMIT_PADDING: usize = 8;
        socket.write_all(&[0u8; RET_SUBMIT_PADDING])?;

        // Write ISO packet descriptors if this is an isochronous transfer
        // Per USB/IP protocol, ISO descriptors come after the header but before data
        let mut iso_buf = Vec::new();
        for iso_packet in &iso_packets {
            iso_packet.write_to(&mut iso_buf)?;
        }
        if !iso_buf.is_empty() {
            debug!(
                "Writing {} ISO packet descriptors ({} bytes)",
                iso_packets.len(),
                iso_buf.len()
            );
            socket.write_all(&iso_buf)?;
        }

        // Write response data if any
        if !data.is_empty() {
            debug!("Writing RET_SUBMIT data: {} bytes", data.len());
            socket.write_all(&data)?;
        }

        socket.flush()?;

        let total_len =
            header_buf.len() + ret_buf.len() + RET_SUBMIT_PADDING + iso_buf.len() + data.len();
        eprintln!(
            "[SocketBridge] RET_SUBMIT sent and flushed for seqnum={}, total_bytes={} (header={}, ret={}, padding={}, iso={}, data={})",
            request_header.seqnum,
            total_len,
            header_buf.len(),
            ret_buf.len(),
            RET_SUBMIT_PADDING,
            iso_buf.len(),
            data.len()
        );
        eprintln!("[SocketBridge] RET_SUBMIT header hex: {:02x?}", &header_buf);
        eprintln!("[SocketBridge] RET_SUBMIT payload hex: {:02x?}", &ret_buf);
        if !iso_buf.is_empty() {
            eprintln!(
                "[SocketBridge] RET_SUBMIT ISO packets hex: {:02x?}",
                &iso_buf[..iso_buf.len().min(64)]
            );
        }
        if !data.is_empty() {
            eprintln!(
                "[SocketBridge] RET_SUBMIT data hex: {:02x?}",
                &data[..data.len().min(64)]
            );
        }

        Ok(())
    }

    /// Handle CMD_UNLINK by cancelling a pending transfer (blocking version)
    ///
    /// This method looks up the pending transfer by seqnum_unlink and cancels it
    /// if still in progress. Per USB/IP protocol:
    /// - If transfer is found and cancelled: return status 0 (success)
    /// - If transfer already completed: return status -ENOENT (-2)
    fn handle_cmd_unlink_blocking(
        &self,
        rt: &tokio::runtime::Handle,
        header: UsbIpHeader,
        cmd: UsbIpCmdUnlink,
    ) -> Result<()> {
        let seqnum_unlink = cmd.seqnum_unlink;

        trace!(
            "Processing CMD_UNLINK: seqnum={}, seqnum_unlink={}",
            header.seqnum, seqnum_unlink
        );

        // Try to find and cancel the pending transfer
        let cancel_tx = rt.block_on(async {
            let mut pending = self.pending_transfers.write().await;
            pending.remove(&seqnum_unlink)
        });

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

        // Send RET_UNLINK response (blocking)
        self.send_ret_unlink_blocking(header.seqnum, status)?;

        trace!(
            "Sent RET_UNLINK: seqnum={}, status={}",
            header.seqnum, status
        );

        Ok(())
    }

    /// Send RET_UNLINK back to vhci_hcd (blocking version)
    ///
    /// Per USB/IP protocol, RET_UNLINK consists of:
    /// - Header (20 bytes): command=0x0004, seqnum, devid, direction=0, ep=0
    /// - Payload (4 bytes): status (i32)
    /// - Padding to match kernel struct alignment (16 bytes)
    fn send_ret_unlink_blocking(&self, seqnum: u32, status: i32) -> Result<()> {
        let mut socket = self
            .socket
            .lock()
            .map_err(|e| anyhow!("Failed to lock socket: {}", e))?;

        // Write header (20 bytes)
        let header = UsbIpHeader::new(UsbIpCommand::RetUnlink, seqnum, self.devid);
        let mut header_buf = Vec::new();
        header.write_to(&mut header_buf)?;

        debug!(
            "Writing RET_UNLINK header: command={:#06x}, seqnum={}, devid={}",
            header.command, header.seqnum, header.devid
        );

        socket.write_all(&header_buf)?;

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

        socket.write_all(&ret_buf)?;

        // USB/IP header union is always 28 bytes (size of largest member: cmd_submit)
        // RET_UNLINK only uses 4 bytes (status), rest must be padding
        // We wrote 4 bytes for status, so pad with 24 bytes to reach 28
        const RET_UNLINK_PADDING: usize = 24;
        socket.write_all(&[0u8; RET_UNLINK_PADDING])?;

        socket.flush()?;

        Ok(())
    }
}

impl Drop for SocketBridge {
    fn drop(&mut self) {
        self.stop();
        debug!("Socket bridge dropped for port {}", self.port);
    }
}
