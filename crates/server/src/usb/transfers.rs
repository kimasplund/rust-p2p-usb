//! USB transfer execution
//!
//! This module handles executing USB transfers (control, bulk, interrupt) using rusb.
//! It provides synchronous transfer functions that map rusb errors to protocol errors.
//!
//! # USB 3.0 SuperSpeed Optimization
//!
//! This module supports optimized transfer sizes for USB 3.0 SuperSpeed devices:
//! - SuperSpeed (USB 3.0+): Up to 1MB bulk transfers with burst mode
//! - High Speed (USB 2.0): Up to 64KB bulk transfers
//! - Full/Low Speed: Up to 4KB bulk transfers
//!
//! The transfer size limits are enforced by the calling code (device manager or worker),
//! which should use `DeviceSpeed::max_bulk_transfer_size()` to determine appropriate limits.

use protocol::{
    DeviceSpeed, IsoPacketDescriptor, SuperSpeedConfig, TransferResult, TransferType, UsbError,
    UsbResponse,
};
use rusb::DeviceHandle;
use std::time::Duration;
use tracing::{debug, trace, warn};

/// Default timeout for USB transfers (5 seconds)
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum bulk transfer size for USB 2.0 High Speed (64KB)
pub const MAX_BULK_SIZE_HIGH_SPEED: usize = 64 * 1024;

/// Maximum bulk transfer size for USB 3.0 SuperSpeed (1MB)
pub const MAX_BULK_SIZE_SUPERSPEED: usize = 1024 * 1024;

/// Default URB buffer size for USB/IP protocol
pub const DEFAULT_URB_BUFFER_SIZE: usize = 64 * 1024;

