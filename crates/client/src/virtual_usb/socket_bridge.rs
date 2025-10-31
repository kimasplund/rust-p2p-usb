//! Socket bridge between vhci_hcd and USB device proxy
//!
//! This module creates a Unix domain socket pair and bridges USB/IP protocol
//! messages between the vhci_hcd kernel driver and our DeviceProxy over QUIC.

use super::usbip_protocol::*;
use crate::network::device_proxy::DeviceProxy;
use anyhow::{Context, Result};
use std::os::fd::OwnedFd;
use std::os::unix::io::{FromRawFd, RawFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::Mutex;
use tracing::{debug, error, info, trace, warn};

/// Socket bridge for USB/IP protocol
///
/// Bridges vhci_hcd kernel driver (via Unix socket) to DeviceProxy (via QUIC)
pub struct SocketBridge {
    /// Device proxy for communicating with remote USB device
    device_proxy: Arc<DeviceProxy>,
    /// Unix socket connected to vhci_hcd
    socket: Arc<Mutex<UnixStream>>,
    /// Device ID for USB/IP protocol
    devid: u32,
    /// Port number on vhci_hcd
    port: u8,
    /// Running flag
    running: Arc<AtomicBool>,
}

impl SocketBridge {
    /// Create a new socket bridge
    ///
    /// Returns (SocketBridge, raw_fd_for_vhci)
    /// The raw FD should be passed to vhci_hcd via sysfs attach
    pub async fn new(
        device_proxy: Arc<DeviceProxy>,
        devid: u32,
        port: u8,
    ) -> Result<(Self, RawFd)> {
        use nix::sys::socket::{AddressFamily, SockFlag, SockType, socketpair};
        use std::os::fd::{AsRawFd, IntoRawFd};
        use std::os::unix::net::UnixStream;

        // Create Unix domain socket pair
        // One socket for vhci_hcd, one for our bridge
        let (vhci_owned_fd, bridge_owned_fd) = socketpair(
            AddressFamily::Unix,
            SockType::Stream,
            None,
            SockFlag::empty(),
        )
        .context("Failed to create Unix socket pair")?;

        debug!("Created Unix socketpair for devid={}, port={}", devid, port);

        // Get device info for handshake
        let device_info = device_proxy.device_info();

        // Perform USB/IP import handshake BEFORE passing FD to vhci_hcd
        Self::perform_import_handshake(&bridge_owned_fd, device_info, devid, port)
            .await
            .context("Failed to perform USB/IP import handshake")?;

        // Extract vhci raw FD (keep OwnedFd alive by leaking)
        let vhci_fd = vhci_owned_fd.as_raw_fd();

        // Transfer ownership of bridge FD (consumes OwnedFd)
        let bridge_fd = bridge_owned_fd.into_raw_fd();

        // Keep vhci_owned_fd alive by leaking it (kernel will manage it)
        std::mem::forget(vhci_owned_fd);

        // Convert the bridge FD to a std UnixStream (takes ownership of FD)
        let std_stream = unsafe { UnixStream::from_raw_fd(bridge_fd) };

        // Set non-blocking mode for tokio
        std_stream
            .set_nonblocking(true)
            .context("Failed to set socket non-blocking")?;

        // Convert to tokio UnixStream
        let tokio_stream = tokio::net::UnixStream::from_std(std_stream)
            .context("Failed to create tokio UnixStream")?;

        debug!(
            "Created Unix socket bridge: devid={}, port={}, vhci_fd={}",
            devid, port, vhci_fd
        );

        Ok((
            Self {
                device_proxy,
                socket: Arc::new(Mutex::new(tokio_stream)),
                devid,
                port,
                running: Arc::new(AtomicBool::new(true)),
            },
            vhci_fd,
        ))
    }

    /// Perform USB/IP import handshake over socket
    ///
    /// This simulates the OP_REQ_IMPORT / OP_REP_IMPORT exchange that
    /// normally happens between usbip client and server. Since we control
    /// both ends of the socket, we perform the handshake synchronously.
    async fn perform_import_handshake(
        socket_fd: &OwnedFd,
        device_info: &protocol::DeviceInfo,
        devid: u32,
        port: u8,
    ) -> Result<()> {
        use nix::unistd::write;

        debug!(
            "Performing USB/IP import handshake for device {} on port {}",
            devid, port
        );

        // Create bus ID (format: "port-devid")
        let busid = format!("{}-{}", port, devid);

        // Send OP_REQ_IMPORT (client → server)
        let req_import = UsbIpReqImport::new(&busid);
        let mut req_buf = Vec::new();
        req_import
            .write_to(&mut req_buf)
            .context("Failed to serialize OP_REQ_IMPORT")?;

        // Write to socket (this is normally done over TCP)
        let written =
            write(socket_fd, &req_buf).context("Failed to write OP_REQ_IMPORT to socket")?;

        debug!("Sent OP_REQ_IMPORT: {} bytes", written);

        // Send OP_REP_IMPORT (server → client)
        let rep_import = UsbIpRepImport::from_device_info(device_info, &busid);
        let mut rep_buf = Vec::new();
        rep_import
            .write_to(&mut rep_buf)
            .context("Failed to serialize OP_REP_IMPORT")?;

        let written =
            write(socket_fd, &rep_buf).context("Failed to write OP_REP_IMPORT to socket")?;

        debug!("Sent OP_REP_IMPORT: {} bytes", written);

        info!(
            "USB/IP import handshake complete for device {} ({})",
            devid, busid
        );

        Ok(())
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
        socket
            .read_exact(&mut header_buf)
            .await
            .context("Failed to read USB/IP header")?;

        let mut cursor = std::io::Cursor::new(&header_buf);
        let header = UsbIpHeader::read_from(&mut cursor)?;

        // Read CMD_SUBMIT payload (40 bytes)
        let mut cmd_buf = vec![0u8; UsbIpCmdSubmit::SIZE];
        socket
            .read_exact(&mut cmd_buf)
            .await
            .context("Failed to read CMD_SUBMIT")?;

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
        self.send_ret_submit(header.seqnum, ret, response_data)
            .await?;

        Ok(())
    }

    /// Send RET_SUBMIT back to vhci_hcd
    async fn send_ret_submit(&self, seqnum: u32, ret: UsbIpRetSubmit, data: Vec<u8>) -> Result<()> {
        let mut socket = self.socket.lock().await;

        // Write header
        let header = UsbIpHeader::new(UsbIpCommand::RetSubmit, seqnum, self.devid);
        let mut header_buf = Vec::new();
        header.write_to(&mut header_buf)?;
        socket.write_all(&header_buf).await?;

        // Write RET_SUBMIT payload
        let mut ret_buf = Vec::new();
        ret.write_to(&mut ret_buf)?;
        socket.write_all(&ret_buf).await?;

        // Write response data if any
        if !data.is_empty() {
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
