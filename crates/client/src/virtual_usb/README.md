# Virtual USB Device Layer - Phase 5

## Overview

This module implements virtual USB device creation on the client side, allowing remote USB devices to appear as if they were locally connected.

## Architecture

The virtual USB layer uses platform-specific implementations:

### Linux (USB/IP) - **IMPLEMENTED**

Uses the vhci_hcd (Virtual Host Controller Interface) kernel module to create virtual USB devices.

**Requirements:**
- vhci_hcd kernel module loaded (`sudo modprobe vhci-hcd`)
- Root privileges or appropriate udev rules
- Linux kernel 2.6.28+ (USB/IP support)

**How it works:**
1. Load vhci_hcd kernel module
2. Write to `/sys/devices/platform/vhci_hcd.X/attach` to attach devices
3. Device appears in `lsusb` and can be used by applications
4. All USB operations are forwarded to remote device via DeviceProxy

**Limitations:**
- Maximum 8 devices per vhci_hcd instance
- Requires kernel module support
- No isochronous transfers (Phase 1 limitation)

### macOS - **NOT IMPLEMENTED**

Stub implementation that returns errors.

**Future implementation options:**
- DriverKit (macOS 13+, recommended)
- IOKit (macOS 10-12, deprecated)
- VM-based workaround

### Windows - **NOT IMPLEMENTED**

Stub implementation that returns errors.

**Future implementation options:**
- libusb + UsbDk
- WinUSB + Custom Driver
- Windows Filter Driver

## Usage Example

```rust
use crate::virtual_usb::VirtualUsbManager;
use crate::network::device_proxy::DeviceProxy;
use std::sync::Arc;

// Create virtual USB manager
let mut manager = VirtualUsbManager::new().await?;

// Create device proxy (from network layer)
let device_proxy: Arc<DeviceProxy> = ...;

// Attach virtual device
let handle = manager.attach_device(device_proxy).await?;

// Device now appears in lsusb and can be used by applications

// Detach when done
manager.detach_device(handle).await?;
```

## Testing

### Unit Tests

Run unit tests (speed mapping, device ID generation):
```bash
cargo test -p client --bin p2p-usb-client
```

### Manual Testing (Linux Only)

Requires:
- vhci_hcd module loaded
- Server running with a USB device attached
- Root privileges or appropriate udev rules

```bash
# Load vhci_hcd module
sudo modprobe vhci-hcd

# Verify module loaded
ls /sys/devices/platform/vhci_hcd.0/

# Run client and attach device
sudo cargo run -p client

# In another terminal, verify device appears
lsusb
lsusb -v -d <vendor_id>:<product_id>

# Check vhci_hcd status
cat /sys/devices/platform/vhci_hcd.0/status
```

### Expected Output

```
$ cat /sys/devices/platform/vhci_hcd.0/status
hub port sta spd dev      sockfd local_busid
hs  0000 004 000 00000000 -1     0-0
hs  0001 006 003 00000001 -1     1-5  <-- Attached device
hs  0002 004 000 00000000 -1     0-0
...
```

## Implementation Details

### File Structure

- `mod.rs` - Platform-agnostic interface
- `linux.rs` - Linux USB/IP implementation
- `device.rs` - Virtual device state management
- `macos.rs` - macOS stub
- `windows.rs` - Windows stub

### Key Types

#### VirtualUsbManager

Platform-agnostic manager for virtual USB devices.

**Methods:**
- `new()` - Create manager, verify platform support
- `attach_device(device_proxy)` - Attach virtual device
- `detach_device(handle)` - Detach virtual device
- `list_devices()` - List attached devices

#### VirtualDevice

Represents a single virtual USB device.

**Responsibilities:**
- Maintain device state (handle, descriptor, port)
- Generate unique request IDs
- Forward USB operations to DeviceProxy
- Handle control, bulk, and interrupt transfers

#### LinuxVirtualUsbManager (Linux-specific)

Linux implementation using USB/IP.

