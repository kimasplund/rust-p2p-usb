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
//!
//! # USB 3.0 SuperSpeed Support
//!
//! This module supports USB 3.0 SuperSpeed devices with:
//! - Larger URB buffer sizes (up to 1MB for bulk transfers)
//! - SuperSpeed port assignment (ports 8-15 for SS devices)
//! - Speed-aware buffer allocation

use anyhow::Result;
use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use protocol::{
    DeviceSpeed, IsoPacketDescriptor, RequestId, TransferType, UsbRequest, UsbResponse,
};
use std::io::{Read, Write};

/// USB/IP protocol version
#[allow(dead_code)]
pub const USBIP_VERSION: u16 = 0x0111; // Version 1.1.1

/// Default URB buffer size for USB 2.0 devices (64KB)
pub const URB_BUFFER_SIZE_HIGH_SPEED: usize = 64 * 1024;

/// URB buffer size for USB 3.0 SuperSpeed devices (256KB)
pub const URB_BUFFER_SIZE_SUPERSPEED: usize = 256 * 1024;

/// Maximum URB buffer size for USB 3.0 SuperSpeed+ devices (1MB)
pub const URB_BUFFER_SIZE_SUPERSPEED_PLUS: usize = 1024 * 1024;

/// Get optimal URB buffer size based on device speed
pub fn optimal_urb_buffer_size(speed: DeviceSpeed) -> usize {
    match speed {
        DeviceSpeed::Low | DeviceSpeed::Full => URB_BUFFER_SIZE_HIGH_SPEED,
        DeviceSpeed::High => URB_BUFFER_SIZE_HIGH_SPEED,
        DeviceSpeed::Super => URB_BUFFER_SIZE_SUPERSPEED,
        DeviceSpeed::SuperPlus => URB_BUFFER_SIZE_SUPERSPEED_PLUS,
    }
}

/// USB/IP import/export commands
#[allow(dead_code)]
pub const OP_REQ_IMPORT: u16 = 0x8003;
#[allow(dead_code)]
pub const OP_REP_IMPORT: u16 = 0x0003;

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

/// Parsed USB/IP message from vhci_hcd
///
/// Represents either a CMD_SUBMIT or CMD_UNLINK message with its header and payload
#[derive(Debug)]
pub enum UsbIpMessage {
    /// USB transfer submission request
    Submit {
        header: UsbIpHeader,
        cmd: UsbIpCmdSubmit,
        data: Vec<u8>,
    },
    /// USB transfer cancellation request
    Unlink {
        header: UsbIpHeader,
        cmd: UsbIpCmdUnlink,
    },
}

/// USB/IP common header (48 bytes)
///
/// This header precedes all USB/IP messages
#[derive(Debug, Clone)]
pub struct UsbIpHeader {
    /// Command code (u32 in kernel, but we only use lower 16 bits)
    pub command: u32,
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
    /// Size of the basic header in bytes (without payload)
    /// This is just the 5 u32 fields: command, seqnum, devid, direction, ep
    pub const SIZE: usize = 20;

    /// Create a new header
    pub fn new(command: UsbIpCommand, seqnum: u32, devid: u32) -> Self {
        Self {
            command: command as u32,
            seqnum,
            devid,
            direction: 0,
            ep: 0,
        }
    }

    /// Read header from a reader
    /// This reads only the basic header (20 bytes), NOT including payload
    pub fn read_from<R: Read>(reader: &mut R) -> Result<Self> {
        let command = reader.read_u32::<BigEndian>()?;
        let seqnum = reader.read_u32::<BigEndian>()?;
        let devid = reader.read_u32::<BigEndian>()?;
        let direction = reader.read_u32::<BigEndian>()?;
        let ep = reader.read_u32::<BigEndian>()?;

        // NO padding - header is exactly 20 bytes (matches kernel's usbip_header_basic)

        Ok(Self {
            command,
            seqnum,
            devid,
            direction,
            ep,
        })
    }

