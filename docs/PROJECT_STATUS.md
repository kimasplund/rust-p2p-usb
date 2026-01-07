# rust-p2p-usb Project Status

**Last Updated**: 2026-01-05
**Version**: 0.1.0 (Development)
**Stage**: Alpha - Feature Development

---

## Overview

rust-p2p-usb is a high-performance Rust application for secure peer-to-peer USB device sharing over the internet using Iroh networking. The project enables access to USB devices connected to a remote server (like a Raspberry Pi) from anywhere as if they were plugged in locally.

---

## Phase Completion Summary

| Phase | Description | Status | Completion |
|-------|-------------|--------|------------|
| Phase 0 | Project Setup | Complete | 100% |
| Phase 1 | Protocol Foundation | Complete | 100% |
| Phase 2 | USB Subsystem (Server) | Complete | 95% |
| Phase 3 | Network Layer (Server) | Complete | 95% |
| Phase 4 | Network Layer (Client) | Complete | 95% |
| Phase 5 | Virtual USB (Client) | Complete | 90% |
| Phase 6 | TUI (Server & Client) | Complete | 90% |
| Phase 7 | Configuration & CLI | Complete | 95% |
| Phase 8 | Systemd Integration | Complete | 90% |
| Phase 9 | Integration Testing | In Progress | 40% |
| Phase 10 | Documentation & Release | In Progress | 50% |

**Overall Project Completion: ~75%**

---

## What's Working

### Core Functionality

- **Iroh P2P Networking (iroh 0.95)**
  - Server creates and manages Iroh endpoint
  - Client connects to server via EndpointId
  - Uses `endpoint.online()` for connection readiness
  - ALPN protocol identifier for version matching
  - End-to-end encryption via QUIC/TLS 1.3

- **USB Device Management (Server)**
  - Device enumeration via rusb
  - Hot-plug detection
  - Kernel driver detachment for device access
  - Interface claiming for exclusive access
  - Kernel driver reattachment on close
  - Control, Bulk, and Interrupt transfers

- **Virtual USB Devices (Client - Linux)**
  - USB/IP vhci_hcd integration
  - TCP socket-based USB/IP protocol
  - Port bitmap allocation (HS ports 0-7, SS ports 8-15)
  - USB/IP import protocol handshake
  - CMD_SUBMIT and CMD_UNLINK message handling
  - RET_SUBMIT and RET_UNLINK responses

- **Protocol Layer**
  - Type-safe message definitions with serde
  - Postcard serialization (efficient binary format)
  - Protocol versioning support
  - Request/response matching via RequestId

- **Configuration**
  - TOML configuration files for server and client
  - CLI argument parsing with clap
  - Allowlist support for client/server EndpointIds
  - Log level configuration

- **Systemd Integration**
  - Service mode (headless operation)
  - sd-notify integration for ready status
  - Watchdog support
  - Graceful shutdown handling

### Recent Improvements (January 2025)

1. **Iroh 0.95 Upgrade**: Updated from iroh 0.28 to 0.95 with new API
2. **endpoint.online() Support**: Proper connection readiness detection
3. **Port Bitmap Allocation**: Correct port range handling for USB 2.0/3.0+
4. **CMD_UNLINK Support**: Proper handling of USB transfer cancellation
5. **TCP Socket Bridge**: Refactored from Unix sockets to TCP for USB/IP
6. **Kernel Driver Management**: Auto-detach/reattach for shared devices

---

## Known Issues

### Critical Issues

1. **Virtual USB Enumeration Stalls**
   - **Symptom**: Device attaches to vhci_hcd but enumeration may not complete
   - **Status**: Under investigation
   - **Workaround**: Retry attach operation

### Medium Priority Issues



3. **Isochronous Transfers Not Supported**
   - **Affected devices**: Webcams, audio devices
   - **Status**: Deferred to v0.2

### Low Priority Issues

4. **macOS/Windows Virtual USB**
   - **Status**: Stub implementations only
   - **Impact**: Client only works on Linux

5. **Hot-plug Detection on Client**
   - **Status**: Not implemented
   - **Impact**: Must restart client to see new server devices

---

## Dependencies Status

| Dependency | Version | Status |
|------------|---------|--------|
| iroh | 0.95 | Current (Jan 2025) |
| tokio | 1.48 | Current |
| rusb | 0.9 | Current |
| ratatui | 0.29 | Current |
| postcard | 1.0 | Current |
| nix | 0.29 | Current |
| clap | 4.5 | Current |

