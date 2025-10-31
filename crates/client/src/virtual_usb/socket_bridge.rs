//! Socket bridge between vhci_hcd and USB device proxy
//!
//! This module creates a Unix socketpair bridge and handles USB/IP protocol
//! messages between the vhci_hcd kernel driver and our DeviceProxy over QUIC.
//!
//! # Architecture
//!
//! vhci_hcd expects a socket FD with a completed USB/IP import handshake:
//! 1. Create Unix socketpair (we control both ends)
//! 2. Perform USB/IP handshake (OP_REQ_IMPORT / OP_REP_IMPORT) over socketpair
//! 3. Pass vhci_fd to vhci_hcd via sysfs attach (kernel can close this)
//! 4. Keep bridge_fd alive for ongoing CMD_SUBMIT/RET_SUBMIT communication

use super::usbip_protocol::*;
use crate::network::device_proxy::DeviceProxy;
use anyhow::{anyhow, Context, Result};
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream as TokioUnixStream;
use tokio::sync::Mutex;
use tracing::{debug, error, info, trace, warn};

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
}

impl SocketBridge {
    /// Create a new socketpair-based socket bridge
    ///
    /// Returns (SocketBridge, raw_fd_for_vhci)
    /// The raw FD should be passed to vhci_hcd via sysfs attach
    ///
    /// IMPORTANT: The handshake will happen AFTER attach, initiated by the kernel.
    /// Do NOT perform the handshake before passing vhci_fd to the kernel!
    pub async fn new(
        device_proxy: Arc<DeviceProxy>,
        devid: u32,
        port: u8,
    ) -> Result<(Self, RawFd)> {
        // 1. Create Unix socketpair (we control both ends)
        let (vhci_stream, bridge_stream) = UnixStream::pair()
            .context("Failed to create Unix socketpair")?;

        debug!(
            "Created Unix socketpair for device {} port {}: vhci_fd={}, bridge_fd={}",
            devid, port, vhci_stream.as_raw_fd(), bridge_stream.as_raw_fd()
        );

        // 2. Extract vhci FD before converting to tokio
        // The kernel will use this FD for reading device metadata and initiating the handshake
        let vhci_fd = vhci_stream.as_raw_fd();

        // 3. Keep vhci_stream alive (don't leak it!)
        // The kernel will duplicate the FD when we pass it via sysfs,
        // so we need to keep our copy alive to prevent socketpair closure

        // 4. Convert bridge_stream to tokio for async operations
        // We need to set non-blocking mode first
        bridge_stream.set_nonblocking(true)
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
        };

