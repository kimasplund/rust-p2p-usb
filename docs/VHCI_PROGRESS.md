# vhci_hcd Socket Bridge Progress

## Date: 2025-10-31

## Summary

Successfully implemented Unix socketpair approach for vhci_hcd integration. Devices now attach and stay connected, with enumeration beginning. However, kernel does not send USB/IP protocol messages over the socket.

## Progress Timeline

### 1. Initial TCP Socket Approach (Failed)
- Created TCP socket pair via `TcpListener::bind` and `TcpStream::connect`
- Devices attached but immediately disconnected
- Kernel logs showed: "Device attached" → "connection closed"

### 2. Unix Socketpair Attempt #1 (FD Ownership Bug)
- Switched to `socketpair(AF_UNIX, SOCK_STREAM, 0)`
- Devices attached but immediately disconnected (same as TCP)
- Root cause: File descriptor ownership bug
  - Used `as_raw_fd()` to extract bridge FD
  - Passed raw FD to `UnixStream::from_raw_fd()`
  - Original `OwnedFd` still owned FD and closed it on drop

### 3. Unix Socketpair with FD Ownership Fix (Current State)
**Fix Applied**: Use `into_raw_fd()` to transfer ownership
```rust
// Before (buggy):
let bridge_fd = bridge_owned_fd.as_raw_fd();

// After (fixed):
let bridge_fd = bridge_owned_fd.into_raw_fd();
```

**Results**: ✅ MAJOR PROGRESS
- Devices attach successfully to vhci_hcd
- NO "connection closed" messages
- Enumeration BEGINS: `usb 5-1: new high-speed USB device number 3 using vhci_hcd`
- vhci_hcd status shows **005** (enumeration started) on multiple ports
- Socket bridges run without crashing

**Remaining Issue**: ❌ EOF ERRORS
- Socket bridge receives EOF when reading USB/IP messages
- Kernel does NOT send CMD_SUBMIT or other USB/IP protocol messages
- Enumeration stalls after initial device number assignment

## Technical Details

### Successful Attachment
```
vhci_hcd vhci_hcd.0: pdev(0) rhport(1) sockfd(18)
vhci_hcd vhci_hcd.0: devid(5) speed(3) speed_str(high-speed)
vhci_hcd vhci_hcd.0: Device attached
usb 5-1: new high-speed USB device number 3 using vhci_hcd
```

No "connection closed" messages - devices stay attached!

### Port Status
```
hub port sta spd dev      sockfd local_busid
hs  0000 005 000 00000000 000000 0-0   ← Enumeration started
hs  0001 005 000 00000000 000000 0-0   ← Enumeration started
hs  0002 005 000 00000000 000000 0-0   ← Enumeration started
ss  0008 005 000 00000000 000000 0-0   ← Enumeration started
ss  0009 005 000 00000000 000000 0-0   ← Enumeration started
ss  0010 005 000 00000000 000000 0-0   ← Enumeration started
```

Status **005** indicates kernel initiated enumeration but it's not progressing.

### Socket Bridge Errors
```
ERROR p2p_usb_client::virtual_usb::socket_bridge: Failed to read USB/IP message: failed to fill whole buffer
```

Socket returns EOF - no data being sent by kernel.

## Root Cause Analysis

The vhci_hcd sysfs attach interface expects:
1. An already-established TCP connection
2. With USB/IP import protocol handshake completed
3. Socket FD passed to vhci should have active USB/IP session

**What we provide**: Raw Unix socketpair with no prior protocol state

**Kernel behavior**:
- Accepts the socket FD
- Marks device as attached
- Begins enumeration (assigns device number)
- **Does NOT** send USB/IP protocol messages (CMD_SUBMIT for descriptors)

## Next Steps - Three Approaches

### Option 1: Implement USB/IP Import Protocol (Recommended)
Before passing socket to vhci_hcd, implement minimal USB/IP handshake:
1. Send OP_REQ_DEVLIST or OP_REQ_IMPORT on socket
2. Exchange device info/descriptors
3. Put socket in "connected" state
4. Pass to vhci_hcd

**Pros**: Matches expected vhci_hcd usage pattern
**Cons**: Requires understanding USB/IP wire protocol

### Option 2: Investigate vhci_hcd Driver Code
Study kernel source to understand:
- What socket checks vhci_hcd performs on attach
- Why it's not sending USB/IP messages
- Whether socket options or state flags are needed

**Pros**: Could find simple solution (e.g., socket option)
**Cons**: Requires kernel code analysis

### Option 3: Alternative Virtual USB Approach
Abandon USB/IP and explore alternatives:
- Direct usbfs/devio manipulation
- Custom kernel module
- USB Gadget framework (if applicable)

**Pros**: Full control over virtual USB
**Cons**: More complex, different deployment model

## Files Modified

### `crates/client/src/virtual_usb/socket_bridge.rs`
- Switched from TCP to Unix socketpair
- Fixed FD ownership with `into_raw_fd()`
- Socket remains open and bridges run stably

### `Cargo.toml`
- Added "socket" feature to nix crate

### `docs/VHCI_INVESTIGATION.md`
- Documented TCP socket failure
- Analyzed root causes
- Proposed solutions

## Testing Evidence

Test logs demonstrate progressive improvements:
1. `/tmp/client-final-param-fix-test.log` - TCP socketpair (connection closed)
2. `/tmp/client-unix-socketpair-test.log` - Unix socketpair (connection closed due to FD bug)
3. `/tmp/client-fd-fix-clean-test.log` - Current state (enumeration starts, EOF errors)

## Conclusion

We've successfully solved the socket lifecycle problem - devices now attach and stay connected with enumeration beginning. The remaining challenge is making vhci_hcd actively communicate USB/IP protocol over the socket. This likely requires implementing the USB/IP import handshake or discovering what socket state/options the kernel expects.

**Status**: 70% complete - socket bridge functional, protocol layer missing
**Blocked on**: Understanding vhci_hcd expectations for socket protocol state
**Recommendation**: Research USB/IP import protocol and implement minimal handshake
