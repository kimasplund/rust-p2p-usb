# Phase 1 Implementation Guide: Linux USB/IP Completion

## Overview

Complete the Linux USB/IP implementation by adding the OP_REQ_IMPORT handshake that vhci_hcd expects.

**Current Status**: 70% complete - devices attach and enumeration begins, but kernel doesn't send CMD_SUBMIT messages

**Blocker**: vhci_hcd expects an active USB/IP protocol session with completed handshake

**Solution**: Implement USB/IP import protocol before passing socket to vhci_hcd

**Timeline**: 3-5 days

**Confidence**: 95%

---

## Root Cause Analysis

From your VHCI_PROGRESS.md:
```
vhci_hcd sysfs attach interface expects:
1. An already-established TCP connection
2. With USB/IP import protocol handshake completed
3. Socket FD passed to vhci should have active USB/IP session

What we provide: Raw Unix socketpair with no prior protocol state

Kernel behavior:
- Accepts the socket FD ✅
- Marks device as attached ✅
- Begins enumeration (assigns device number) ✅
- Does NOT send USB/IP protocol messages ❌ (expects handshake first)
```

---

## USB/IP Import Protocol

### Standard usbip attach flow:

```
1. usbip attach connects to remote server (TCP)
2. Sends OP_REQ_IMPORT message:
   - Version: 0x0111
   - Command: OP_REQ_IMPORT (0x8003)
   - Bus ID: "1-2" (device bus ID)
3. Server responds OP_REP_IMPORT:
   - Status: 0 (success)
   - Device info (descriptors, speed, etc.)
4. Socket is now in "imported" state
5. usbip passes socket FD to vhci_hcd via sysfs
6. vhci_hcd begins sending CMD_SUBMIT messages
7. Client receives CMD_SUBMIT, processes, returns RET_SUBMIT
```

### What you need to implement:

Since you're client and server (socket bridge), simulate the handshake:

```
SocketBridge::new():
  1. Create socketpair ✅ (already done)
  2. [NEW] On bridge side: send OP_REQ_IMPORT to self
  3. [NEW] On bridge side: respond with OP_REP_IMPORT
  4. [NEW] Mark socket as "imported" state
  5. Extract vhci FD ✅ (already done)
  6. Pass FD to vhci_hcd ✅ (already done)
  7. Start bridge to handle CMD_SUBMIT ✅ (already done)
```

---

## Implementation Steps

### Step 1: Add USB/IP Import Protocol Messages

**File**: `crates/client/src/virtual_usb/usbip_protocol.rs`

**Add these structures** (after existing UsbIpCommand):

```rust
/// USB/IP import/export commands
pub const OP_REQ_IMPORT: u16 = 0x8003;
pub const OP_REP_IMPORT: u16 = 0x0003;

/// OP_REQ_IMPORT message (32 bytes)
#[derive(Debug, Clone)]
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

impl UsbIpRepImport {
    pub fn from_device_info(info: &DeviceInfo, busid: &str) -> Self {
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

fn map_device_speed_to_u32(speed: DeviceSpeed) -> u32 {
    match speed {
        DeviceSpeed::Low => 1,
        DeviceSpeed::Full => 2,
        DeviceSpeed::High => 3,
        DeviceSpeed::Super => 5,
        DeviceSpeed::SuperPlus => 6,
    }
}
```

---

### Step 2: Implement Handshake in SocketBridge

**File**: `crates/client/src/virtual_usb/socket_bridge.rs`

**Modify SocketBridge::new()** to perform handshake:

```rust
impl SocketBridge {
    pub async fn new(
        device_proxy: Arc<DeviceProxy>,
        devid: u32,
        port: u8,
    ) -> Result<(Self, RawFd)> {
        // Create socketpair
        let (vhci_fd, bridge_fd) = socketpair(
            AddressFamily::Unix,
            SockType::Stream,
            None,
            SockFlag::empty(),
        )
        .context("Failed to create Unix socketpair")?;

        // Get device info for handshake
        let device_info = device_proxy.device_info();
        
        // [NEW] Perform USB/IP import handshake
        Self::perform_import_handshake(&bridge_fd, device_info, devid, port)
            .await
            .context("Failed to perform USB/IP import handshake")?;

        // Convert bridge FD to UnixStream
        let vhci_fd_raw = vhci_fd.into_raw_fd();
        let bridge_stream = unsafe { UnixStream::from_raw_fd(bridge_fd.into_raw_fd()) };
        
        // ... rest of existing code ...
        
        Ok((bridge, vhci_fd_raw))
    }
    
    /// Perform USB/IP import handshake over socket
    ///
    /// This simulates the OP_REQ_IMPORT / OP_REP_IMPORT exchange that
    /// normally happens between usbip client and server. Since we control
    /// both ends of the socket, we perform the handshake synchronously.
    async fn perform_import_handshake(
        socket_fd: &OwnedFd,
        device_info: &DeviceInfo,
        devid: u32,
        port: u8,
    ) -> Result<()> {
        use std::io::Write;
        use std::os::unix::io::AsRawFd;
        
        debug!("Performing USB/IP import handshake for device {} on port {}", devid, port);
        
        // Create bus ID (format: "port-devid")
        let busid = format!("{}-{}", port, devid);
        
        // Send OP_REQ_IMPORT (client → server)
        let req_import = UsbIpReqImport::new(&busid);
        let mut req_buf = Vec::new();
        req_import.write_to(&mut req_buf)
            .context("Failed to serialize OP_REQ_IMPORT")?;
        
        // Write to socket (this is normally done over TCP)
        let socket_raw = socket_fd.as_raw_fd();
        let written = unsafe {
            libc::write(
                socket_raw,
                req_buf.as_ptr() as *const libc::c_void,
                req_buf.len(),
            )
        };
        
        if written < 0 {
            return Err(anyhow!("Failed to write OP_REQ_IMPORT to socket"));
        }
        
        debug!("Sent OP_REQ_IMPORT: {} bytes", written);
        
        // Send OP_REP_IMPORT (server → client)
        let rep_import = UsbIpRepImport::from_device_info(device_info, &busid);
        let mut rep_buf = Vec::new();
        rep_import.write_to(&mut rep_buf)
            .context("Failed to serialize OP_REP_IMPORT")?;
        
        let written = unsafe {
            libc::write(
                socket_raw,
                rep_buf.as_ptr() as *const libc::c_void,
                rep_buf.len(),
            )
        };
        
        if written < 0 {
            return Err(anyhow!("Failed to write OP_REP_IMPORT to socket"));
        }
        
        debug!("Sent OP_REP_IMPORT: {} bytes", written);
        
        info!(
            "USB/IP import handshake complete for device {} ({})",
            devid, busid
        );
        
        Ok(())
    }
}
```

