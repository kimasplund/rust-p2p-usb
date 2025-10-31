//! USB/IP wire protocol implementation
//!
//! This module implements the USB/IP protocol for communicating with the vhci_hcd kernel driver.
//! The protocol is documented in the Linux kernel: drivers/usb/usbip/usbip_common.h
//!
//! # Protocol Overview
//!
//! USB/IP uses a simple binary protocol over TCP/Unix sockets:
//! - All integers are big-endian (network byte order)
//! - Each message has a 48-byte header followed by optional payload
//! - Requests from vhci_hcd to userspace: CMD_SUBMIT, CMD_UNLINK
//! - Responses from userspace to vhci_hcd: RET_SUBMIT, RET_UNLINK

use anyhow::Result;
use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use protocol::{RequestId, TransferType, UsbRequest, UsbResponse};
use std::io::{Read, Write};

/// USB/IP protocol version
pub const USBIP_VERSION: u16 = 0x0111; // Version 1.1.1

/// USB/IP command codes
#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbIpCommand {
    /// Submit a USB request (URB)
    CmdSubmit = 0x0001,
    /// Return from USB request
    RetSubmit = 0x0003,
    /// Unlink a USB request
    CmdUnlink = 0x0002,
    /// Return from unlink
    RetUnlink = 0x0004,
}

impl UsbIpCommand {
    pub fn from_u16(value: u16) -> Result<Self> {
        match value {
            0x0001 => Ok(Self::CmdSubmit),
            0x0003 => Ok(Self::RetSubmit),
            0x0002 => Ok(Self::CmdUnlink),
            0x0004 => Ok(Self::RetUnlink),
            _ => Err(anyhow::anyhow!("Unknown USB/IP command: {:#06x}", value)),
        }
    }
}

/// USB/IP common header (48 bytes)
///
/// This header precedes all USB/IP messages
#[derive(Debug, Clone)]
pub struct UsbIpHeader {
    /// Command code
    pub command: u16,
    /// Sequence number for matching requests/responses
    pub seqnum: u32,
    /// Device ID
    pub devid: u32,
    /// Direction: 0 = USBIP_DIR_OUT, 1 = USBIP_DIR_IN
    pub direction: u32,
    /// Endpoint number
    pub ep: u32,
}

impl UsbIpHeader {
    /// Size of the header in bytes
    pub const SIZE: usize = 48;

    /// Create a new header
    pub fn new(command: UsbIpCommand, seqnum: u32, devid: u32) -> Self {
        Self {
            command: command as u16,
            seqnum,
            devid,
            direction: 0,
            ep: 0,
        }
    }

    /// Read header from a reader
    pub fn read_from<R: Read>(reader: &mut R) -> Result<Self> {
        let command = reader.read_u16::<BigEndian>()?;
        let seqnum = reader.read_u32::<BigEndian>()?;
        let devid = reader.read_u32::<BigEndian>()?;
        let direction = reader.read_u32::<BigEndian>()?;
        let ep = reader.read_u32::<BigEndian>()?;

        // Read and skip padding (32 bytes) to make header exactly 48 bytes
        let mut padding = [0u8; 32];
        reader.read_exact(&mut padding)?;

        Ok(Self {
            command,
            seqnum,
            devid,
            direction,
            ep,
        })
    }

    /// Write header to a writer
    pub fn write_to<W: Write>(&self, writer: &mut W) -> Result<()> {
        writer.write_u16::<BigEndian>(self.command)?;
        writer.write_u32::<BigEndian>(self.seqnum)?;
        writer.write_u32::<BigEndian>(self.devid)?;
        writer.write_u32::<BigEndian>(self.direction)?;
        writer.write_u32::<BigEndian>(self.ep)?;

        // Write padding (32 bytes) to make header exactly 48 bytes
        writer.write_all(&[0u8; 32])?;

        Ok(())
    }

    /// Get command type
    pub fn command_type(&self) -> Result<UsbIpCommand> {
        UsbIpCommand::from_u16(self.command)
    }
}

/// USB/IP CMD_SUBMIT payload
///
/// Follows the common header when vhci_hcd sends a USB request
#[derive(Debug, Clone)]
pub struct UsbIpCmdSubmit {
    /// Transfer flags
    pub transfer_flags: u32,
    /// Transfer buffer length
    pub transfer_buffer_length: u32,
    /// Start frame for isochronous/interrupt transfers
    pub start_frame: u32,
    /// Number of packets for isochronous transfers
    pub number_of_packets: u32,
    /// Interval for interrupt/isochronous transfers
    pub interval: u32,
    /// Setup packet for control transfers (8 bytes)
    pub setup: [u8; 8],
}

