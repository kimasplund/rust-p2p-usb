//! USB transfer execution
//!
//! This module handles executing USB transfers (control, bulk, interrupt) using rusb.
//! It provides synchronous transfer functions that map rusb errors to protocol errors.

use protocol::{TransferResult, TransferType, UsbError, UsbResponse};
use rusb::DeviceHandle;
use std::time::Duration;
use tracing::{debug, warn};

/// Default timeout for USB transfers (5 seconds)
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Execute a USB transfer and return the response
///
/// This function dispatches to the appropriate transfer type handler and
/// ensures proper error mapping from rusb to protocol types.
pub fn execute_transfer(
    handle: &mut DeviceHandle<rusb::Context>,
    transfer: TransferType,
    request_id: protocol::RequestId,
) -> UsbResponse {
    let result = match transfer {
        TransferType::Control {
            request_type,
            request,
            value,
            index,
            data,
        } => execute_control_transfer(handle, request_type, request, value, index, data),

        TransferType::Bulk {
            endpoint,
            data,
            timeout_ms,
        } => execute_bulk_transfer(handle, endpoint, data, timeout_ms),

        TransferType::Interrupt {
            endpoint,
            data,
            timeout_ms,
        } => execute_interrupt_transfer(handle, endpoint, data, timeout_ms),

        /*
        TransferType::Isochronous {
            endpoint,
            data,
            packet_lengths,
            timeout_ms,
        } => execute_isochronous_transfer(handle, endpoint, data, packet_lengths, timeout_ms),
        */
    };

    UsbResponse {
        id: request_id,
        result,
    }
}

/// Execute a control transfer
///
/// Control transfers are always synchronous and go to endpoint 0.
/// For IN transfers, data vec should be empty or contain the desired buffer size.
/// For OUT transfers, data vec contains the data to send.
fn execute_control_transfer(
    handle: &mut DeviceHandle<rusb::Context>,
    request_type: u8,
    request: u8,
    value: u16,
    index: u16,
    data: Vec<u8>,
) -> TransferResult {
    debug!(
        "Control transfer: request_type={:#x}, request={:#x}, value={:#x}, index={:#x}, data_len={}",
        request_type,
        request,
        value,
        index,
        data.len()
    );

    // Determine direction from request_type bit 7
    let is_in = (request_type & 0x80) != 0;

    let result = if is_in {
        // IN transfer: read from device
        let buffer_size = if data.is_empty() {
            64 // Default control transfer buffer size
        } else {
            data.len()
        };
        let mut buffer = vec![0u8; buffer_size];

        match handle.read_control(
            request_type,
            request,
            value,
            index,
            &mut buffer,
            DEFAULT_TIMEOUT,
        ) {
            Ok(len) => {
                buffer.truncate(len);
                Ok(buffer)
            }
            Err(e) => Err(map_rusb_error(e)),
        }
    } else {
        // OUT transfer: write to device
        match handle.write_control(request_type, request, value, index, &data, DEFAULT_TIMEOUT) {
            Ok(_len) => Ok(Vec::new()), // OUT transfers return no data
            Err(e) => Err(map_rusb_error(e)),
        }
    };

    match result {
        Ok(data) => {
            debug!("Control transfer succeeded: {} bytes", data.len());
            TransferResult::Success { data }
        }
        Err(error) => {
            warn!("Control transfer failed: {:?}", error);
            TransferResult::Error { error }
        }
    }
}