---

### Step 3: Test with Kernel Tracing

Enable USB/IP kernel debugging:

```bash
# Enable usbip debug
echo 'module usbip_core +p' | sudo tee /sys/kernel/debug/dynamic_debug/control
echo 'module vhci_hcd +p' | sudo tee /sys/kernel/debug/dynamic_debug/control

# Watch kernel logs
sudo dmesg -wH | grep -i 'vhci\|usbip'
```

Run your client:
```bash
cargo build --release
sudo ./target/release/p2p-usb-client --connect <server-node-id>
```

**Expected kernel logs** (after handshake):
```
vhci_hcd: Device attached
usb 5-1: new high-speed USB device number 3 using vhci_hcd
vhci_hcd: Receiving CMD_SUBMIT for port 0    <-- NEW (should appear now)
usb 5-1: New USB device found, idVendor=1234, idProduct=5678
```

---

### Step 4: Validate Enumeration

Check if device appears:
```bash
lsusb
# Should show your virtual device

# Check vhci status
cat /sys/devices/platform/vhci_hcd.0/status
# Port should show "006" or higher (fully enumerated)
```

If successful:
- lsusb shows virtual device
- dmesg shows "New USB device found"
- No EOF errors in client logs

---

## Testing Checklist

- [ ] Handshake messages compile (OP_REQ_IMPORT, OP_REP_IMPORT)
- [ ] SocketBridge::new() performs handshake before passing FD
- [ ] Kernel debug logs show CMD_SUBMIT messages
- [ ] Device enumeration completes (status = 006)
- [ ] lsusb shows virtual device
- [ ] No EOF errors in socket bridge
- [ ] Control transfers work (descriptor requests)
- [ ] Bulk transfers work (if applicable)
- [ ] Multiple devices can attach

---

## Debugging Tips

### If handshake fails:

1. **Check socket state**: Ensure socket is still open after handshake
2. **Verify message format**: Use hexdump to inspect OP_REQ/REP_IMPORT bytes
3. **Kernel tracing**: Check if vhci_hcd receives handshake messages
4. **Compare with usbip**: Run `strace usbip attach` to see what real client does

### If kernel still doesn't send CMD_SUBMIT:

1. **Socket FD ownership**: Ensure FD not closed after handshake
2. **Message order**: Handshake must complete before passing FD to vhci_hcd
3. **Kernel state**: Check `vhci_hcd.0/status` - port should show device attached

### Common issues:

- **EOF on socket after handshake**: FD closed prematurely, check ownership
- **Kernel rejects attach**: Handshake not recognized, check message format
- **Device number assigned but no enumeration**: Handshake incomplete or wrong

---

## Reference

**Linux kernel source**:
- `tools/usb/usbip/src/usbip_attach.c` - Reference implementation
- `drivers/usb/usbip/usbip_common.h` - Protocol definitions
- `drivers/usb/usbip/vhci_sysfs.c` - Sysfs attach handling

**Your existing code**:
- `crates/client/src/virtual_usb/usbip_protocol.rs` - Already has CMD_SUBMIT/RET_SUBMIT
- `crates/client/src/virtual_usb/socket_bridge.rs` - Socket bridge implementation
- `crates/client/src/virtual_usb/linux.rs` - VHCI attachment logic

---

## Timeline

- **Day 1**: Implement OP_REQ/REP_IMPORT structures (Step 1)
- **Day 2**: Add handshake to SocketBridge::new() (Step 2)
- **Day 3**: Test with kernel tracing, debug issues (Step 3)
- **Day 4**: Validate enumeration, test transfers (Step 4)
- **Day 5**: Test with multiple devices, cleanup

---

## Success Criteria

Phase 1 is complete when:
- ✅ Devices attach and stay connected
- ✅ Kernel sends CMD_SUBMIT messages
- ✅ Device enumeration completes
- ✅ lsusb shows virtual devices
- ✅ Control transfers work
- ✅ Bulk/interrupt transfers work
- ✅ Multiple devices work simultaneously

---

**Confidence**: 95% - This is the correct solution based on USB/IP protocol documentation and your current progress.