**Key operations:**
- Find vhci_hcd device path
- Allocate VHCI ports (0-7)
- Write to sysfs (attach/detach)
- Map device speeds to USB/IP codes

### USB/IP Protocol Details

**Attach format:**
```
<port> <speed> <devid> <sockfd>
```

**Speed codes:**
- 1 = Low speed (1.5 Mbps)
- 2 = Full speed (12 Mbps)
- 3 = High speed (480 Mbps)
- 4 = SuperSpeed (5 Gbps)
- 5 = SuperSpeed+ (10 Gbps)

**Device ID:**
- Unique identifier (we use DeviceHandle value)
- Traditionally: `busnum << 16 | devnum`

**Socket FD:**
- `-1` for userspace implementation (our case)
- File descriptor for kernel implementation

## Error Handling

### Common Errors

1. **vhci_hcd not found**
   ```
   Error: vhci_hcd not found. Please load the kernel module: sudo modprobe vhci-hcd
   ```
   **Solution:** Load the vhci-hcd kernel module

2. **Permission denied**
   ```
   Error: Failed to open /sys/devices/platform/vhci_hcd.0/attach (requires root or appropriate udev rules)
   ```
   **Solution:** Run with sudo or setup udev rules

3. **No available ports**
   ```
   Error: No available VHCI ports (maximum 8 devices supported)
   ```
   **Solution:** Detach some devices or use multiple vhci_hcd instances

4. **Platform not supported**
   ```
   Error: macOS virtual USB support is not yet implemented.
   ```
   **Solution:** Use Linux for Phase 5

## Future Enhancements

1. **Port reuse** - Currently ports are not reused after detach
2. **Multiple vhci_hcd instances** - Support >8 devices
3. **macOS support** - Implement DriverKit-based solution
4. **Windows support** - Implement libusb+UsbDk solution
5. **Isochronous transfers** - Add support for webcams/audio devices
6. **Hot-plug handling** - Better handling of device disconnects

## Performance Considerations

### Latency

Virtual USB adds minimal overhead:
- USB/IP attach: ~1ms (one-time)
- Transfer forwarding: <0.1ms (in-process)
- Total latency dominated by network (Phase 4)

### Throughput

No significant throughput impact:
- USB/IP is kernel-level (no userspace bottleneck)
- Transfer data passed directly to DeviceProxy
- Expect 80-90% of USB 2.0 bandwidth (network dependent)

## Security Considerations

### Permissions

Linux USB/IP requires:
- Write access to `/sys/devices/platform/vhci_hcd.X/attach`
- Write access to `/sys/devices/platform/vhci_hcd.X/detach`

**Options:**
1. Run as root (simplest, least secure)
2. Setup udev rules (recommended)
3. Add user to specific group with sysfs access

### udev Rules Example

```
# /etc/udev/rules.d/99-vhci-hcd.rules
SUBSYSTEM=="vhci_hcd", MODE="0660", GROUP="usb-proxy"
```

Then add user to group:
```bash
sudo usermod -a -G usb-proxy $USER
```

### Attack Surface

Virtual USB devices have same security considerations as physical devices:
- Malicious device descriptors can exploit kernel drivers
- Buffer overflows in USB driver stack
- DMA attacks (if device has DMA access)

**Mitigations:**
- Device allowlist (only attach known devices)
- Per-device approval in TUI
- Kernel security hardening (latest kernel patches)

## References

- [Linux USB/IP Documentation](https://www.kernel.org/doc/readme/tools-usb-usbip-README)
- [VHCI_HCD Driver](https://www.kernel.org/doc/Documentation/usb/usbip_protocol.txt)
- [USB Specification](https://www.usb.org/documents)
- Architecture Document: `/home/kim-asplund/projects/rust-p2p-usb/docs/ARCHITECTURE.md` Section 4.4
- Implementation Roadmap: Phase 5

## Contact

For questions or issues with the virtual USB layer, please refer to:
- Architecture document for design decisions
- Roadmap for implementation timeline
- Root cause analyzer agent for debugging