---

## File Structure

```
rust-p2p-usb/
├── Cargo.toml                    # Workspace (edition 2024)
├── crates/
│   ├── protocol/                 # Protocol library [100%]
│   │   ├── src/messages.rs       # Message types
│   │   ├── src/codec.rs          # Postcard serialization
│   │   └── src/types.rs          # Shared types
│   │
│   ├── common/                   # Shared utilities [100%]
│   │   ├── src/channel.rs        # USB bridge channels
│   │   ├── src/logging.rs        # Tracing setup
│   │   └── src/alpn.rs           # Protocol identifier
│   │
│   ├── server/                   # Server binary [90%]
│   │   ├── src/main.rs           # Entry point, CLI
│   │   ├── src/usb/              # USB management
│   │   │   ├── worker.rs         # USB thread
│   │   │   ├── device.rs         # Device wrapper
│   │   │   └── transfers.rs      # Transfer handling
│   │   ├── src/network/          # Iroh server
│   │   │   ├── server.rs         # Accept connections
│   │   │   └── connection.rs     # Client handling
│   │   ├── src/config.rs         # TOML config
│   │   ├── src/service.rs        # Systemd integration
│   │   └── src/tui/              # TUI (scaffolding)
│   │
│   └── client/                   # Client binary [85%]
│       ├── src/main.rs           # Entry point, CLI
│       ├── src/virtual_usb/      # Virtual USB devices
│       │   ├── linux.rs          # vhci_hcd integration
│       │   ├── socket_bridge.rs  # USB/IP socket bridge
│       │   └── usbip_protocol.rs # USB/IP wire protocol
│       ├── src/network/          # Iroh client
│       │   ├── client.rs         # Server connections
│       │   └── device_proxy.rs   # Remote device proxy
│       ├── src/config.rs         # TOML config
│       └── src/tui/              # TUI (scaffolding)
│
├── docs/                         # Documentation
├── systemd/                      # Service files
└── scripts/                      # Utility scripts
```

---

## Performance Targets

| Metric | Target | Current Status |
|--------|--------|----------------|
| USB Latency | 5-20ms | ~15-25ms (LAN) |
| Throughput | 80% USB 2.0 | Not measured |
| Memory | <50MB RSS | ~30MB |
| CPU (RPi4) | <5% idle | ~2% |

---

## Next Steps

### Immediate (v0.1.0 Release)

1. [ ] Stabilize virtual USB enumeration
2. [ ] Complete CHANGELOG.md
3. [ ] Add basic integration tests
4. [ ] Fix remaining clippy warnings
5. [ ] Update documentation

### Short Term (v0.2.0)

1. [ ] Polish TUI UX and add help screens
2. [ ] Add interrupt transfer support
3. [ ] Performance benchmarking
4. [ ] Reconnection logic with backoff

### Medium Term (v0.3.0)

1. [ ] Device filtering by VID/PID
2. [ ] Performance metrics in TUI
3. [ ] Compression for control messages
4. [ ] macOS client support research

---

## Testing Instructions

### Prerequisites

```bash
# Install dependencies (Ubuntu/Debian)
sudo apt-get install libusb-1.0-0-dev

# Load vhci_hcd module (client)
sudo modprobe vhci-hcd

# Build project
cargo build --release
```

### Server Testing

```bash
# List USB devices
cargo run -p server -- --list-devices

# Run in service mode
cargo run -p server -- --service

# Run with debug logging
RUST_LOG=debug cargo run -p server
```

### Client Testing

```bash
# Connect to server (requires server EndpointId)
cargo run -p client -- --connect <server-endpoint-id>

# Run in headless mode
RUST_LOG=debug cargo run -p client
```

### Verifying Virtual USB

```bash
# Check vhci_hcd status (as root)
cat /sys/devices/platform/vhci_hcd.0/status

# List USB devices (should show virtual device)
lsusb

# Check kernel logs
dmesg | tail -20
```

---

## Contributing

See `CLAUDE.md` for development workflow and agent usage.

Key files for contributors:
- `/docs/ARCHITECTURE.md` - System design
- `/docs/IMPLEMENTATION_ROADMAP.md` - Phase details
- `/crates/protocol/src/messages.rs` - Protocol reference

---

## Contact

- Issues: GitHub Issues
- Documentation: `/docs/` directory
