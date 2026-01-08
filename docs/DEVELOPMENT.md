# rust-p2p-usb Development Guide

**Last Updated**: 2026-01-08
**Version**: 0.1.0
**Confidence**: 90%

This guide covers setting up a development environment, understanding the codebase architecture, and contributing to rust-p2p-usb.

---

## Table of Contents

1. [Development Environment Setup](#development-environment-setup)
2. [Project Structure](#project-structure)
3. [Building and Testing](#building-and-testing)
4. [Code Style and Conventions](#code-style-and-conventions)
5. [Architecture Overview](#architecture-overview)
6. [Adding USB Transfer Types](#adding-usb-transfer-types)
7. [Adding Protocol Messages](#adding-protocol-messages)
8. [Debugging](#debugging)
9. [Contributing Guidelines](#contributing-guidelines)

---

## Development Environment Setup

### Prerequisites

1. **Rust Toolchain** (1.90+ for Edition 2024):

   ```bash
   # Install Rust
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

   # Add to PATH
   source ~/.cargo/env

   # Verify
   rustc --version  # Should be >= 1.90.0
   ```

2. **System Dependencies**:

   ```bash
   # Ubuntu/Debian
   sudo apt-get update
   sudo apt-get install -y \
       libusb-1.0-0-dev \
       pkg-config \
       build-essential \
       linux-modules-extra-$(uname -r)

   # Fedora
   sudo dnf install -y libusb1-devel pkg-config

   # Arch Linux
   sudo pacman -S libusb pkg-config
   ```

3. **Kernel Module** (for client development):

   ```bash
   # Load vhci-hcd module
   sudo modprobe vhci-hcd

   # Verify
   lsmod | grep vhci
   ```

4. **Development Tools** (recommended):

   ```bash
   # Install useful cargo extensions
   cargo install cargo-watch     # Auto-rebuild on changes
   cargo install cargo-nextest   # Faster test runner
   cargo install cargo-tarpaulin # Code coverage
   cargo install cargo-audit     # Security audit
   cargo install cross           # Cross-compilation
   ```

### IDE Setup

**VS Code** (recommended):

1. Install the `rust-analyzer` extension
2. Install the `Even Better TOML` extension
3. Optional: `crates` extension for dependency management

Recommended settings (`.vscode/settings.json`):

```json
{
    "rust-analyzer.check.command": "clippy",
    "rust-analyzer.cargo.features": "all",
    "editor.formatOnSave": true
}
```

**Other IDEs**:
- IntelliJ IDEA with Rust plugin
- CLion with Rust plugin
- Neovim with rust-analyzer LSP

### Clone and Build

```bash
# Clone the repository
git clone https://github.com/kimasplund/rust-p2p-usb.git
cd rust-p2p-usb

# Build all crates
cargo build

# Run tests
cargo test

# Build in release mode
cargo build --release
```

---

## Project Structure

```
rust-p2p-usb/
├── Cargo.toml                    # Workspace configuration
├── CLAUDE.md                     # AI assistant configuration
├── crates/
│   ├── protocol/                 # Protocol library (no I/O)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs            # Public API exports
│   │       ├── messages.rs       # Protocol message types
│   │       ├── codec.rs          # Postcard serialization
│   │       ├── types.rs          # Shared types (DeviceInfo, etc.)
│   │       └── version.rs        # Protocol versioning
│   │
│   ├── common/                   # Shared utilities
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs            # Public API exports
│   │       ├── channel.rs        # USB bridge (async channels)
│   │       ├── alpn.rs           # ALPN protocol identifier
│   │       ├── logging.rs        # Tracing setup
│   │       ├── rate_limiter.rs   # Bandwidth limiting
│   │       └── metrics.rs        # Transfer metrics
│   │
│   ├── server/                   # Server binary
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs           # Entry point, CLI
│   │       ├── config.rs         # Configuration management
│   │       ├── service.rs        # Systemd integration
│   │       ├── audit.rs          # Audit logging
│   │       ├── policy.rs         # Device passthrough policies
│   │       ├── qos.rs            # Quality of service
│   │       ├── usb/              # USB subsystem
│   │       │   ├── mod.rs
│   │       │   ├── worker.rs     # USB thread (libusb events)
│   │       │   ├── manager.rs    # Device management
│   │       │   ├── device.rs     # Device wrapper
│   │       │   ├── transfers.rs  # Transfer handling
│   │       │   └── sharing.rs    # Multi-client sharing
│   │       ├── network/          # Iroh server
│   │       │   ├── mod.rs
│   │       │   ├── server.rs     # Iroh endpoint
│   │       │   ├── connection.rs # Client handling
│   │       │   └── notification_aggregator.rs
│   │       └── tui/              # Terminal UI
│   │           ├── mod.rs
│   │           ├── app.rs        # Application state
│   │           ├── ui.rs         # Rendering
│   │           ├── events.rs     # Input handling
│   │           └── qr.rs         # QR code display
│   │
│   └── client/                   # Client binary
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs           # Entry point, CLI
│           ├── config.rs         # Configuration management
│           ├── network/          # Iroh client
│           │   ├── mod.rs
│           │   ├── client.rs     # Server connections
│           │   ├── connection.rs # Connection management
│           │   ├── session.rs    # Session state
│           │   ├── device_proxy.rs # Remote device proxy
│           │   └── health.rs     # Connection health
│           ├── virtual_usb/      # Virtual USB devices
│           │   ├── mod.rs
│           │   ├── linux.rs      # vhci_hcd implementation
│           │   ├── usbip_protocol.rs # USB/IP wire format
│           │   ├── socket_bridge.rs  # TCP socket bridge
│           │   ├── device.rs     # Virtual device state
│           │   ├── macos.rs      # macOS stub
│           │   └── windows.rs    # Windows stub
│           └── tui/              # Terminal UI
│               ├── mod.rs
│               ├── app.rs
│               ├── ui.rs
│               ├── events.rs
│               └── qr.rs
│
├── docs/                         # Documentation
│   ├── ARCHITECTURE.md           # System design
│   ├── DEPLOYMENT.md             # Production deployment
│   ├── DEVELOPMENT.md            # This file
│   ├── SYSTEMD.md                # Service configuration
│   ├── CHANGELOG.md              # Version history
│   └── PROJECT_STATUS.md         # Current status
│
├── examples/                     # Example configurations
│   ├── server.toml
│   └── client.toml
│
├── systemd/                      # Systemd service files
│   └── p2p-usb-server.service
│
└── scripts/                      # Utility scripts
    ├── install-service.sh
    └── uninstall-service.sh
```

### Crate Dependencies

```
protocol (no external deps except serde/postcard)
    ^
    |
common (iroh, tokio, async-channel)
    ^
    |
+---+---+
|       |
server  client (rusb, ratatui, nix)
```

---

## Building and Testing

### Basic Commands

```bash
# Build all crates
cargo build

# Build specific crate
cargo build -p protocol
cargo build -p server

# Build release binaries
cargo build --release

# Run all tests
cargo test

# Run tests for specific crate
cargo test -p protocol
cargo test -p server

# Run specific test
cargo test test_message_roundtrip

# Run tests with output
cargo test -- --nocapture
```

### Using CLAUDE.md Commands

The project includes custom commands defined in CLAUDE.md:

```bash
# Run test suite (from project root)
cargo test

# Run with pattern
cargo test usb_device

# Build for release
cargo build --release

# Run quality checks
cargo fmt --check && cargo clippy && cargo test
```

### Cross-Compilation

```bash
# Install cross
cargo install cross

# Build for Raspberry Pi
cross build --release --target aarch64-unknown-linux-gnu

# Build for x86_64 Linux
cross build --release --target x86_64-unknown-linux-gnu
```

### Running in Development

**Server**:
```bash
# Run server with debug logging
RUST_LOG=debug cargo run -p server

# Run in service mode
cargo run -p server -- --service

# List USB devices
cargo run -p server -- --list-devices
```

**Client**:
```bash
# Run client with debug logging
RUST_LOG=debug cargo run -p client

# Connect to specific server
cargo run -p client -- --connect <endpoint-id>
```

### Code Coverage

```bash
# Install tarpaulin
cargo install cargo-tarpaulin

# Run coverage
cargo tarpaulin --out Html
# Open tarpaulin-report.html
```

---

## Code Style and Conventions

### Formatting

```bash
# Format all code
cargo fmt

# Check formatting without changes
cargo fmt --check
```

### Linting

```bash
# Run clippy
cargo clippy

# Run clippy with all warnings as errors
cargo clippy -- -D warnings

# Fix automatically fixable issues
cargo clippy --fix
```

### Code Conventions

1. **Error Handling**:
   - Use `anyhow::Result` for application-level errors (binaries)
   - Use `thiserror::Error` for library errors (protocol, common)
   - Always add context with `.context()` or `.with_context()`

   ```rust
   // Good
   let config = load_config()
       .context("Failed to load configuration")?;

   // Library error type
   #[derive(Debug, thiserror::Error)]
   pub enum ProtocolError {
       #[error("Serialization error: {0}")]
       Serialization(#[from] postcard::Error),
   }
   ```

2. **Async Patterns**:
   - Use `tokio` for async runtime
   - Avoid blocking in async code (use `spawn_blocking` for CPU-intensive work)
   - Use `async_channel` for cancel-safe channels

   ```rust
   // Good: Non-blocking async
   async fn process_message(&self, msg: Message) -> Result<()> {
       // async operations here
   }

   // For CPU-intensive work
   let result = tokio::task::spawn_blocking(|| {
       compute_hash(&data)
   }).await?;
   ```

3. **Logging**:
   - Use `tracing` macros (`info!`, `debug!`, `warn!`, `error!`)
   - Include relevant context in log messages
   - Use structured logging for machine-readable fields

   ```rust
   use tracing::{info, debug, warn, error};

   info!("Processing device: {:04x}:{:04x}", vendor_id, product_id);
   debug!(device_id = ?device.id, "Device attached");
   warn!("Connection timeout after {}ms", timeout_ms);
   error!(?err, "Failed to submit transfer");
   ```

4. **Documentation**:
   - Document all public items with `///` doc comments
   - Include examples in documentation where helpful
   - Use `//!` for module-level documentation

   ```rust
   /// Submit a USB transfer request to the remote device.
   ///
   /// # Arguments
   ///
   /// * `request` - The USB transfer request containing endpoint and data
   ///
   /// # Returns
   ///
   /// The transfer response with data (for IN transfers) or status.
   ///
   /// # Errors
   ///
   /// Returns an error if the device is disconnected or transfer times out.
   pub async fn submit_transfer(&self, request: UsbRequest) -> Result<UsbResponse>
   ```

5. **Naming Conventions**:
   - Types: `PascalCase`
   - Functions/methods: `snake_case`
   - Constants: `SCREAMING_SNAKE_CASE`
   - Modules: `snake_case`
   - Avoid abbreviations except well-known ones (USB, ID, etc.)

---

## Architecture Overview

### Hybrid Sync-Async Design

The architecture separates synchronous USB operations from asynchronous networking:

```
┌─────────────────────────────────────────────────────────┐
│                   Tokio Runtime                          │
│  ┌─────────────┐  ┌────────────┐  ┌─────────────────┐  │
│  │ Iroh Server │  │    TUI     │  │ Config/Logging │  │
│  │   (async)   │  │  (async)   │  │    (async)     │  │
│  └──────┬──────┘  └────────────┘  └─────────────────┘  │
│         │                                               │
│         │  async_channel (bounded, 256)                │
│         │                                               │
└─────────┼───────────────────────────────────────────────┘
          │
┌─────────▼───────────────────────────────────────────────┐
│              USB Worker Thread (std::thread)            │
│  ┌────────────────────────────────────────────────────┐ │
│  │  libusb_handle_events() loop                       │ │
│  │  - Hot-plug callbacks                              │ │
│  │  - Transfer completion                             │ │
│  │  - Command processing                              │ │
│  └────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────┘
```

**Why this design?**

1. `rusb` (libusb) is synchronous - doesn't work well with async/await
2. USB event loop needs continuous polling - would block async runtime
3. Async channels provide clean separation and backpressure
4. Follows Tokio best practices for mixed sync/async code

### Protocol Flow

```
Client                                    Server
  │                                         │
  │──────── Connect (QUIC) ────────────────►│
  │                                         │
  │◄─────── ServerCapabilities ─────────────│
  │──────── ClientCapabilities ────────────►│
  │                                         │
  │──────── ListDevicesRequest ────────────►│
  │◄─────── ListDevicesResponse ────────────│
  │                                         │
  │──────── AttachDeviceRequest ───────────►│
  │◄─────── AttachDeviceResponse ───────────│
  │                                         │
  │──────── SubmitTransfer ────────────────►│
  │◄─────── TransferComplete ──────────────│
  │                                         │
  │──────── Heartbeat ─────────────────────►│
  │◄─────── HeartbeatAck ──────────────────│
  │                                         │
  │◄─────── DeviceArrivedNotification ──────│ (push)
  │◄─────── DeviceRemovedNotification ──────│ (push)
  │                                         │
```

### Key Components

**Protocol Crate** (`crates/protocol`):
- Message definitions (`MessagePayload` enum)
- Type definitions (`DeviceInfo`, `UsbRequest`, etc.)
- Serialization (postcard codec)
- No I/O - pure data types

**Common Crate** (`crates/common`):
- USB bridge channels (`UsbCommand`, `UsbEvent`)
- Logging setup
- Rate limiting
- Shared utilities

**Server** (`crates/server`):
- USB device management via rusb
- Iroh P2P server endpoint
- TUI for device management
- Systemd integration

**Client** (`crates/client`):
- Iroh P2P client
- Virtual USB via vhci_hcd (Linux)
- TUI for device browsing
- Multi-server support

---

## Adding USB Transfer Types

Currently supported: Control, Bulk, Interrupt
Future: Isochronous (infrastructure exists but disabled)

### 1. Update Protocol Types

In `crates/protocol/src/types.rs`:

```rust
/// USB transfer types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransferType {
    Control { /* ... */ },
    Bulk { /* ... */ },
    Interrupt { /* ... */ },
    // Add new type:
    Isochronous {
        endpoint: u8,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
        /// Number of packets
        num_packets: u32,
        /// Packet size
        packet_size: u32,
    },
}
```

### 2. Implement Server-Side Handling

In `crates/server/src/usb/transfers.rs`:

```rust
pub fn execute_transfer(
    device: &UsbDevice,
    transfer: &TransferType,
) -> Result<TransferResult> {
    match transfer {
        // ... existing cases ...
        TransferType::Isochronous { endpoint, data, num_packets, packet_size } => {
            // Implement isochronous transfer using rusb
            // This requires libusb's async transfer API
        }
    }
}
```

### 3. Implement Client-Side Handling

In `crates/client/src/virtual_usb/usbip_protocol.rs`:

Update the USB/IP command handling to support the new transfer type.

### 4. Add Tests

```rust
#[test]
fn test_isochronous_transfer_roundtrip() {
    let request = UsbRequest {
        id: RequestId(1),
        handle: DeviceHandle(1),
        transfer: TransferType::Isochronous {
            endpoint: 0x81,
            data: vec![0; 1024],
            num_packets: 8,
            packet_size: 128,
        },
    };

    let serialized = postcard::to_allocvec(&request).unwrap();
    let deserialized: UsbRequest = postcard::from_bytes(&serialized).unwrap();
    // Assert equality
}
```

---

## Adding Protocol Messages

### 1. Define the Message

In `crates/protocol/src/messages.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessagePayload {
    // ... existing messages ...

    /// Request device statistics
    GetDeviceStatsRequest {
        device_id: DeviceId,
    },

    /// Response with device statistics
    GetDeviceStatsResponse {
        result: Result<DeviceStats, AttachError>,
    },
}
```

### 2. Define Supporting Types

In `crates/protocol/src/types.rs`:

```rust
/// Statistics for a USB device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceStats {
    pub bytes_transferred: u64,
    pub transfers_completed: u64,
    pub average_latency_ms: f64,
    pub errors: u32,
}
```

### 3. Update Server Handler

In `crates/server/src/network/connection.rs`:

```rust
async fn handle_message(&mut self, payload: MessagePayload) -> Result<MessagePayload> {
    match payload {
        // ... existing handlers ...

        MessagePayload::GetDeviceStatsRequest { device_id } => {
            let stats = self.get_device_stats(device_id).await?;
            Ok(MessagePayload::GetDeviceStatsResponse {
                result: Ok(stats),
            })
        }

        _ => Ok(MessagePayload::Error {
            message: "Unknown message type".to_string(),
        }),
    }
}
```

### 4. Update Client

In `crates/client/src/network/client.rs`:

```rust
pub async fn get_device_stats(&self, device_id: DeviceId) -> Result<DeviceStats> {
    let request = MessagePayload::GetDeviceStatsRequest { device_id };
    let response = self.send_request(request).await?;

    match response {
        MessagePayload::GetDeviceStatsResponse { result } => {
            result.map_err(|e| anyhow!("Failed to get stats: {:?}", e))
        }
        _ => Err(anyhow!("Unexpected response")),
    }
}
```

### 5. Update Protocol Version

If the change is breaking, increment the version in `crates/protocol/src/version.rs`:

```rust
pub const CURRENT_VERSION: ProtocolVersion = ProtocolVersion {
    major: 1,
    minor: 1,  // Increment for new features
    patch: 0,
};
```

---

## Debugging

### Logging Levels

```bash
# Trace (most verbose)
RUST_LOG=trace cargo run -p server

# Debug
RUST_LOG=debug cargo run -p client

# Component-specific
RUST_LOG=server::usb=debug,server::network=trace cargo run -p server

# Suppress noisy crates
RUST_LOG=info,iroh=warn cargo run -p server
```

### USB Debugging

```bash
# List USB devices
lsusb
lsusb -v -d 1234:5678  # Verbose for specific device

# Monitor USB events
udevadm monitor --subsystem-match=usb

# Check USB permissions
ls -la /dev/bus/usb/

# View USB device tree
lsusb -t

# Kernel USB messages
dmesg | grep -i usb
```

### vhci-hcd Debugging (Client)

```bash
# Check vhci status
cat /sys/devices/platform/vhci_hcd.0/status

# Force detach (if stuck)
echo "0" | sudo tee /sys/devices/platform/vhci_hcd.0/detach

# Monitor vhci changes
sudo udevadm monitor --kernel --subsystem-match=usb
```

### Network Debugging

```bash
# Check Iroh connectivity
# Look for "Direct connection" vs "Using relay" in logs

# Network statistics
ss -u -a | grep -i iroh

# Packet capture (advanced)
sudo tcpdump -i any udp port 443
```

### Common Issues

**"Channel closed" errors**:
- USB worker thread crashed - check for panics in logs
- Backpressure - channel is full (256 items)

**"Device not found" errors**:
- Device was unplugged
- Kernel driver took back control
- USB permissions issue

**"Connection timeout"**:
- Network connectivity issue
- Wrong EndpointId
- Server not in `approved_clients`

### Using Debug Builds

Debug builds include extra assertions and better stack traces:

```bash
# Build with debug symbols even in release
RUSTFLAGS="-g" cargo build --release

# Run with backtrace on panic
RUST_BACKTRACE=1 cargo run -p server
RUST_BACKTRACE=full cargo run -p server  # Full backtrace
```

---

## Contributing Guidelines

### Getting Started

1. Fork the repository
2. Create a feature branch: `git checkout -b feature/my-feature`
3. Make your changes
4. Run tests: `cargo test`
5. Run quality checks: `cargo fmt && cargo clippy`
6. Commit with descriptive message
7. Push and create a Pull Request

### Pull Request Checklist

- [ ] Code compiles without warnings (`cargo build`)
- [ ] Tests pass (`cargo test`)
- [ ] Code is formatted (`cargo fmt --check`)
- [ ] Clippy is clean (`cargo clippy -- -D warnings`)
- [ ] Documentation updated if needed
- [ ] CHANGELOG.md updated for notable changes
- [ ] Commit messages are descriptive

### Commit Message Format

```
type: Short description

Longer explanation if needed. Explain the "why" not the "what".

Fixes #123
```

Types:
- `feat`: New feature
- `fix`: Bug fix
- `docs`: Documentation only
- `refactor`: Code change that doesn't fix bug or add feature
- `test`: Adding or updating tests
- `chore`: Maintenance tasks

### Code Review Process

1. All PRs require at least one review
2. CI must pass (tests, clippy, fmt)
3. Address review comments
4. Squash commits if requested

### Reporting Issues

When reporting bugs:

1. Check existing issues first
2. Include:
   - Rust version (`rustc --version`)
   - OS and version
   - Steps to reproduce
   - Expected vs actual behavior
   - Relevant logs (with `RUST_LOG=debug`)

### Testing Requirements

- New features need tests
- Bug fixes should include regression tests
- Maintain or improve code coverage
- Test edge cases

### Documentation Requirements

- Public APIs need doc comments
- New features need usage documentation
- Architecture changes need ARCHITECTURE.md updates
- Breaking changes need migration notes

---

## Additional Resources

- [ARCHITECTURE.md](./ARCHITECTURE.md) - Detailed system design
- [PROJECT_STATUS.md](./PROJECT_STATUS.md) - Current implementation status
- [DEPLOYMENT.md](./DEPLOYMENT.md) - Production deployment guide
- [SYSTEMD.md](./SYSTEMD.md) - Systemd service configuration
- [Iroh Documentation](https://iroh.computer/docs) - P2P networking
- [rusb Documentation](https://docs.rs/rusb) - USB device access
- [Tokio Guide](https://tokio.rs/tokio/tutorial) - Async programming

---

**Questions?** Open a [GitHub Issue](https://github.com/kimasplund/rust-p2p-usb/issues) or check existing discussions.