    /// Write header to a writer
    /// This writes only the basic header (20 bytes), NOT including payload
    pub fn write_to<W: Write>(&self, writer: &mut W) -> Result<()> {
        writer.write_u32::<BigEndian>(self.command)?;
        writer.write_u32::<BigEndian>(self.seqnum)?;
        writer.write_u32::<BigEndian>(self.devid)?;
        writer.write_u32::<BigEndian>(self.direction)?;
        writer.write_u32::<BigEndian>(self.ep)?;

        // NO padding - header is exactly 20 bytes (matches kernel's usbip_header_basic)

        Ok(())
    }

    /// Get command type
    pub fn command_type(&self) -> Result<UsbIpCommand> {
        UsbIpCommand::from_u16(self.command as u16)
    }
}

/// USB/IP ISO packet descriptor
///
/// Used in isochronous transfers to describe each packet
#[derive(Debug, Clone, Copy)]
pub struct UsbIpIsoPacketDescriptor {
    pub offset: u32,
    pub length: u32,
    pub actual_length: u32,
    pub status: u32,
}

impl UsbIpIsoPacketDescriptor {
    /// Size of ISO packet descriptor in bytes (16 bytes: 4 x u32)
    pub const SIZE: usize = 16;

    /// Read descriptor from reader
    pub fn read_from<R: Read>(reader: &mut R) -> Result<Self> {
        Ok(Self {
            offset: reader.read_u32::<BigEndian>()?,
            length: reader.read_u32::<BigEndian>()?,
            actual_length: reader.read_u32::<BigEndian>()?,
            status: reader.read_u32::<BigEndian>()?,
        })
    }

    /// Write descriptor to writer
    pub fn write_to<W: Write>(&self, writer: &mut W) -> Result<()> {
        writer.write_u32::<BigEndian>(self.offset)?;
        writer.write_u32::<BigEndian>(self.length)?;
        writer.write_u32::<BigEndian>(self.actual_length)?;
        writer.write_u32::<BigEndian>(self.status)?;
        Ok(())
    }
}

/// USB/IP CMD_SUBMIT payload
///
/// Follows the common header when vhci_hcd sends a USB request
#[derive(Debug, Clone)]
#[allow(dead_code)]
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
    /// ISO packet descriptors (only if number_of_packets > 0)
    pub iso_packets: Vec<UsbIpIsoPacketDescriptor>,
}

#[allow(dead_code)]
impl UsbIpCmdSubmit {
    /// Size of CMD_SUBMIT payload in bytes (28 bytes: 5 x u32 + 8-byte setup)
    /// Linux kernel struct is __packed, so no padding after setup[8]
    /// Combined with UsbIpHeader (20 bytes), total message is 48 bytes
    pub const SIZE: usize = 28;

    /// Read CMD_SUBMIT from a reader (28 bytes total)
    pub fn read_from<R: Read>(reader: &mut R) -> Result<Self> {
        let transfer_flags = reader.read_u32::<BigEndian>()?;
        let transfer_buffer_length = reader.read_u32::<BigEndian>()?;
        let start_frame = reader.read_u32::<BigEndian>()?;
        let number_of_packets = reader.read_u32::<BigEndian>()?;
        let interval = reader.read_u32::<BigEndian>()?;

        let mut setup = [0u8; 8];
        reader.read_exact(&mut setup)?;

        // NO padding - kernel struct is __packed (28 bytes total)

        // Read ISO descriptors if number_of_packets > 0
        // We must read them to keep the stream in sync, even if we don't support ISO transfers yet
        let mut iso_packets = Vec::new();
        if number_of_packets > 0 {
            for _ in 0..number_of_packets {
                iso_packets.push(UsbIpIsoPacketDescriptor::read_from(reader)?);
            }
        }

        Ok(Self {
            transfer_flags,
            transfer_buffer_length,
            start_frame,
            number_of_packets,
            interval,
            setup,
            iso_packets,
        })
    }