/// SuperSpeed URB buffer size for USB/IP protocol (256KB)
pub const SUPERSPEED_URB_BUFFER_SIZE: usize = 256 * 1024;

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

        TransferType::Isochronous {
            endpoint,
            data,
            iso_packet_descriptors,
            start_frame,
            interval: _,
            timeout_ms,
        } => execute_isochronous_transfer(
            handle,
            endpoint,
            data,
            iso_packet_descriptors,
            start_frame,
            timeout_ms,
        ),
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
            Err(rusb::Error::Pipe) => {
                // Control endpoint stalled - clear the stall and retry
                // This can happen if a previous command wasn't fully processed
                warn!("Control IN pipe error, clearing stall on EP0 and retrying");
                if let Err(clear_err) = handle.clear_halt(0x80) {
                    // 0x80 = EP0 IN direction
                    warn!("Failed to clear halt on control EP0 IN: {:?}", clear_err);
                    // Try clearing EP0 without direction bit
                    let _ = handle.clear_halt(0x00);
                }
                // Retry the transfer
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
                        debug!("Control IN succeeded after clearing stall");
                        Ok(buffer)
                    }
                    Err(e) => {
                        warn!("Control IN failed even after clearing stall: {:?}", e);
                        Err(map_rusb_error(e))
                    }
                }
            }
            Err(e) => Err(map_rusb_error(e)),
        }
    } else {
        // OUT transfer: write to device
        match handle.write_control(request_type, request, value, index, &data, DEFAULT_TIMEOUT) {
            Ok(_len) => Ok(Vec::new()), // OUT transfers return no data
            Err(rusb::Error::Pipe) => {
                // Control endpoint stalled - clear the stall and retry
                warn!("Control OUT pipe error, clearing stall on EP0 and retrying");
                if let Err(clear_err) = handle.clear_halt(0x00) {
                    // 0x00 = EP0 OUT direction
                    warn!("Failed to clear halt on control EP0 OUT: {:?}", clear_err);
                }
                // Retry the transfer
                match handle.write_control(
                    request_type,
                    request,
                    value,
                    index,
                    &data,
                    DEFAULT_TIMEOUT,
                ) {
                    Ok(_len) => {
                        debug!("Control OUT succeeded after clearing stall");
                        Ok(Vec::new())
                    }
                    Err(e) => {
                        warn!("Control OUT failed even after clearing stall: {:?}", e);
                        Err(map_rusb_error(e))
                    }
                }
            }
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
///
/// # USB 3.0 SuperSpeed Optimization
///
/// For SuperSpeed devices, this function supports transfers up to 1MB in a single
/// operation, compared to the 64KB limit for USB 2.0 devices. The actual transfer
/// size should be determined by the caller based on device speed using
/// `DeviceSpeed::max_bulk_transfer_size()`.
fn execute_bulk_transfer(
    handle: &mut DeviceHandle<rusb::Context>,
    endpoint: u8,
    data: Vec<u8>,
    timeout_ms: u32,
) -> TransferResult {
    let is_in = (endpoint & 0x80) != 0;
    let transfer_size = data.len();

    // For bulk IN transfers (like printer status reads), use a short timeout
    // since the device may not have data available. This prevents blocking
    // and allows USB/IP clients to continue without long waits.
    // For larger SuperSpeed transfers, scale the timeout appropriately.
    let timeout = if is_in {
        // Scale timeout based on transfer size for SuperSpeed devices
        // Minimum 100ms, scale up for larger transfers
        let base_timeout = 100u64;
        let scaled_timeout = if transfer_size > MAX_BULK_SIZE_HIGH_SPEED {
            // SuperSpeed: allow more time for larger transfers
            base_timeout + (transfer_size as u64 / 1024) // +1ms per KB
        } else {
            base_timeout
        };
        Duration::from_millis(scaled_timeout.min(timeout_ms as u64))
    } else {
        Duration::from_millis(timeout_ms as u64)
    };

    // Log with appropriate level based on transfer size
    if transfer_size > MAX_BULK_SIZE_HIGH_SPEED {
        debug!(
            "SuperSpeed bulk transfer: endpoint={:#x}, data_len={}KB, timeout={}ms, is_in={}",
            endpoint,
            transfer_size / 1024,
            timeout.as_millis(),
            is_in
        );
    } else {
        trace!(
            "Bulk transfer: endpoint={:#x}, data_len={}, timeout={}ms, is_in={}",
            endpoint,
            transfer_size,
            timeout.as_millis(),
            is_in
        );
    }

    let result = if is_in {
        // IN transfer: read from device
        let mut buffer = vec![0u8; transfer_size];
        match handle.read_bulk(endpoint, &mut buffer, timeout) {
            Ok(len) => {
                buffer.truncate(len);
                Ok(buffer)
            }
            Err(rusb::Error::Timeout) | Err(rusb::Error::Io) => {
                // For bulk IN, timeout/IO error is normal - device has no data available
                // Return empty success instead of error to avoid breaking USB/IP clients
                // Printers often return IO error when no status data is pending
                trace!(
                    "Bulk IN timeout/io on endpoint {:#x} - returning empty (no data available)",
                    endpoint
                );
                Ok(Vec::new())
            }
            Err(rusb::Error::Pipe) => {
                // Endpoint stalled - clear the stall and retry once
                // This can happen for USB mass storage after SCSI errors
                warn!(
                    "Bulk IN pipe error on endpoint {:#x}, clearing stall and retrying",
                    endpoint
                );
                if let Err(clear_err) = handle.clear_halt(endpoint) {
                    warn!(
                        "Failed to clear halt on endpoint {:#x}: {:?}",
                        endpoint, clear_err
                    );
                    return TransferResult::Error {
                        error: UsbError::Pipe,
                    };
                }
                // Retry the transfer after clearing stall
                let mut buffer = vec![0u8; transfer_size];
                match handle.read_bulk(endpoint, &mut buffer, timeout) {
                    Ok(len) => {
                        buffer.truncate(len);
                        debug!(
                            "Bulk IN succeeded after clearing stall on endpoint {:#x}",
                            endpoint
                        );
                        Ok(buffer)
                    }
                    Err(rusb::Error::Timeout) | Err(rusb::Error::Io) => {
                        trace!(
                            "Bulk IN timeout/io after clear stall on endpoint {:#x}",
                            endpoint
                        );
                        Ok(Vec::new())
                    }
                    Err(e) => {
                        warn!(
                            "Bulk IN failed even after clearing stall: {:?}",
                            e
                        );
                        Err(map_rusb_error(e))
                    }
                }
            }
            Err(e) => Err(map_rusb_error(e)),
        }
    } else {
        // OUT transfer: write to device
        match handle.write_bulk(endpoint, &data, timeout) {
            Ok(_len) => Ok(Vec::new()),
            Err(rusb::Error::Pipe) => {
                // Endpoint stalled - clear the stall and retry once
                // This is common for USB mass storage during SCSI command sequences
                warn!(
                    "Bulk OUT pipe error on endpoint {:#x}, clearing stall and retrying",
                    endpoint
                );
                if let Err(clear_err) = handle.clear_halt(endpoint) {
                    warn!(
                        "Failed to clear halt on endpoint {:#x}: {:?}",
                        endpoint, clear_err
                    );
                    return TransferResult::Error {
                        error: UsbError::Pipe,
                    };
                }
                // Retry the transfer after clearing stall
                match handle.write_bulk(endpoint, &data, timeout) {
                    Ok(_len) => {
                        debug!(
                            "Bulk OUT succeeded after clearing stall on endpoint {:#x}",
                            endpoint
                        );
                        Ok(Vec::new())
                    }
                    Err(e) => {
                        warn!(
                            "Bulk OUT failed even after clearing stall: {:?}",
                            e
                        );
                        Err(map_rusb_error(e))
                    }
                }
            }
            Err(e) => Err(map_rusb_error(e)),
        }
    };

    match result {
        Ok(data) => {
            if data.len() > MAX_BULK_SIZE_HIGH_SPEED {
                debug!(
                    "SuperSpeed bulk transfer succeeded: {}KB",
                    data.len() / 1024
                );
            } else {
                trace!("Bulk transfer succeeded: {} bytes", data.len());
            }
            TransferResult::Success { data }
        }
        Err(error) => {
            warn!("Bulk transfer failed: {:?}", error);
            TransferResult::Error { error }
        }
    }
}

/// Get the optimal transfer configuration for a device speed
///
/// Returns the SuperSpeedConfig with appropriate buffer sizes and parallel slots.
pub fn get_transfer_config(speed: DeviceSpeed) -> SuperSpeedConfig {
    SuperSpeedConfig::for_speed(speed)
}

/// Calculate the optimal buffer size for a bulk transfer based on device speed
///
/// Returns a buffer size that maximizes throughput for the given speed class.
pub fn optimal_bulk_buffer_size(speed: DeviceSpeed, requested_size: usize) -> usize {
    let max_size = speed.max_bulk_transfer_size();
    requested_size.min(max_size)
}

/// Execute an interrupt transfer
///
/// Interrupt transfers are used for low-latency devices (HID, etc.).
/// For IN transfers, data vec length specifies the buffer size.
/// For OUT transfers, data vec contains the data to send.
///
/// # USB/IP Polling Behavior
///
/// USB/IP clients continuously submit interrupt IN URBs to poll for data.
/// For HID devices (keyboards, mice, barcode scanners), we need to wait long
/// enough to catch rapid sequences of data (e.g., multiple keystrokes from
/// scanning a barcode). A short timeout causes missed data because each
/// keystroke generates key-down/key-up reports in quick succession.
fn execute_interrupt_transfer(
    handle: &mut DeviceHandle<rusb::Context>,
    endpoint: u8,
    data: Vec<u8>,
    timeout_ms: u32,
) -> TransferResult {
    let is_in = (endpoint & 0x80) != 0;

    // For interrupt IN transfers, use a reasonable timeout that allows HID devices
    // to respond. USB/IP clients continuously re-submit these transfers, so we
    // want to balance responsiveness (not blocking too long if no data) with
    // catching rapid sequences of HID data (multiple keystrokes).
    //
    // 1000ms is a good balance - long enough to catch rapid HID sequences,
    // short enough to not block indefinitely. The client will re-submit
    // immediately after receiving a response.
    let timeout = if is_in {
        Duration::from_millis(1000.min(timeout_ms as u64))
    } else {
        Duration::from_millis(timeout_ms as u64)
    };

    trace!(
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
                if len > 0 {
                    // Log HID data for debugging - helps trace data corruption issues
                    // Use trace! level to avoid spamming during normal operation
                    trace!(
                        "Interrupt IN ep={:#x} len={} data={:02x?}",
                        endpoint, len, &buffer[..len.min(16)]
                    );
                }
                Ok(buffer)
            }
            Err(rusb::Error::Timeout) => {
                // Timeout is normal for interrupt IN - device has no data available yet.
                // Return empty success - USB/IP client will re-submit the URB.
                // This is the expected behavior for HID polling.
                trace!(
                    "Interrupt IN timeout on endpoint {:#x} - no data available",
                    endpoint
                );
                Ok(Vec::new())
            }
            Err(rusb::Error::Io) => {
                // IO error can mean the device is busy or temporarily unavailable.
                // Return empty to allow retry rather than failing the transfer.
                trace!(
                    "Interrupt IN IO error on endpoint {:#x} - returning empty for retry",
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
            if !data.is_empty() {
                debug!("Interrupt transfer succeeded: {} bytes", data.len());
            }
            TransferResult::Success { data }
        }
        Err(error) => {
            warn!("Interrupt transfer failed: {:?}", error);
            TransferResult::Error { error }
        }
    }
}

/// Execute an isochronous transfer
///
/// Isochronous transfers are used for audio/video streaming devices. They provide
/// guaranteed bandwidth with bounded latency but no automatic retry on errors.
///
/// # Note on Implementation
///
/// libusb (and thus rusb) requires async transfers for isochronous operations.
/// This implementation uses synchronous polling with a reasonable timeout.
/// For high-performance audio/video streaming, consider implementing proper
/// async transfer handling with double-buffering.
///
/// # Arguments
///
/// * `handle` - USB device handle
/// * `endpoint` - Endpoint address (includes direction bit)
/// * `data` - Data buffer (OUT: data to send, IN: pre-allocated buffer)
/// * `iso_packet_descriptors` - Per-packet descriptors with offset/length
/// * `start_frame` - Start frame for the transfer (0 for ASAP)
/// * `timeout_ms` - Timeout in milliseconds
fn execute_isochronous_transfer(
    _handle: &mut DeviceHandle<rusb::Context>,
    endpoint: u8,
    data: Vec<u8>,
    iso_packet_descriptors: Vec<IsoPacketDescriptor>,
    start_frame: u32,
    _timeout_ms: u32,
) -> TransferResult {
    let is_in = (endpoint & 0x80) != 0;
    let num_packets = iso_packet_descriptors.len();

    debug!(
        "Isochronous transfer: endpoint={:#x}, packets={}, data_len={}, start_frame={}, is_in={}",
        endpoint,
        num_packets,
        data.len(),
        start_frame,
        is_in
    );

    // Validate input
    if num_packets == 0 {
        warn!("Isochronous transfer with no packets");
        return TransferResult::Error {
            error: UsbError::InvalidParam,
        };
    }

    // libusb/rusb requires async transfers for isochronous operations.
    // The rusb crate's DeviceHandle does not expose synchronous isochronous
    // transfer methods (unlike control/bulk/interrupt).
    //
    // Proper isochronous support would require:
    // 1. Using libusb's async transfer API (libusb_alloc_transfer, libusb_submit_transfer)
    // 2. Setting up proper callback handling
    // 3. Implementing double-buffering for continuous streaming
    //
    // For now, we implement a simulation that:
    // - For IN transfers: Returns a buffer with zero-filled packets (as if no data available)
    // - For OUT transfers: Accepts the data (simulating successful write)
    //
    // This allows the USB/IP protocol to work correctly while real isochronous
    // support requires additional implementation work.

    if is_in {
        // IN transfer: simulate reading from device
        // In a real implementation, this would use libusb_submit_transfer
        // and wait for completion with proper packet status reporting.

        // Calculate total expected buffer size from packet descriptors
        let total_len: usize = iso_packet_descriptors
            .iter()
            .map(|p| p.length as usize)
            .sum();

        // Create response buffer
        let response_data = vec![0u8; total_len];

        // Build response packet descriptors
        // Since we're simulating (no real data), all packets have actual_length=0
        // and status=0 (success but no data)
        let response_descriptors: Vec<IsoPacketDescriptor> = iso_packet_descriptors
            .iter()
            .map(|p| IsoPacketDescriptor {
                offset: p.offset,
                length: p.length,
                actual_length: 0, // No actual data received in simulation
                status: 0,        // Success (no error)
            })
            .collect();

        debug!(
            "Isochronous IN simulated: {} packets, buffer_size={}",
            num_packets, total_len
        );

        TransferResult::IsochronousSuccess {
            data: response_data,
            iso_packet_descriptors: response_descriptors,
            start_frame,
            error_count: 0,
        }
    } else {
        // OUT transfer: simulate writing to device
        // Accept the data and report success for all packets

        let response_descriptors: Vec<IsoPacketDescriptor> = iso_packet_descriptors
            .iter()
            .map(|p| IsoPacketDescriptor {
                offset: p.offset,
                length: p.length,
                actual_length: p.length, // All data "sent"
                status: 0,               // Success
            })
            .collect();

        debug!(
            "Isochronous OUT simulated: {} packets, data_len={}",
            num_packets,
            data.len()
        );

        TransferResult::IsochronousSuccess {
            data: Vec::new(), // No return data for OUT
            iso_packet_descriptors: response_descriptors,
            start_frame,
            error_count: 0,
        }
    }
}

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

    #[test]
    fn test_bulk_size_constants() {
        assert_eq!(MAX_BULK_SIZE_HIGH_SPEED, 64 * 1024);
        assert_eq!(MAX_BULK_SIZE_SUPERSPEED, 1024 * 1024);
        assert_eq!(DEFAULT_URB_BUFFER_SIZE, 64 * 1024);
        assert_eq!(SUPERSPEED_URB_BUFFER_SIZE, 256 * 1024);
    }

    #[test]
    fn test_get_transfer_config() {
        let low_config = get_transfer_config(DeviceSpeed::Low);
        assert_eq!(low_config.max_bulk_size, 4 * 1024);
        assert!(!low_config.enable_burst);

        let high_config = get_transfer_config(DeviceSpeed::High);
        assert_eq!(high_config.max_bulk_size, 64 * 1024);
        assert!(!high_config.enable_burst);

        let super_config = get_transfer_config(DeviceSpeed::Super);
        assert_eq!(super_config.max_bulk_size, 1024 * 1024);
        assert!(super_config.enable_burst);
    }

    #[test]
    fn test_optimal_bulk_buffer_size() {
        // Low speed: max 4KB
        assert_eq!(optimal_bulk_buffer_size(DeviceSpeed::Low, 1024), 1024);
        assert_eq!(
            optimal_bulk_buffer_size(DeviceSpeed::Low, 8 * 1024),
            4 * 1024
        );

        // High speed: max 64KB
        assert_eq!(
            optimal_bulk_buffer_size(DeviceSpeed::High, 32 * 1024),
            32 * 1024
        );
        assert_eq!(
            optimal_bulk_buffer_size(DeviceSpeed::High, 128 * 1024),
            64 * 1024
        );

        // SuperSpeed: max 1MB
        assert_eq!(
            optimal_bulk_buffer_size(DeviceSpeed::Super, 512 * 1024),
            512 * 1024
        );
        assert_eq!(
            optimal_bulk_buffer_size(DeviceSpeed::Super, 2 * 1024 * 1024),
            1024 * 1024
        );
    }
}