        debug!(
            "Created Unix socketpair bridge: devid={}, port={}, vhci_fd={} (handshake will happen after attach)",
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

        // The kernel expects the socket to be ready for CMD_SUBMIT/RET_SUBMIT immediately
        // There is no handshake - the handshake happens externally before the FD is passed to vhci_hcd

        info!(
            "Socket bridge ready for device {} on port {}, entering main loop",
            self.devid, self.port
        );

        // Enter the main loop for CMD_SUBMIT/RET_SUBMIT
        while self.running.load(Ordering::Acquire) {
            // Read USB/IP message from vhci_hcd
            let (header, cmd, data) = match self.read_usbip_message().await {
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

            trace!(
                "Received USB/IP command: {:?}, seqnum={}",
                header.command_type(),
                header.seqnum
            );

            // Handle the command
            match header.command_type()? {
                UsbIpCommand::CmdSubmit => {
                    self.handle_cmd_submit(header, cmd, data).await?;
                }
                UsbIpCommand::CmdUnlink => {
                    // Unlink not yet implemented (used for cancelling requests)
                    warn!("CMD_UNLINK not yet implemented, ignoring");
                    self.send_ret_unlink(header.seqnum, 0).await?;
                }
                cmd => {
                    warn!("Unexpected command from vhci_hcd: {:?}", cmd);
                }
            }
        }

        info!("Socket bridge stopped for port {}", self.port);
        Ok(())
    }

    /// Read a USB/IP message from the socket
    async fn read_usbip_message(&self) -> Result<(UsbIpHeader, UsbIpCmdSubmit, Vec<u8>)> {
        let mut socket = self.socket.lock().await;

        // Read header (48 bytes)
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
                // Read CMD_SUBMIT payload (40 bytes)
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

                Ok((header, cmd, data))
            }
            UsbIpCommand::CmdUnlink => {
                // CMD_UNLINK has only a 4-byte payload: seqnum_unlink (u32)
                // This is the sequence number of the request to unlink
                let mut unlink_buf = vec![0u8; 4];
                socket
                    .read_exact(&mut unlink_buf)
                    .await
                    .context("Failed to read CMD_UNLINK payload")?;

                // Return empty CMD_SUBMIT (will be ignored by caller)
                let empty_cmd = UsbIpCmdSubmit {
                    transfer_flags: 0,
                    transfer_buffer_length: 0,
                    start_frame: 0,
                    number_of_packets: 0,
                    interval: 0,
                    setup: [0; 8],
                };
                Ok((header, empty_cmd, Vec::new()))
            }
            _ => {
                Err(anyhow!(
                    "Unexpected command type in read_usbip_message: {:?}",
                    cmd_type
                ))
            }
        }
    }

    /// Handle CMD_SUBMIT by forwarding to DeviceProxy
    async fn handle_cmd_submit(
        &self,
        header: UsbIpHeader,
        cmd: UsbIpCmdSubmit,
        data: Vec<u8>,
    ) -> Result<()> {
        // Convert USB/IP to our protocol
        // Note: DeviceHandle will be obtained from device_proxy
        let usb_request = usbip_to_usb_request(&self.device_proxy, &header, &cmd, data).await?;

        trace!(
            "Submitting USB request: seqnum={}, id={}",
            header.seqnum, usb_request.id.0
        );

        // Submit to device proxy (over QUIC to server)
        let usb_response = self
            .device_proxy
            .submit_transfer(usb_request)
            .await
            .context("Failed to submit transfer to device proxy")?;

        // Convert response back to USB/IP
        let (ret, response_data) = usb_response_to_usbip(&usb_response);

        trace!(
            "Completed USB request: seqnum={}, status={}, len={}",
            header.seqnum, ret.status, ret.actual_length
        );

        // Send RET_SUBMIT back to vhci_hcd
        self.send_ret_submit(&header, ret, response_data)
            .await?;

        Ok(())
    }

    /// Send RET_SUBMIT back to vhci_hcd
    async fn send_ret_submit(&self, request_header: &UsbIpHeader, ret: UsbIpRetSubmit, data: Vec<u8>) -> Result<()> {
        let mut socket = self.socket.lock().await;

        // Write header - preserve direction and ep from request
        let mut header = UsbIpHeader::new(UsbIpCommand::RetSubmit, request_header.seqnum, request_header.devid);
        header.direction = request_header.direction;
        header.ep = request_header.ep;
        let mut header_buf = Vec::new();
        header.write_to(&mut header_buf)?;

        debug!(
            "Writing RET_SUBMIT header: command={:#06x}, seqnum={}, devid={}, direction={}, ep={}, header_bytes={:02x?}",
            header.command, header.seqnum, header.devid, header.direction, header.ep,
            &header_buf[..16.min(header_buf.len())]
        );

        socket.write_all(&header_buf).await?;

        // Write RET_SUBMIT payload
        let mut ret_buf = Vec::new();
        ret.write_to(&mut ret_buf)?;

        debug!(
            "Writing RET_SUBMIT payload: status={}, actual_length={}, ret_bytes={:02x?}",
            ret.status, ret.actual_length, &ret_buf[..20.min(ret_buf.len())]
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

    /// Send RET_UNLINK back to vhci_hcd
    async fn send_ret_unlink(&self, seqnum: u32, status: i32) -> Result<()> {
        let mut socket = self.socket.lock().await;

        // Write header
        let header = UsbIpHeader::new(UsbIpCommand::RetUnlink, seqnum, self.devid);
        let mut header_buf = Vec::new();
        header.write_to(&mut header_buf)?;
        socket.write_all(&header_buf).await?;

        // Write status (4 bytes) + padding (44 bytes) = 48 bytes total
        socket.write_i32(status).await?;
        socket.write_all(&[0u8; 44]).await?;

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