/// Execute a bulk transfer
///
/// Bulk transfers are used for large data transfers (storage, network, etc.).
/// For IN transfers, data vec length specifies the buffer size.
/// For OUT transfers, data vec contains the data to send.
fn execute_bulk_transfer(
    handle: &mut DeviceHandle<rusb::Context>,
    endpoint: u8,
    data: Vec<u8>,
    timeout_ms: u32,
) -> TransferResult {
    let is_in = (endpoint & 0x80) != 0;

    // For bulk IN transfers (like printer status reads), use a short timeout
    // since the device may not have data available. This prevents blocking
    // and allows USB/IP clients to continue without long waits.
    let timeout = if is_in {
        Duration::from_millis(100.min(timeout_ms as u64))
    } else {
        Duration::from_millis(timeout_ms as u64)
    };

    debug!(
        "Bulk transfer: endpoint={:#x}, data_len={}, timeout={}ms, is_in={}",
        endpoint,
        data.len(),
        timeout.as_millis(),
        is_in
    );

    let result = if is_in {
        // IN transfer: read from device
        let mut buffer = vec![0u8; data.len()];
        match handle.read_bulk(endpoint, &mut buffer, timeout) {
            Ok(len) => {
                buffer.truncate(len);
                Ok(buffer)
            }
            Err(rusb::Error::Timeout) | Err(rusb::Error::Io) => {
                // For bulk IN, timeout/IO error is normal - device has no data available
                // Return empty success instead of error to avoid breaking USB/IP clients
                // Printers often return IO error when no status data is pending
                debug!(
                    "Bulk IN timeout/io on endpoint {:#x} - returning empty (no data available)",
                    endpoint
                );
                Ok(Vec::new())
            }
            Err(e) => Err(map_rusb_error(e)),
        }
    } else {
        // OUT transfer: write to device
        match handle.write_bulk(endpoint, &data, timeout) {
            Ok(_len) => Ok(Vec::new()),
            Err(e) => Err(map_rusb_error(e)),
        }
    };

    match result {
        Ok(data) => {
            debug!("Bulk transfer succeeded: {} bytes", data.len());
            TransferResult::Success { data }
        }
        Err(error) => {
            warn!("Bulk transfer failed: {:?}", error);
            TransferResult::Error { error }
        }
    }
}

/// Execute an interrupt transfer
///
/// Interrupt transfers are used for low-latency devices (HID, etc.).
/// For IN transfers, data vec length specifies the buffer size.
/// For OUT transfers, data vec contains the data to send.
fn execute_interrupt_transfer(
    handle: &mut DeviceHandle<rusb::Context>,
    endpoint: u8,
    data: Vec<u8>,
    timeout_ms: u32,
) -> TransferResult {
    let is_in = (endpoint & 0x80) != 0;

    // For interrupt IN transfers (like printer status reads), use a short timeout
    // since the device may not have data available. This prevents blocking
    // and allows USB/IP clients to continue without long waits.
    let timeout = if is_in {
        Duration::from_millis(100.min(timeout_ms as u64))
    } else {
        Duration::from_millis(timeout_ms as u64)
    };

    debug!(
        "Interrupt transfer: endpoint={:#x}, data_len={}, timeout={}ms, is_in={}",
        endpoint,
        data.len(),
        timeout.as_millis(),
        is_in
    );

    let result = if is_in {
        // IN transfer: read from device
        let mut buffer = vec![0u8; data.len()];
        match handle.read_interrupt(endpoint, &mut buffer, timeout) {
            Ok(len) => {
                buffer.truncate(len);
                Ok(buffer)
            }
            Err(rusb::Error::Timeout) | Err(rusb::Error::Io) => {
                // For interrupt IN, timeout/IO error is normal - device has no data available
                // Return empty success instead of error to avoid breaking USB/IP clients
                // Printers often return IO error when no status data is pending
                debug!(
                    "Interrupt IN timeout/io on endpoint {:#x} - returning empty (no data available)",
                    endpoint
                );
                Ok(Vec::new())
            }
            Err(e) => Err(map_rusb_error(e)),
        }
    } else {
        // OUT transfer: write to device
        match handle.write_interrupt(endpoint, &data, timeout) {
            Ok(_len) => Ok(Vec::new()),
            Err(e) => Err(map_rusb_error(e)),
        }
    };

    match result {
        Ok(data) => {
            debug!("Interrupt transfer succeeded: {} bytes", data.len());
            TransferResult::Success { data }
        }
        Err(error) => {
            warn!("Interrupt transfer failed: {:?}", error);
            TransferResult::Error { error }
        }
    }
}