    /// Write CMD_SUBMIT to a writer (28 bytes total + optional ISO descriptors)
    pub fn write_to<W: Write>(&self, writer: &mut W) -> Result<()> {
        writer.write_u32::<BigEndian>(self.transfer_flags)?;
        writer.write_u32::<BigEndian>(self.transfer_buffer_length)?;
        writer.write_u32::<BigEndian>(self.start_frame)?;
        writer.write_u32::<BigEndian>(self.number_of_packets)?;
        writer.write_u32::<BigEndian>(self.interval)?;
        writer.write_all(&self.setup)?;

        // Write ISO descriptors if any
        for packet in &self.iso_packets {
            packet.write_to(writer)?;
        }

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
    /// Size of RET_SUBMIT payload in bytes (20 bytes: 5 x i32)
    /// Combined with UsbIpHeader (20 bytes), total message is 40 bytes
    #[allow(dead_code)]
    pub const SIZE: usize = 20;

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

    /// Write RET_SUBMIT to a writer (20 bytes: 5 x i32)
    /// Note: All fields should be i32 according to kernel, but we use u32 for some.
    /// This doesn't matter for serialization as we write them as raw bytes.
    pub fn write_to<W: Write>(&self, writer: &mut W) -> Result<()> {
        writer.write_i32::<BigEndian>(self.status)?;
        writer.write_i32::<BigEndian>(self.actual_length as i32)?; // Cast to i32 for kernel
        writer.write_i32::<BigEndian>(self.start_frame as i32)?; // Cast to i32 for kernel
        writer.write_i32::<BigEndian>(self.number_of_packets as i32)?; // Cast to i32 for kernel
        writer.write_i32::<BigEndian>(self.error_count as i32)?; // Cast to i32 for kernel

        // NO padding - payload is exactly 20 bytes (matches kernel's usbip_header_ret_submit)

        Ok(())
    }
}

/// USB/IP CMD_UNLINK payload
///
/// Sent by vhci_hcd to cancel a pending USB request
#[derive(Debug, Clone)]
pub struct UsbIpCmdUnlink {
    /// Sequence number of the request to unlink/cancel
    pub seqnum_unlink: u32,
}

impl UsbIpCmdUnlink {
    /// Size of CMD_UNLINK payload in bytes (4 bytes for seqnum_unlink)
    /// Note: The kernel struct has padding, but we only need the first 4 bytes
    pub const SIZE: usize = 4;

    /// Read CMD_UNLINK from a reader
    pub fn read_from<R: Read>(reader: &mut R) -> Result<Self> {
        let seqnum_unlink = reader.read_u32::<BigEndian>()?;
        Ok(Self { seqnum_unlink })
    }
}

/// USB/IP RET_UNLINK payload
///
/// Response sent back to vhci_hcd after processing an unlink request
#[derive(Debug, Clone)]
pub struct UsbIpRetUnlink {
    /// Status code: 0 = success (cancelled), -ENOENT = not found (already completed)
    pub status: i32,
}

impl UsbIpRetUnlink {
    /// Size of RET_UNLINK payload in bytes (4 bytes for status)
    #[allow(dead_code)]
    pub const SIZE: usize = 4;

    /// Create a successful unlink response (request was cancelled)
    #[allow(dead_code)]
    pub fn success() -> Self {
        Self { status: 0 }
    }

    /// Create a not-found response (request already completed)
    #[allow(dead_code)]
    pub fn not_found() -> Self {
        Self { status: -2 } // -ENOENT
    }