impl UsbIpCmdSubmit {
    /// Size of CMD_SUBMIT payload in bytes (excluding header and data)
    pub const SIZE: usize = 40;

    /// Read CMD_SUBMIT from a reader
    pub fn read_from<R: Read>(reader: &mut R) -> Result<Self> {
        let transfer_flags = reader.read_u32::<BigEndian>()?;
        let transfer_buffer_length = reader.read_u32::<BigEndian>()?;
        let start_frame = reader.read_u32::<BigEndian>()?;
        let number_of_packets = reader.read_u32::<BigEndian>()?;
        let interval = reader.read_u32::<BigEndian>()?;

        let mut setup = [0u8; 8];
        reader.read_exact(&mut setup)?;

        // Skip padding (8 bytes)
        let mut padding = [0u8; 8];
        reader.read_exact(&mut padding)?;

        Ok(Self {
            transfer_flags,
            transfer_buffer_length,
            start_frame,
            number_of_packets,
            interval,
            setup,
        })
    }

    /// Write CMD_SUBMIT to a writer
    pub fn write_to<W: Write>(&self, writer: &mut W) -> Result<()> {
        writer.write_u32::<BigEndian>(self.transfer_flags)?;
        writer.write_u32::<BigEndian>(self.transfer_buffer_length)?;
        writer.write_u32::<BigEndian>(self.start_frame)?;
        writer.write_u32::<BigEndian>(self.number_of_packets)?;
        writer.write_u32::<BigEndian>(self.interval)?;
        writer.write_all(&self.setup)?;

        // Write padding (8 bytes)
        writer.write_all(&[0u8; 8])?;

        Ok(())
    }
}

/// USB/IP RET_SUBMIT payload
///
/// Response sent back to vhci_hcd after processing a USB request
#[derive(Debug, Clone)]
pub struct UsbIpRetSubmit {
    /// Status code (0 = success, negative = error)
    pub status: i32,
    /// Actual length of data transferred
    pub actual_length: u32,
    /// Start frame for isochronous transfers
    pub start_frame: u32,
    /// Number of packets
    pub number_of_packets: u32,
    /// Error count
    pub error_count: u32,
}

impl UsbIpRetSubmit {
    /// Size of RET_SUBMIT payload in bytes (excluding header and data)
    pub const SIZE: usize = 48;

    /// Create a successful return
    pub fn success(actual_length: u32) -> Self {
        Self {
            status: 0,
            actual_length,
            start_frame: 0,
            number_of_packets: 0,
            error_count: 0,
        }
    }

    /// Create an error return
    pub fn error(status: i32) -> Self {
        Self {
            status,
            actual_length: 0,
            start_frame: 0,
            number_of_packets: 0,
            error_count: 0,
        }
    }

    /// Write RET_SUBMIT to a writer
    pub fn write_to<W: Write>(&self, writer: &mut W) -> Result<()> {
        writer.write_i32::<BigEndian>(self.status)?;
        writer.write_u32::<BigEndian>(self.actual_length)?;
        writer.write_u32::<BigEndian>(self.start_frame)?;
        writer.write_u32::<BigEndian>(self.number_of_packets)?;
        writer.write_u32::<BigEndian>(self.error_count)?;

        // Write padding (28 bytes) to make payload 48 bytes
        writer.write_all(&[0u8; 28])?;

        Ok(())
    }
}