/*
/// Execute an isochronous transfer
///
/// Isochronous transfers are used for audio/video streaming.
/// For IN transfers, packet_lengths specifies expected size for each packet.
/// For OUT transfers, data contains contiguous packet data, and packet_lengths specifies split.
fn execute_isochronous_transfer(
    handle: &mut DeviceHandle<rusb::Context>,
    endpoint: u8,
    data: Vec<u8>,
    packet_lengths: Vec<u32>,
    timeout_ms: u32,
) -> TransferResult {
    let is_in = (endpoint & 0x80) != 0;
    let timeout = Duration::from_millis(timeout_ms as u64);

    debug!(
        "Isochronous transfer: endpoint={:#x}, packets={}, total_len={}, is_in={}",
        endpoint,
        packet_lengths.len(),
        data.len(),
        is_in
    );

    // Convert packet lengths to i32 for rusb
    let iso_packet_lengths: Vec<i32> = packet_lengths.iter().map(|&l| l as i32).collect();

    let result = if is_in {
        // IN transfer: read from device
        // Calculate total buffer size from packet lengths
        let total_len: usize = packet_lengths.iter().map(|&l| l as usize).sum();
        let mut buffer = vec![0u8; total_len];

        match handle.read_isochronous(endpoint, &mut buffer, &iso_packet_lengths, timeout) {
            Ok(len) => {
                buffer.truncate(len);
                Ok(buffer)
            }
            Err(e) => Err(map_rusb_error(e)),
        }
    } else {
        // OUT transfer: write to device
        match handle.write_isochronous(endpoint, &data, &iso_packet_lengths, timeout) {
            Ok(_len) => Ok(Vec::new()),
            Err(e) => Err(map_rusb_error(e)),
        }
    };

    match result {
        Ok(data) => {
            debug!("Isochronous transfer succeeded: {} bytes", data.len());
            TransferResult::Success { data }
        }
        Err(error) => {
            warn!("Isochronous transfer failed: {:?}", error);
            TransferResult::Error { error }
        }
    }
}
*/

/// Map rusb::Error to protocol::UsbError
///
/// This provides a clean mapping from low-level rusb errors to protocol-level errors
/// that can be serialized and sent over the network.
pub fn map_rusb_error(err: rusb::Error) -> UsbError {
    match err {
        rusb::Error::Timeout => UsbError::Timeout,
        rusb::Error::Pipe => UsbError::Pipe,
        rusb::Error::NoDevice => UsbError::NoDevice,
        rusb::Error::NotFound => UsbError::NotFound,
        rusb::Error::Busy => UsbError::Busy,
        rusb::Error::Overflow => UsbError::Overflow,
        rusb::Error::Io => UsbError::Io,
        rusb::Error::InvalidParam => UsbError::InvalidParam,
        rusb::Error::Access => UsbError::Access,
        _ => UsbError::Other {
            message: err.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_rusb_error() {
        assert_eq!(map_rusb_error(rusb::Error::Timeout), UsbError::Timeout);
        assert_eq!(map_rusb_error(rusb::Error::Pipe), UsbError::Pipe);
        assert_eq!(map_rusb_error(rusb::Error::NoDevice), UsbError::NoDevice);
        assert_eq!(map_rusb_error(rusb::Error::NotFound), UsbError::NotFound);
    }

    #[test]
    fn test_control_transfer_direction() {
        // Bit 7 = 1 means IN (device to host)
        let request_type_in = 0x80;
        assert!((request_type_in & 0x80) != 0);

        // Bit 7 = 0 means OUT (host to device)
        let request_type_out = 0x00;
        assert!((request_type_out & 0x80) == 0);
    }

    #[test]
    fn test_endpoint_direction() {
        // Bit 7 = 1 means IN endpoint
        let endpoint_in = 0x81;
        assert!((endpoint_in & 0x80) != 0);

        // Bit 7 = 0 means OUT endpoint
        let endpoint_out = 0x01;
        assert!((endpoint_out & 0x80) == 0);
    }
}
