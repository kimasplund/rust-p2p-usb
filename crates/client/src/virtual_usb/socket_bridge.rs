//! Socket bridge between vhci_hcd and USB device proxy
//!
//! This module creates a TCP localhost socket bridge and handles USB/IP protocol
//! messages between the vhci_hcd kernel driver and our DeviceProxy over QUIC.
//!
//! # Architecture
//!
//! vhci_hcd expects a TCP socket with a completed USB/IP import handshake:
//! 1. TCP server listens on localhost random port
//! 2. Client connects to establish connection (this FD goes to vhci)
//! 3. USB/IP handshake (OP_REQ_IMPORT / OP_REP_IMPORT) completed over connection
//! 4. Client FD passed to vhci_hcd via sysfs attach
//! 5. Server side becomes the bridge, forwarding CMD_SUBMIT/RET_SUBMIT

use super::usbip_protocol::*;
use crate::network::device_proxy::DeviceProxy;
use anyhow::{Context, Result};
use std::net::{Ipv4Addr, SocketAddr};
use std::os::unix::io::{AsRawFd, RawFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tracing::{debug, error, info, trace, warn};

/// Socket bridge for USB/IP protocol
///
/// Bridges vhci_hcd kernel driver (via TCP localhost socket) to DeviceProxy (via QUIC)
pub struct SocketBridge {
    /// Device proxy for communicating with remote USB device
    device_proxy: Arc<DeviceProxy>,
    /// TCP socket (server side) connected to vhci_hcd
    socket: Arc<Mutex<TcpStream>>,
    /// Device ID for USB/IP protocol
    devid: u32,
    /// Port number on vhci_hcd
    port: u8,
    /// Running flag
    running: Arc<AtomicBool>,
}

impl SocketBridge {
    /// Create a new TCP-based socket bridge
    ///
    /// Returns (SocketBridge, raw_fd_for_vhci)
    /// The raw FD should be passed to vhci_hcd via sysfs attach
    pub async fn new(
        device_proxy: Arc<DeviceProxy>,
        devid: u32,
        port: u8,
    ) -> Result<(Self, RawFd)> {
        // 1. Start TCP server on localhost random port
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .await
            .context("Failed to bind TCP listener")?;

        let server_addr = listener
            .local_addr()
            .context("Failed to get listener address")?;

        debug!(
            "TCP server listening on {} for device {} port {}",
            server_addr, devid, port
        );

        // 2. Connect as client (this socket will be passed to vhci)
        let mut client_stream = TcpStream::connect(server_addr)
            .await
            .context("Failed to connect to TCP server")?;

        // 3. Accept the connection on server side
        let (mut server_stream, client_addr) = listener
            .accept()
            .await
            .context("Failed to accept connection")?;

        debug!("Accepted TCP connection from {}", client_addr);

        // 4. Get device info for handshake
        let device_info = device_proxy.device_info();

        // 5. Perform USB/IP handshake over the connection
        // Client sends OP_REQ_IMPORT, server responds OP_REP_IMPORT
        Self::perform_tcp_handshake(
            &mut client_stream,
            &mut server_stream,
            device_info,
            devid,
            port,
        )
        .await
        .context("Failed to perform USB/IP handshake")?;

        // 6. Extract client FD for vhci_hcd (leak ownership)
        let vhci_fd = client_stream.as_raw_fd();

        debug!(
            "USB/IP handshake complete, vhci_fd={} for device {} port {}",
            vhci_fd, devid, port
        );

        // Keep client_stream alive by leaking it (kernel will manage it after sysfs attach)
        std::mem::forget(client_stream);

        // 7. Server side becomes our bridge
        let bridge = Self {
            device_proxy,
            socket: Arc::new(Mutex::new(server_stream)),
            devid,
            port,
            running: Arc::new(AtomicBool::new(true)),
        };

        debug!(
            "Created TCP socket bridge: devid={}, port={}, vhci_fd={}",
            devid, port, vhci_fd
        );

        Ok((bridge, vhci_fd))
    }

    /// Perform USB/IP handshake over TCP connection
    ///
    /// This performs a proper OP_REQ_IMPORT / OP_REP_IMPORT exchange
    /// over the TCP connection before passing the FD to vhci_hcd.
    ///
    /// Client side sends OP_REQ_IMPORT, server side responds with OP_REP_IMPORT.
    async fn perform_tcp_handshake(
        client: &mut TcpStream,
        server: &mut TcpStream,
        device_info: &protocol::DeviceInfo,
        devid: u32,
        port: u8,
    ) -> Result<()> {
        debug!(
            "Performing USB/IP TCP handshake for device {} on port {}",
            devid, port
        );

        // Create bus ID (format: "port-devid")
        let busid = format!("{}-{}", port, devid);

        // Serialize OP_REQ_IMPORT
        let req = UsbIpReqImport::new(&busid);
        let mut req_buf = Vec::new();
        req.write_to(&mut req_buf)
            .context("Failed to serialize OP_REQ_IMPORT")?;

        // Serialize OP_REP_IMPORT
        let rep = UsbIpRepImport::from_device_info(device_info, &busid);
        let mut rep_buf = Vec::new();
        rep.write_to(&mut rep_buf)
            .context("Failed to serialize OP_REP_IMPORT")?;

        debug!(
            "Handshake message sizes: OP_REQ_IMPORT={} bytes, OP_REP_IMPORT={} bytes",
            req_buf.len(),
            rep_buf.len()
        );

        // Split the TCP streams for independent read/write operations
        // We need to use split() to get separate read and write halves
        let (mut client_read, mut client_write) = client.split();
        let (mut server_read, mut server_write) = server.split();

        // Client sends OP_REQ_IMPORT
        client_write
            .write_all(&req_buf)
            .await
            .context("Failed to write OP_REQ_IMPORT")?;
        client_write
            .flush()
            .await
            .context("Failed to flush OP_REQ_IMPORT")?;

        trace!("Client sent OP_REQ_IMPORT: {} bytes", req_buf.len());

        // Server reads OP_REQ_IMPORT
        let mut req_read_buf = vec![0u8; req_buf.len()];
        server_read
            .read_exact(&mut req_read_buf)
            .await
            .context("Failed to read OP_REQ_IMPORT")?;

        trace!(
            "Server received OP_REQ_IMPORT: {} bytes",
            req_read_buf.len()
        );

        // Server sends OP_REP_IMPORT
        server_write
            .write_all(&rep_buf)
            .await
            .context("Failed to write OP_REP_IMPORT")?;
        server_write
            .flush()
            .await
            .context("Failed to flush OP_REP_IMPORT")?;

        trace!("Server sent OP_REP_IMPORT: {} bytes", rep_buf.len());

        // Client reads OP_REP_IMPORT
        let mut rep_read_buf = vec![0u8; rep_buf.len()];
        client_read
            .read_exact(&mut rep_read_buf)
            .await
            .context("Failed to read OP_REP_IMPORT")?;

        trace!(
            "Client received OP_REP_IMPORT: {} bytes",
            rep_read_buf.len()
        );

        info!(
            "USB/IP TCP handshake complete for device {} ({})",
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
