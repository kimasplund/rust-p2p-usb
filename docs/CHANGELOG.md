# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

#### Server-Side Integrations
- **Rate limiter integration** - Bandwidth limiting enforced on transfers in `connection.rs`
  - Uses atomic `try_consume()` with `rollback()` on failure
  - Prevents bandwidth exhaustion from aggressive clients
- **Notification aggregator** - Batches rapid device events for TUI responsiveness
  - Prevents UI flickering during rapid hotplug sequences
  - Configurable aggregation window
- **Server metrics tracking** - Per-client and per-device statistics
  - TX/RX bytes, latency measurements, active transfer counts
  - Integrated into TUI display

#### Client-Side Integrations
- **Health monitoring** (`health.rs`) - Connection health tracking
  - RTT measurement with min/current/max statistics
  - Connection quality assessment (Good <50ms, Fair 50-150ms, Poor >150ms)
  - Heartbeat tracking and connection state (Connecting/Connected/Degraded/Disconnected)
  - Factory pattern: `create_health_monitor()` for consistent initialization
- **USB 3.0 helpers** - Speed-aware port allocation
  - `port_range_for_speed()` returns correct VHCI port ranges (HS: 0-7, SS: 8-15)
  - `is_superspeed_port()`, `is_high_speed_port()` helpers
  - `optimal_urb_buffer_size()` for speed-appropriate buffer sizing
- **Bulk device cleanup** - `detach_all_from_server()` for clean disconnect handling
- **Health metrics TUI display** - Shows RTT, quality, and heartbeat counts per server

#### Common Crate Enhancements
- **Enhanced rate limiter** - Atomic operations for thread-safe bandwidth limiting
  - `try_consume()` returns success/failure without blocking
  - `rollback()` for undoing consumption on transfer failure
- **Transfer metrics helpers**
  - `record_transfer()` for recording transfer statistics
  - `rolling_window_duration()` for windowed throughput calculation
  - `SAMPLE_INTERVAL_MS` made public for consistent timing
- **Protocol SIZE constants** - Compile-time validated via static assertions

#### Configuration Enhancements
- **Multi-server config** with `all_servers()` merging
  - Combines legacy `approved_servers` with new `[[servers.configured]]` format
  - Configured servers take precedence over legacy entries
- **AutoConnectMode enum** - Manual, Auto, AutoWithDevices
- GitHub Actions CI workflow for automated testing and building
- **Multi-server architecture** with GlobalDeviceId for unique device identification across servers
- **Server name lookup** - Connect using friendly names (`--connect pi5-home`) instead of 64-char EndpointIds
- **Per-device auto_attach filtering** - Granular control over which devices to auto-attach per server:
  - Pattern formats: `"vid:pid"` (exact), `"vid:*"` (vendor wildcard), `"ProductName"` (substring)
  - Configurable per server in `[[servers.configured]]` sections
- Device hotplug auto-attach based on configured filters

### Changed
- Config default path changed from `~/.config/rust-p2p-usb/` to `~/.config/p2p-usb/` for consistency
- Device listing now shows `[auto]`/`[skip]` status based on auto_attach configuration
- `handle_notifications` now auto-attaches new devices matching filters (not just previously attached)
- **TUI enhanced** - Server TUI now displays TX/RX bytes, latency, throughput metrics
- **Reconnection policy** - `max_retries` now used in reconnection logic
- **Response conversion optimized** - Simple vs full ISO support paths

### Fixed
- Configuration file discovery (was looking in wrong directory)
- Hot-plug detection on client now works via notification aggregator

---

## [0.1.0] - 2026-01-05

### Added

#### Core Infrastructure
- Cargo workspace with 4 crates: `protocol`, `common`, `server`, `client`
- Rust 2024 edition with minimum version 1.90
- Comprehensive error handling with `anyhow` and `thiserror`
- Structured logging with `tracing` and `tracing-subscriber`

#### Protocol Library (`crates/protocol`)
- Type-safe message definitions for P2P USB communication
- `Message`, `MessagePayload` enum with all protocol message types
- `DeviceInfo`, `DeviceId`, `DeviceHandle` types
- `UsbRequest`, `UsbResponse` for transfer operations
- `TransferType` enum: Control, Bulk, Interrupt transfers
- Error types: `AttachError`, `DetachError`, `UsbError`
- Protocol versioning with `ProtocolVersion` struct
- Postcard serialization codec (efficient binary format)
- Serialization benchmarks for performance validation

#### Common Library (`crates/common`)
- Async channel bridge (`UsbBridge`, `UsbWorker`) for USB thread communication
- `UsbCommand` enum: ListDevices, AttachDevice, DetachDevice, SubmitTransfer, Shutdown
- `UsbEvent` enum: DeviceArrived, DeviceLeft, TransferComplete
- ALPN protocol identifier for Iroh connections
- Logging setup utilities