    /// Write RET_UNLINK to a writer
    pub fn write_to<W: Write>(&self, writer: &mut W) -> Result<()> {
        writer.write_i32::<BigEndian>(self.status)?;
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
        // wLength is in setup[6..8], used for buffer size on IN transfers

        // For control IN transfers (request_type bit 7 set), we need to tell the server
        // how much data to read. The data vec is empty for IN transfers, but the server
        // uses data.len() to determine buffer size. Create a buffer of transfer_buffer_length.
        let control_data = if (request_type & 0x80) != 0 && data.is_empty() {
            // Control IN: create buffer of expected size from transfer_buffer_length
            vec![0u8; cmd.transfer_buffer_length as usize]
        } else {
            // Control OUT: use the data as-is
            data
        };

        TransferType::Control {
            request_type,
            request,
            value,
            index,
            data: control_data,
        }
    } else {
        // Bulk or Interrupt transfer based on endpoint and interval
        // IMPORTANT: USB/IP header.ep only contains the endpoint NUMBER (0-15).
        // The direction is stored separately in header.direction (0=OUT, 1=IN).
        // USB APIs expect the direction bit in the endpoint address:
        // - OUT endpoint 2 = 0x02
        // - IN endpoint 2 = 0x82 (bit 7 set for IN)
        let endpoint = if header.direction == 1 {
            header.ep as u8 | 0x80 // IN: set direction bit
        } else {
            header.ep as u8 // OUT: no direction bit
        };
        let timeout_ms = 5000; // Default 5 second timeout

        // For IN transfers, we need to tell the server how much data to read.
        // The server uses data.len() as the buffer size, so for IN transfers
        // we create a buffer of the expected size (transfer_buffer_length).
        // For OUT transfers, data already contains the data to send.
        let transfer_data = if header.direction == 1 {
            // IN transfer: create buffer of expected size
            vec![0u8; cmd.transfer_buffer_length as usize]
        } else {
            // OUT transfer: use the data from the request
            data
        };

        // Determine transfer type based on number_of_packets and interval
        // number_of_packets > 0 indicates isochronous transfer
        // interval > 1 indicates interrupt transfer (interval=0 or 1 is commonly bulk)
        if cmd.number_of_packets > 0 {
            // Isochronous transfer
            // Convert USB/IP ISO descriptors to protocol IsoPacketDescriptor
            let iso_packet_descriptors: Vec<IsoPacketDescriptor> = cmd
                .iso_packets
                .iter()
                .map(|p| IsoPacketDescriptor {
                    offset: p.offset,
                    length: p.length,
                    actual_length: p.actual_length,
                    status: p.status as i32,
                })
                .collect();

            TransferType::Isochronous {
                endpoint,
                data: transfer_data,
                iso_packet_descriptors,
                start_frame: cmd.start_frame,
                interval: cmd.interval,
                timeout_ms,
            }
        } else if cmd.interval >= 1 {
            // Interrupt transfer (has polling interval >= 1)
            // HID devices (keyboards, mice) typically use interval=1 for high-speed
            // The previous condition (interval > 1) incorrectly classified these as bulk
            TransferType::Interrupt {
                endpoint,
                data: transfer_data,
                timeout_ms,
            }
        } else {
            // Bulk transfer (interval=0, used by mass storage, CDC, etc.)
            TransferType::Bulk {
                endpoint,
                data: transfer_data,
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

/// Result from converting UsbResponse to USB/IP format
pub struct UsbIpConvertedResponse {
    pub ret: UsbIpRetSubmit,
    pub data: Vec<u8>,
    pub iso_packets: Vec<UsbIpIsoPacketDescriptor>,
}

/// Convert our protocol UsbResponse to USB/IP RET_SUBMIT
pub fn usb_response_to_usbip(response: &UsbResponse) -> (UsbIpRetSubmit, Vec<u8>) {
    let converted = usb_response_to_usbip_full(response);
    (converted.ret, converted.data)
}

/// Convert our protocol UsbResponse to USB/IP RET_SUBMIT with full ISO support
pub fn usb_response_to_usbip_full(response: &UsbResponse) -> UsbIpConvertedResponse {
    match &response.result {
        protocol::TransferResult::Success { data } => {
            let ret = UsbIpRetSubmit::success(data.len() as u32);
            UsbIpConvertedResponse {
                ret,
                data: data.clone(),
                iso_packets: Vec::new(),
            }
        }
        protocol::TransferResult::IsochronousSuccess {
            data,
            iso_packet_descriptors,
            start_frame,
            error_count,
        } => {
            let ret = UsbIpRetSubmit {
                status: 0,
                actual_length: data.len() as u32,
                start_frame: *start_frame,
                number_of_packets: iso_packet_descriptors.len() as u32,
                error_count: *error_count,
            };
            let iso_packets: Vec<UsbIpIsoPacketDescriptor> = iso_packet_descriptors
                .iter()
                .map(|p| UsbIpIsoPacketDescriptor {
                    offset: p.offset,
                    length: p.length,
                    actual_length: p.actual_length,
                    status: p.status as u32,
                })
                .collect();
            UsbIpConvertedResponse {
                ret,
                data: data.clone(),
                iso_packets,
            }
        }
        protocol::TransferResult::Error { error } => {
            // Map protocol errors to Linux errno values
            let errno = match error {
                protocol::UsbError::Timeout => -110,     // ETIMEDOUT
                protocol::UsbError::Pipe => -32,         // EPIPE
                protocol::UsbError::NoDevice => -19,     // ENODEV
                protocol::UsbError::InvalidParam => -22, // EINVAL
                protocol::UsbError::Busy => -16,         // EBUSY
                protocol::UsbError::Overflow => -75,     // EOVERFLOW
                protocol::UsbError::Io => -5,            // EIO
                protocol::UsbError::Access => -13,       // EACCES
                protocol::UsbError::NotFound => -2,      // ENOENT
                protocol::UsbError::Other { .. } => -5,  // EIO
            };
            let ret = UsbIpRetSubmit::error(errno);
            UsbIpConvertedResponse {
                ret,
                data: Vec::new(),
                iso_packets: Vec::new(),
            }
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
            iso_packets: Vec::new(),
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

    #[test]
    fn test_ret_submit_serialization_success() {
        // Test successful RET_SUBMIT with 18 bytes of data transferred
        let ret = UsbIpRetSubmit::success(18);

        let mut buf = Vec::new();
        ret.write_to(&mut buf).unwrap();

        // Verify exact size matches kernel struct (5 x i32 = 20 bytes, __packed)
        assert_eq!(buf.len(), 20);
        assert_eq!(buf.len(), UsbIpRetSubmit::SIZE);

        // Verify wire format (big-endian)
        // status = 0
        assert_eq!(&buf[0..4], &[0x00, 0x00, 0x00, 0x00]);
        // actual_length = 18 (0x12)
        assert_eq!(&buf[4..8], &[0x00, 0x00, 0x00, 0x12]);
        // start_frame = 0
        assert_eq!(&buf[8..12], &[0x00, 0x00, 0x00, 0x00]);
        // number_of_packets = 0
        assert_eq!(&buf[12..16], &[0x00, 0x00, 0x00, 0x00]);
        // error_count = 0
        assert_eq!(&buf[16..20], &[0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn test_ret_submit_serialization_error() {
        // Test error RET_SUBMIT with ETIMEDOUT (-110)
        let ret = UsbIpRetSubmit::error(-110);

        let mut buf = Vec::new();
        ret.write_to(&mut buf).unwrap();

        assert_eq!(buf.len(), UsbIpRetSubmit::SIZE);

        // Verify status is -110 in big-endian two's complement
        // -110 = 0xFFFFFF92
        assert_eq!(&buf[0..4], &[0xFF, 0xFF, 0xFF, 0x92]);
        // actual_length = 0
        assert_eq!(&buf[4..8], &[0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn test_ret_submit_serialization_large_transfer() {
        // Test with larger transfer size (64KB - common for bulk transfers)
        let ret = UsbIpRetSubmit::success(65536);

        let mut buf = Vec::new();
        ret.write_to(&mut buf).unwrap();

        assert_eq!(buf.len(), UsbIpRetSubmit::SIZE);

        // actual_length = 65536 (0x00010000)
        assert_eq!(&buf[4..8], &[0x00, 0x01, 0x00, 0x00]);
    }

    #[test]
    fn test_full_message_size() {
        // Verify total message sizes match USB/IP protocol spec
        // CMD_SUBMIT: header (20) + payload (28) = 48 bytes
        assert_eq!(UsbIpHeader::SIZE + UsbIpCmdSubmit::SIZE, 48);

        // RET_SUBMIT: header (20) + payload (20) = 40 bytes
        assert_eq!(UsbIpHeader::SIZE + UsbIpRetSubmit::SIZE, 40);
    }

    #[test]
    fn test_cmd_unlink_read() {
        // Test reading CMD_UNLINK payload
        // seqnum_unlink = 42 (0x0000002A) in big-endian
        let data = [0x00, 0x00, 0x00, 0x2A];
        let mut cursor = Cursor::new(&data);
        let cmd = UsbIpCmdUnlink::read_from(&mut cursor).unwrap();

        assert_eq!(cmd.seqnum_unlink, 42);
    }

    #[test]
    fn test_ret_unlink_success() {
        // Test RET_UNLINK with success status (cancelled)
        let ret = UsbIpRetUnlink::success();

        let mut buf = Vec::new();
        ret.write_to(&mut buf).unwrap();

        assert_eq!(buf.len(), 4);
        assert_eq!(ret.status, 0);
        // Verify wire format: status = 0 in big-endian
        assert_eq!(&buf[..], &[0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn test_ret_unlink_not_found() {
        // Test RET_UNLINK with not-found status (already completed)
        let ret = UsbIpRetUnlink::not_found();

        let mut buf = Vec::new();
        ret.write_to(&mut buf).unwrap();

        assert_eq!(buf.len(), 4);
        assert_eq!(ret.status, -2); // -ENOENT
        // Verify wire format: -2 = 0xFFFFFFFE in big-endian
        assert_eq!(&buf[..], &[0xFF, 0xFF, 0xFF, 0xFE]);
    }

    #[test]
    fn test_cmd_unlink_message_size() {
        // Verify CMD_UNLINK message sizes
        // CMD_UNLINK: header (20) + payload (4) = 24 bytes
        assert_eq!(UsbIpHeader::SIZE + UsbIpCmdUnlink::SIZE, 24);
    }

    #[test]
    fn test_urb_buffer_size_constants() {
        assert_eq!(URB_BUFFER_SIZE_HIGH_SPEED, 64 * 1024);
        assert_eq!(URB_BUFFER_SIZE_SUPERSPEED, 256 * 1024);
        assert_eq!(URB_BUFFER_SIZE_SUPERSPEED_PLUS, 1024 * 1024);
    }

    #[test]
    fn test_optimal_urb_buffer_size() {
        assert_eq!(
            optimal_urb_buffer_size(DeviceSpeed::Low),
            URB_BUFFER_SIZE_HIGH_SPEED
        );
        assert_eq!(
            optimal_urb_buffer_size(DeviceSpeed::Full),
            URB_BUFFER_SIZE_HIGH_SPEED
        );
        assert_eq!(
            optimal_urb_buffer_size(DeviceSpeed::High),
            URB_BUFFER_SIZE_HIGH_SPEED
        );
        assert_eq!(
            optimal_urb_buffer_size(DeviceSpeed::Super),
            URB_BUFFER_SIZE_SUPERSPEED
        );
        assert_eq!(
            optimal_urb_buffer_size(DeviceSpeed::SuperPlus),
            URB_BUFFER_SIZE_SUPERSPEED_PLUS
        );
    }

    #[test]
    fn test_map_device_speed_to_u32() {
        assert_eq!(map_device_speed_to_u32(DeviceSpeed::Low), 1);
        assert_eq!(map_device_speed_to_u32(DeviceSpeed::Full), 2);
        assert_eq!(map_device_speed_to_u32(DeviceSpeed::High), 3);
        assert_eq!(map_device_speed_to_u32(DeviceSpeed::Super), 5);
        assert_eq!(map_device_speed_to_u32(DeviceSpeed::SuperPlus), 6);
    }

    #[test]
    fn test_ret_submit_serialization_superspeed() {
        // Test with SuperSpeed transfer size (256KB)
        let ret = UsbIpRetSubmit::success(URB_BUFFER_SIZE_SUPERSPEED as u32);

        let mut buf = Vec::new();
        ret.write_to(&mut buf).unwrap();

        assert_eq!(buf.len(), UsbIpRetSubmit::SIZE);

        // actual_length = 262144 (0x00040000)
        assert_eq!(&buf[4..8], &[0x00, 0x04, 0x00, 0x00]);
    }

    #[test]
    fn test_ret_submit_serialization_superspeed_plus() {
        // Test with SuperSpeed+ transfer size (1MB)
        let ret = UsbIpRetSubmit::success(URB_BUFFER_SIZE_SUPERSPEED_PLUS as u32);

        let mut buf = Vec::new();
        ret.write_to(&mut buf).unwrap();

        assert_eq!(buf.len(), UsbIpRetSubmit::SIZE);

        // actual_length = 1048576 (0x00100000)
        assert_eq!(&buf[4..8], &[0x00, 0x10, 0x00, 0x00]);
    }

    #[test]
    fn test_usb_response_to_usbip_simple() {
        // Test the simple response conversion helper
        let response = protocol::UsbResponse {
            id: protocol::RequestId(1),
            result: protocol::TransferResult::Success {
                data: vec![0x01, 0x02, 0x03, 0x04],
            },
        };

        let (ret, data) = usb_response_to_usbip(&response);

        assert_eq!(ret.status, 0);
        assert_eq!(ret.actual_length, 4);
        assert_eq!(data, vec![0x01, 0x02, 0x03, 0x04]);
    }

    #[test]
    fn test_usb_response_to_usbip_error() {
        // Test error response conversion
        let response = protocol::UsbResponse {
            id: protocol::RequestId(2),
            result: protocol::TransferResult::Error {
                error: protocol::UsbError::Pipe,
            },
        };

        let (ret, data) = usb_response_to_usbip(&response);

        // EPIPE = -32, representing stall/pipe error
        assert_eq!(ret.status, -32);
        assert_eq!(ret.actual_length, 0);
        assert!(data.is_empty());
    }

    #[test]
    fn test_iso_packet_descriptor_size() {
        // Verify SIZE constant matches actual serialization
        let desc = UsbIpIsoPacketDescriptor {
            offset: 0,
            length: 512,
            actual_length: 512,
            status: 0,
        };

        let mut buf = Vec::new();
        desc.write_to(&mut buf).unwrap();

        assert_eq!(buf.len(), UsbIpIsoPacketDescriptor::SIZE);
    }

    #[test]
    fn test_cmd_unlink_size() {
        // Verify SIZE constant is correct (4 bytes for seqnum)
        assert_eq!(UsbIpCmdUnlink::SIZE, 4);
    }
}

/// OP_REQ_IMPORT message (40 bytes)
///
/// Sent by client to request importing a USB device
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct UsbIpReqImport {
    /// USB/IP version (0x0111)
    pub version: u16,
    /// Command code (OP_REQ_IMPORT = 0x8003)
    pub command: u16,
    /// Status (0 for request)
    pub status: u32,
    /// Bus ID string (32 bytes, null-terminated)
    pub busid: [u8; 32],
}

#[allow(dead_code)]
impl UsbIpReqImport {
    pub fn new(busid: &str) -> Self {
        let mut busid_bytes = [0u8; 32];
        let bytes = busid.as_bytes();
        let len = bytes.len().min(31); // Leave room for null terminator
        busid_bytes[..len].copy_from_slice(&bytes[..len]);

        Self {
            version: USBIP_VERSION,
            command: OP_REQ_IMPORT,
            status: 0,
            busid: busid_bytes,
        }
    }

    pub fn write_to<W: Write>(&self, writer: &mut W) -> Result<()> {
        writer.write_u16::<BigEndian>(self.version)?;
        writer.write_u16::<BigEndian>(self.command)?;
        writer.write_u32::<BigEndian>(self.status)?;
        writer.write_all(&self.busid)?;
        Ok(())
    }
}

/// OP_REP_IMPORT message (header + device info)
///
/// Sent by server in response to OP_REQ_IMPORT
#[derive(Debug, Clone)]
pub struct UsbIpRepImport {
    /// Version
    pub version: u16,
    /// Command (OP_REP_IMPORT = 0x0003)
    pub command: u16,
    /// Status (0 = success)
    pub status: u32,
    /// Device path (256 bytes)
    pub udev_path: [u8; 256],
    /// Bus ID (32 bytes)
    pub busid: [u8; 32],
    /// Bus number
    pub busnum: u32,
    /// Device number
    pub devnum: u32,
    /// Device speed (1-6)
    pub speed: u32,
    /// Vendor ID
    pub id_vendor: u16,
    /// Product ID
    pub id_product: u16,
    /// Device release
    pub bcd_device: u16,
    /// Device class
    pub b_device_class: u8,
    /// Device subclass
    pub b_device_subclass: u8,
    /// Device protocol
    pub b_device_protocol: u8,
    /// Number of configurations
    pub b_num_configurations: u8,
    /// Number of interfaces
    pub b_num_interfaces: u8,
}

#[allow(dead_code)]
impl UsbIpRepImport {
    pub fn from_device_info(info: &protocol::DeviceInfo, busid: &str) -> Self {
        let mut busid_bytes = [0u8; 32];
        let bytes = busid.as_bytes();
        let len = bytes.len().min(31);
        busid_bytes[..len].copy_from_slice(&bytes[..len]);

        Self {
            version: USBIP_VERSION,
            command: OP_REP_IMPORT,
            status: 0,
            udev_path: [0u8; 256], // Not used in our case
            busid: busid_bytes,
            busnum: info.bus_number as u32,
            devnum: info.device_address as u32,
            speed: map_device_speed_to_u32(info.speed),
            id_vendor: info.vendor_id,
            id_product: info.product_id,
            bcd_device: 0x0200, // USB 2.0 device
            b_device_class: info.class,
            b_device_subclass: info.subclass,
            b_device_protocol: info.protocol,
            b_num_configurations: info.num_configurations,
            b_num_interfaces: 1, // Simplified
        }
    }

    pub fn write_to<W: Write>(&self, writer: &mut W) -> Result<()> {
        writer.write_u16::<BigEndian>(self.version)?;
        writer.write_u16::<BigEndian>(self.command)?;
        writer.write_u32::<BigEndian>(self.status)?;
        writer.write_all(&self.udev_path)?;
        writer.write_all(&self.busid)?;
        writer.write_u32::<BigEndian>(self.busnum)?;
        writer.write_u32::<BigEndian>(self.devnum)?;
        writer.write_u32::<BigEndian>(self.speed)?;
        writer.write_u16::<BigEndian>(self.id_vendor)?;
        writer.write_u16::<BigEndian>(self.id_product)?;
        writer.write_u16::<BigEndian>(self.bcd_device)?;
        writer.write_u8(self.b_device_class)?;
        writer.write_u8(self.b_device_subclass)?;
        writer.write_u8(self.b_device_protocol)?;
        writer.write_u8(self.b_num_configurations)?;
        writer.write_u8(self.b_num_interfaces)?;
        // Padding to align
        writer.write_all(&[0u8; 3])?;
        Ok(())
    }
}

/// Map DeviceSpeed enum to USB/IP protocol speed value
#[allow(dead_code)]
fn map_device_speed_to_u32(speed: protocol::DeviceSpeed) -> u32 {
    match speed {
        protocol::DeviceSpeed::Low => 1,
        protocol::DeviceSpeed::Full => 2,
        protocol::DeviceSpeed::High => 3,
        protocol::DeviceSpeed::Super => 5,
        protocol::DeviceSpeed::SuperPlus => 6,
    }
}
