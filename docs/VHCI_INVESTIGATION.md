# vhci_hcd Socket Bridge Investigation

## Date: 2025-10-31

## Summary

Successfully attached 6 virtual USB devices to vhci_hcd using sysfs interface. Devices show in vhci_hcd status with state 005 (attached). Kernel begins USB enumeration but fails to complete. Socket bridge receives EOF when reading USB/IP messages.

## Kernel Behavior

### Successful Attachment
```
[30160.269788] vhci_hcd vhci_hcd.0: pdev(0) rhport(0) sockfd(17)
[30160.269795] vhci_hcd vhci_hcd.0: devid(5) speed(3) speed_str(high-speed)
[30160.269801] vhci_hcd vhci_hcd.0: Device attached
```

### Enumeration Started
```
[30160.483963] usb 5-1: new high-speed USB device number 2 using vhci_hcd
```

**Note**: No follow-up enumeration messages (device found, product info, etc.)

### vhci_hcd Status
```
hub port sta spd dev      sockfd local_busid
hs  0000 005 000 00000000 000000 0-0      # Device attached, port 0
hs  0001 005 000 00000000 000000 0-0      # Device attached, port 1
hs  0002 005 000 00000000 000000 0-0      # Device attached, port 2
ss  0008 005 000 00000000 000000 0-0      # Device attached, port 8
ss  0009 005 000 00000000 000000 0-0      # Device attached, port 9
ss  0010 005 000 00000000 000000 0-0      # Device attached, port 10
```

Status 005 = device attached, enumeration should begin

## Socket Bridge Behavior

### Socket Creation
1. Created TCP listener on `127.0.0.1:random_port`
2. Connected to listener creating socket pair
3. Passed connector FD to vhci_hcd via `std::mem::forget`
4. Used accepted socket in bridge

### Socket State
From lsof output:
```
p2p-usb-c 486097 root  17u  IPv4  1744182  TCP localhost:50010->localhost:39889 (ESTABLISHED)
p2p-usb-c 486097 root  18u  IPv4  1712002  TCP localhost:39889->localhost:50010 (ESTABLISHED)
```

Sockets ARE established and connected.

### Errors
```
ERROR p2p_usb_client::virtual_usb::socket_bridge: Failed to read USB/IP message: failed to fill whole buffer
```

`read_exact()` fails because socket returns EOF before reading full 48-byte USB/IP header.

## Root Cause

**The vhci_hcd driver is NOT sending USB/IP protocol messages on the socket FD we provided.**

### Why This Happens

The vhci_hcd sysfs attach interface was designed for USB/IP over network:
1. Client connects to remote USB/IP server over network
2. Client negotiates device import with server
3. Client receives connected socket with USB/IP protocol active
4. Client passes this socket to vhci_hcd
5. vhci_hcd uses existing connection to communicate with remote server

Our approach creates a local TCP socket pair, but vhci_hcd may not actively communicate over this socket in the same way. The kernel expects the socket to already have an active USB/IP protocol session running.

## Next Steps

### Option 1: Use Unix Domain Socketpair
Instead of TCP sockets, use `socketpair(AF_UNIX, SOCK_STREAM, 0)` which creates a true bidirectional socket pair. This may be closer to what vhci_hcd expects.

**Pros**: True socketpair, bidirectional, kernel familiar with this pattern
**Cons**: Already tried this approach previously (why did we move to TCP?)

### Option 2: Implement USB/IP Import Protocol
Before passing socket to vhci_hcd, implement the USB/IP import handshake protocol that normally happens over network. This may initialize the socket state correctly.

**Pros**: Matches expected vhci_hcd usage pattern
**Cons**: Complex, requires understanding full USB/IP import protocol

### Option 3: Use USB/IP Kernel Module Differently
Instead of sysfs attach, investigate other vhci_hcd interfaces or alternative USB virtualization approaches.

**Pros**: May find better-suited interface
**Cons**: Requires kernel module research, may not exist

### Option 4: Kernel Module Approach
Write a custom kernel module that implements virtual USB devices without USB/IP protocol layer.

**Pros**: Full control, no protocol overhead
**Cons**: Complex, requires kernel development, deployment challenges

## Technical Details

### USB/IP Protocol Expected by vhci_hcd

When kernel starts enumeration:
1. Send CMD_SUBMIT for GET_DESCRIPTOR (device descriptor)
2. Expect RET_SUBMIT with descriptor data
3. Send CMD_SUBMIT for GET_DESCRIPTOR (config descriptor)
4. Continue enumeration process

Our bridge waits to receive CMD_SUBMIT but kernel never sends it.

### Socket Bridge Implementation

Located in `crates/client/src/virtual_usb/socket_bridge.rs`:

```rust
async fn run(&self) -> Result<()> {
    while self.running.load(Ordering::Acquire) {
        // Blocks here waiting for USB/IP header (48 bytes)
        let (header, cmd, data) = match self.read_usbip_message().await {
            Ok(msg) => msg,
            Err(e) => {
                // Gets EOF error - socket has no data
                error!("Failed to read USB/IP message: {:#}", e);
                continue;
            }
        };
        // Never reaches here
    }
}
```

## Conclusion

The vhci_hcd sysfs attach succeeds at the kernel level, but the kernel does not actively use the socket FD for USB/IP communication as we expected. We need to either:

1. Fix the socket initialization to match what vhci_hcd expects
2. Implement missing USB/IP protocol handshake
3. Use a different approach for virtual USB device creation

**Recommendation**: Investigate USB/IP import protocol and Unix domain socketpair approach first, as these are closest to standard USB/IP usage patterns.