#### Server (`crates/server`)
- USB device enumeration via rusb
- Hot-plug detection using libusb hotplug callbacks
- Dedicated USB worker thread with event loop
- USB device wrapper with cached descriptors
- Kernel driver detachment and interface claiming
- Control, Bulk, and Interrupt transfer support
- Iroh P2P server with EndpointId-based authentication
- Client allowlist enforcement
- Per-client connection handling with QUIC streams
- TOML configuration file support
- CLI with clap: `--config`, `--service`, `--list-devices`, `--log-level`
- Systemd service mode with sd-notify integration
- Watchdog support for systemd
- Graceful shutdown handling
- TUI module scaffolding (not yet functional)

#### Client (`crates/client`)
- Iroh P2P client with server connection management
- Server allowlist enforcement
- Device listing from remote servers
- Device attachment/detachment operations
- USB transfer submission to remote devices
- DeviceProxy for transparent remote device access
- Virtual USB device creation via vhci_hcd (Linux)
- USB/IP protocol implementation:
  - TCP socket-based communication
  - OP_REQ_IMPORT/OP_REP_IMPORT handshake
  - CMD_SUBMIT message handling
  - CMD_UNLINK message handling (transfer cancellation)
  - RET_SUBMIT/RET_UNLINK responses
  - Proper USB speed code mapping
- Port bitmap allocation:
  - High-speed ports (0-7) for USB 2.0 and below
  - Super-speed ports (8-15) for USB 3.0+
- TOML configuration file support
- CLI with clap: `--config`, `--connect`, `--log-level`
- Auto-connect to approved servers
- TUI module scaffolding (not yet functional)

#### Documentation
- Comprehensive ARCHITECTURE.md with system design
- IMPLEMENTATION_ROADMAP.md with 10-phase plan
- SYSTEMD.md with service configuration guide
- VHCI_INVESTIGATION.md and VHCI_PROGRESS.md
- CROSS_PLATFORM_STRATEGY.md
- COMPRESSION_DESIGN.md for future optimization
- DIAGRAMS.md with ASCII architecture diagrams
- REASONING_REPORT.md with design decisions

### Changed

#### Iroh Upgrade (0.28 -> 0.95)
- Updated to iroh 0.95 (January 2025 release)
- Migrated from `NodeId` to `EndpointId` (PublicKey)
- Added `endpoint.online()` for connection readiness
- Updated ALPN configuration for new API
- Fixed connection acceptance pattern

#### USB/IP Implementation
- Refactored from Unix sockets to TCP sockets for vhci_hcd compatibility
- Implemented proper USB/IP import protocol handshake
- Fixed USB speed code mapping (USB_SPEED_SUPER = 5, not 4)
- Corrected port range allocation for device speeds
- Added CMD_UNLINK support for transfer cancellation

#### USB Device Management
- Added automatic kernel driver detachment on device open
- Added interface claiming for exclusive device access
- Added kernel driver reattachment on device close
- Improved error handling for permission issues

### Fixed
- USB device initialization: proper kernel driver detachment and interface claiming
- USB/IP port allocation: correct port ranges for USB 2.0 vs USB 3.0+ devices
- USB/IP speed codes: USB_SPEED_SUPER is 5, USB_SPEED_WIRELESS is 4
- Socket bridge: use TCP sockets instead of Unix domain sockets
- Iroh endpoint: wait for online status before accepting connections

### Security
- EndpointId allowlist for both server and client
- End-to-end encryption via Iroh QUIC (TLS 1.3)
- Per-device sharing granularity on server
- Minimal privileges support via udev rules

### Known Issues
- Virtual USB enumeration may stall in some cases
- Isochronous transfers not supported (infrastructure present but disabled)
- macOS/Windows client not implemented (stubs only)

---

## Version History

- **0.1.0** (2026-01-05): Initial alpha release with core functionality
- Project started: 2025-10-31

---

## Migration Notes

### Upgrading from Development Builds

If you were using development builds before v0.1.0:

1. **Configuration files**: Format unchanged, but verify `approved_clients`/`approved_servers` use new EndpointId format (64-character hex)

2. **Iroh EndpointIds**: If you saved NodeIds from iroh 0.28, they need to be regenerated with iroh 0.95

3. **vhci_hcd**: Ensure kernel module is loaded: `sudo modprobe vhci-hcd`

4. **Dependencies**: Run `cargo update` to get latest compatible versions

---

## Links

- [Repository](https://github.com/kimasplund/rust-p2p-usb)
- [Documentation](./docs/)
- [Architecture](./docs/ARCHITECTURE.md)
- [Roadmap](./docs/IMPLEMENTATION_ROADMAP.md)