/// Convert USB/IP CMD_SUBMIT to our protocol UsbRequest
///
/// This function converts USB/IP protocol messages from vhci_hcd into our
/// internal UsbRequest format. It requires the DeviceProxy to get the device handle.
pub async fn usbip_to_usb_request(
    device_proxy: &crate::network::device_proxy::DeviceProxy,
    header: &UsbIpHeader,
    cmd: &UsbIpCmdSubmit,
    data: Vec<u8>,
) -> Result<UsbRequest> {
    // Get device handle (must be attached)
    let handle = device_proxy.handle().await?;
    let request_id = RequestId(header.seqnum as u64);

    // Determine transfer type from setup packet or endpoint
    let transfer = if cmd.setup != [0u8; 8] {
        // Control transfer - parse the setup packet
        // Setup packet format: [bmRequestType, bRequest, wValue_lo, wValue_hi, wIndex_lo, wIndex_hi, wLength_lo, wLength_hi]
        let request_type = cmd.setup[0];
        let request = cmd.setup[1];
        let value = u16::from_le_bytes([cmd.setup[2], cmd.setup[3]]);
        let index = u16::from_le_bytes([cmd.setup[4], cmd.setup[5]]);
        // wLength is in setup[6..8], but data length comes from transfer_buffer_length

        TransferType::Control {
            request_type,
            request,
            value,
            index,
            data,
        }
    } else {
        // Bulk or Interrupt transfer based on endpoint and interval
        let endpoint = header.ep as u8;
        let timeout_ms = 5000; // Default 5 second timeout

        if cmd.interval > 0 {
            // Interrupt transfer (has polling interval)
            TransferType::Interrupt {
                endpoint,
                data,
                timeout_ms,
            }
        } else {
            // Bulk transfer
            TransferType::Bulk {
                endpoint,
                data,
                timeout_ms,
            }
        }
    };

    Ok(UsbRequest {
        id: request_id,
        handle,
        transfer,
    })
}

/// Convert our protocol UsbResponse to USB/IP RET_SUBMIT
pub fn usb_response_to_usbip(
    response: &UsbResponse,
) -> (UsbIpRetSubmit, Vec<u8>) {
    match &response.result {
        protocol::TransferResult::Success { data } => {
            let ret = UsbIpRetSubmit::success(data.len() as u32);
            (ret, data.clone())
        }
        protocol::TransferResult::Error { error } => {
            // Map protocol errors to Linux errno values
            let errno = match error {
                protocol::UsbError::Timeout => -110,      // ETIMEDOUT
                protocol::UsbError::Pipe => -32,          // EPIPE
                protocol::UsbError::NoDevice => -19,      // ENODEV
                protocol::UsbError::InvalidParam => -22,  // EINVAL
                protocol::UsbError::Busy => -16,          // EBUSY
                protocol::UsbError::Overflow => -75,      // EOVERFLOW
                protocol::UsbError::Io => -5,             // EIO
                protocol::UsbError::Access => -13,        // EACCES
                protocol::UsbError::NotFound => -2,       // ENOENT
                protocol::UsbError::Other { .. } => -5,   // EIO
            };
            let ret = UsbIpRetSubmit::error(errno);
            (ret, Vec::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_header_roundtrip() {
        let header = UsbIpHeader::new(UsbIpCommand::CmdSubmit, 42, 1);

        let mut buf = Vec::new();
        header.write_to(&mut buf).unwrap();

        assert_eq!(buf.len(), UsbIpHeader::SIZE);

        let mut cursor = Cursor::new(buf);
        let decoded = UsbIpHeader::read_from(&mut cursor).unwrap();

        assert_eq!(decoded.command, header.command);
        assert_eq!(decoded.seqnum, header.seqnum);
        assert_eq!(decoded.devid, header.devid);
    }

    #[test]
    fn test_cmd_submit_roundtrip() {
        let cmd = UsbIpCmdSubmit {
            transfer_flags: 0,
            transfer_buffer_length: 64,
            start_frame: 0,
            number_of_packets: 0,
            interval: 0,
            setup: [0x80, 0x06, 0x00, 0x01, 0x00, 0x00, 0x12, 0x00],
        };

        let mut buf = Vec::new();
        cmd.write_to(&mut buf).unwrap();

        assert_eq!(buf.len(), UsbIpCmdSubmit::SIZE);

        let mut cursor = Cursor::new(buf);
        let decoded = UsbIpCmdSubmit::read_from(&mut cursor).unwrap();

        assert_eq!(decoded.transfer_buffer_length, cmd.transfer_buffer_length);
        assert_eq!(decoded.setup, cmd.setup);
    }

    #[test]
    fn test_ret_submit_success() {
        let ret = UsbIpRetSubmit::success(18);

        let mut buf = Vec::new();
        ret.write_to(&mut buf).unwrap();

        assert_eq!(buf.len(), UsbIpRetSubmit::SIZE);
        assert_eq!(ret.status, 0);
        assert_eq!(ret.actual_length, 18);
    }

    #[test]
    fn test_ret_submit_error() {
        let ret = UsbIpRetSubmit::error(-110); // ETIMEDOUT

        assert_eq!(ret.status, -110);
        assert_eq!(ret.actual_length, 0);
    }
}
