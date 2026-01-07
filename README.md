# rust-p2p-usb

**Secure peer-to-peer USB device sharing over the internet using Iroh**

> [!WARNING]
> **ALPHA SOFTWARE (v0.1.0-dev)**: This software is currently in active development. It is functional for testing but not yet ready for production use. The Client currently only supports **Linux**.


A high-performance Rust application that enables secure USB device sharing between machines anywhere on the internet. Built with Iroh for NAT traversal and P2P connectivity, this tool lets you access USB devices connected to a remote server (like a Raspberry Pi) from your laptop as if they were plugged in locally.

## Features

### Server
- **Auto-discovery of USB devices** - Automatically detects and lists all connected USB devices
- **Terminal UI** - Clean, intuitive interface for managing shared devices
- **Selective sharing** - Enable/disable individual devices with a keypress
- **Connection management** - Display Iroh node ID and connection keys
- **Approved endpoints** - Whitelist of authorized client node IDs
- **System service** - Run as a systemd service for always-on availability
- **Low resource usage** - Optimized for single-board computers like Raspberry Pi
- **Real-time monitoring** - Live device connection status and data transfer metrics

### Client
- **Remote device access** - Connect to USB devices over the internet
- **Terminal UI** - Manage multiple server connections and remote devices
- **Auto-reconnection** - Basic reconnection handling
- **Approved servers** - Whitelist of trusted server node IDs
- **Device filtering** - Show only relevant devices based on VID/PID
- **Performance metrics** - Monitor latency and throughput in real-time

### Core Technology
- **Iroh networking** - Built on Iroh for reliable P2P connectivity with NAT traversal
- **Zero-configuration** - No port forwarding or VPN setup required
- **End-to-end encryption** - All traffic encrypted via Iroh's built-in security
- **Linux-first** - Optimized for Linux (Client currently Linux-only)
- **High performance** - Optimized data paths with zero-copy where possible
- **Rust 2024 edition** - Leveraging the latest Rust language features

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                           Internet                               │
│                    (NAT traversal via Iroh)                      │
└────────────┬─────────────────────────────┬──────────────────────┘
             │                             │
             │                             │
    ┌────────▼────────┐          ┌────────▼────────┐
    │  Server (RPi)   │          │  Client (Laptop)│
    │                 │          │                 │
    │  ┌───────────┐  │          │  ┌───────────┐  │
    │  │   TUI     │  │          │  │   TUI     │  │
    │  └─────┬─────┘  │          │  └─────┬─────┘  │
    │        │        │          │        │        │
    │  ┌─────▼─────┐  │          │  ┌─────▼─────┐  │
    │  │USB Manager│  │          │  │ Connection│  │
    │  └─────┬─────┘  │          │  │  Manager  │  │
    │        │        │          │  └─────┬─────┘  │
    │  ┌─────▼─────┐  │          │        │        │
    │  │   Iroh    │  │          │  ┌─────▼─────┐  │
    │  │  Server   │◄─┼──────────┼─►│   Iroh    │  │
    │  └─────┬─────┘  │          │  │  Client   │  │
    │        │        │          │  └─────┬─────┘  │
    │  ┌─────▼─────┐  │          │        │        │
    │  │  libusb   │  │          │  ┌─────▼─────┐  │
    │  └─────┬─────┘  │          │  │USB Virtual│  │
    │        │        │          │  │   Device  │  │
    │   [USB Devices] │          │  └───────────┘  │
    └─────────────────┘          └─────────────────┘
```

### Component Overview

1. **Server Process**
   - Monitors USB bus for device attach/detach events
   - Maintains device state and sharing permissions
   - Exposes shared devices via Iroh protocol
   - Validates client node IDs against approved list
   - Proxies USB traffic from authorized clients

2. **Client Process**
   - Discovers and connects to approved servers
   - Creates virtual USB devices for remote devices
   - Proxies USB requests to server over Iroh
   - Handles connection failures and retry logic

3. **Protocol Layer**
   - Custom binary protocol over Iroh QUIC streams
   - Device enumeration and capability exchange
   - USB control, bulk, interrupt, and isochronous transfers
   - Efficient serialization with `bincode` or `postcard`

4. **TUI Layer**
   - Built with `ratatui` for terminal rendering
   - Responsive keyboard-driven interface
   - Real-time updates via async channels

## Prerequisites

### System Requirements

**Server (Raspberry Pi or Linux host):**
- Rust 1.90+ (edition 2024 support)
- Linux kernel with USB support
- libusb 1.0+
- Root privileges for USB access (or udev rules)

**Client (Linux):**
- Rust 1.90+ (edition 2024 support)
- libusb 1.0+
- USB/IP kernel modules (`vhci-hcd`)

### Dependencies

The project uses the following key crates:
- `iroh` - P2P networking and NAT traversal
- `rusb` - USB device access (libusb bindings)
- `ratatui` - Terminal user interface
- `tokio` - Async runtime
- `serde` - Serialization
- `anyhow` - Error handling
- `clap` - CLI argument parsing
- `tracing` - Structured logging

## Installation

### From Source

```bash
# Clone the repository
git clone https://github.com/kimasplund/rust-p2p-usb.git
cd rust-p2p-usb

# Build release binaries
cargo build --release

# Install to system (optional)
sudo cp target/release/p2p-usb-server /usr/local/bin/
sudo cp target/release/p2p-usb-client /usr/local/bin/
```

### Install libusb

**Ubuntu/Debian:**
```bash
sudo apt-get install libusb-1.0-0-dev
```

**Fedora/RHEL:**
```bash
sudo dnf install libusb1-devel
```

**macOS:**
```bash
brew install libusb
```

**Arch Linux:**
```bash
sudo pacman -S libusb
```

## Configuration

### Server Configuration

Create a configuration file at `~/.config/p2p-usb/server.toml`:

```toml
[server]
# Bind address for local API (optional)
bind_addr = "127.0.0.1:8080"

# Enable systemd service mode
service_mode = false

# Log level: trace, debug, info, warn, error
log_level = "info"

[usb]
# Auto-share new devices
auto_share = false

# Device filters (vendor_id:product_id)
# Empty means all devices available
# filters = ["0x1234:0x5678", "0xabcd:*"]

[security]
# List of approved client node IDs (Iroh public keys)
# Empty list means all clients allowed (NOT RECOMMENDED)
approved_clients = [
    # "iroh_node_id_1",
    # "iroh_node_id_2",
]

# Require approval for new clients
require_approval = true

[iroh]
# Optional: specify custom Iroh relay servers
# relay_servers = ["https://relay.example.com"]
```

### Client Configuration

Create a configuration file at `~/.config/p2p-usb/client.toml`:

```toml
[client]
# Auto-connect to known servers on startup
auto_connect = true

# Log level
log_level = "info"

[servers]
# List of approved server node IDs
approved_servers = [
    # "iroh_node_id_server_1",
    # "iroh_node_id_server_2",
]

[iroh]
# Optional: specify custom Iroh relay servers
# relay_servers = ["https://relay.example.com"]
```

### Setting Up Approved Endpoints

1. **Start the server** and note the Iroh node ID displayed in the TUI
2. **Add the node ID** to the client's `approved_servers` list
3. **Start the client** and note its node ID
4. **Add the client node ID** to the server's `approved_clients` list
5. **Restart both** or reload configuration

## Usage

### Server

**Interactive Mode:**
```bash
# Run server
p2p-usb-server

# List devices
p2p-usb-server --list-devices


# Run with debug logging
RUST_LOG=info p2p-usb-server
```

**TUI Keybindings:**
- `↑/↓` - Navigate device list
- `Space` - Toggle device sharing
- `a` - Toggle share all devices
- `c` - Show connection details (Iroh node ID, QR code)
- `l` - Show logs
- `r` - Refresh device list
- `q` - Quit

**As a System Service:**

Create `/etc/systemd/system/p2p-usb-server.service`:

```ini
[Unit]
Description=P2P USB Server
After=network.target

[Service]
Type=simple
User=root
ExecStart=/usr/local/bin/p2p-usb-server --service
Restart=on-failure
RestartSec=5s

[Install]
WantedBy=multi-user.target
```

Enable and start:
```bash
sudo systemctl daemon-reload
sudo systemctl enable p2p-usb-server
sudo systemctl start p2p-usb-server
sudo systemctl status p2p-usb-server
```

### Client

**Interactive Mode:**
```bash
# Connect to specific server
p2p-usb-client --connect <server-node-id>

# Run with custom config
p2p-usb-client --config /path/to/config.toml
```

**TUI Keybindings:**
- `↑/↓` - Navigate server/device list
- `Enter` - Connect to selected server
- `Space` - Enable/disable remote device
- `d` - Disconnect from server
- `a` - Add new server (enter node ID)
- `r` - Remove selected server from list
- `l` - Show logs
- `q` - Quit

## Security Model

### Authentication
- Each Iroh node has a unique cryptographic identity (Ed25519 keypair)
- Server and client maintain allowlists of approved node IDs
- Connections from unknown nodes are rejected

### Encryption
- All data encrypted end-to-end via Iroh's QUIC transport (TLS 1.3)
- No plaintext USB data transmitted over the network

### Authorization
- Server controls which USB devices are shared
- Client controls which servers it trusts
- Per-device sharing granularity on server side

### Best Practices
- Keep `approved_clients` and `approved_servers` lists up to date
- Use `require_approval = true` on production servers
- Monitor logs for connection attempts
- Rotate Iroh identities periodically (TODO: implement)
- Run server with minimal privileges (udev rules instead of root)

- Run server with minimal privileges (udev rules instead of root)

## Known Issues

- **Virtual USB Stalls**: Occasionally, virtual device enumeration may stall on the client side. Retrying the connection usually resolves this.
- **Isochronous Transfers**: Webcams and audio devices (isochronous transfers) are not yet supported.
- **Platform Support**: Client is currently Linux-only. Windows and macOS support is planned (virtual USB drivers are the main blocker).

## Building from Source

### Development Build

```bash
# Build with debug symbols
cargo build

# Run server directly
cargo run --bin p2p-usb-server

# Run client directly
cargo run --bin p2p-usb-client

# Run tests
cargo test

# Run with verbose logging
RUST_LOG=debug cargo run --bin p2p-usb-server
```

### Release Build

```bash
# Optimized release build
cargo build --release

# Build with link-time optimization
cargo build --release --config profile.release.lto=true

# Cross-compile for Raspberry Pi (aarch64)
cargo install cross
cross build --release --target aarch64-unknown-linux-gnu
```

### Code Quality

```bash
# Run clippy
cargo clippy -- -D warnings

# Format code
cargo fmt

# Run all tests
cargo test --all-features

# Security audit
cargo audit
```

## Project Structure

```
rust-p2p-usb/
├── Cargo.toml                 # Workspace manifest
├── README.md                  # This file
├── LICENSE                    # Project license
├── .gitignore
│
├── crates/
│   ├── server/                # Server binary crate
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs        # Entry point
│   │       ├── usb/           # USB device management
│   │       ├── tui/           # Terminal UI
│   │       ├── config.rs      # Configuration
│   │       └── service.rs     # Systemd service integration
│   │
│   ├── client/                # Client binary crate
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs        # Entry point
│   │       ├── virtual_usb/   # Virtual USB device creation
│   │       ├── tui/           # Terminal UI
│   │       └── config.rs      # Configuration
│   │
│   ├── protocol/              # Shared protocol library
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── messages.rs    # Protocol message types
│   │       ├── codec.rs       # Serialization/deserialization
│   │       └── types.rs       # Shared types
│   │
│   └── common/                # Shared utilities
│       ├── Cargo.toml
│       └── src/
│           ├── lib.rs
│           ├── iroh_ext.rs    # Iroh extensions
│           ├── usb_types.rs   # USB type definitions
│           └── error.rs       # Error types
│
├── docs/                      # Documentation
│   ├── ARCHITECTURE.md        # Detailed architecture
│   ├── PROTOCOL.md            # Protocol specification
│   └── PERFORMANCE.md         # Performance tuning guide
│
├── scripts/                   # Utility scripts
│   ├── install-deps.sh        # Install system dependencies
│   └── setup-udev.sh          # Setup udev rules
│
└── systemd/                   # Service files
    └── p2p-usb-server.service
```

## Performance Optimization

### Server Optimization
- **Zero-copy USB transfers** where possible using `rusb` direct buffer access
- **Async USB polling** via `tokio-rusb` integration
- **Connection pooling** for multiple clients
- **Device state caching** to minimize USB bus queries

### Client Optimization
- **Request coalescing** for bulk operations
- **Adaptive buffer sizing** based on device type
- **Prefetching** for sequential transfers
- **Connection keep-alive** to minimize reconnection overhead

### Network Optimization
- **Iroh QUIC streams** for multiplexed low-latency transfers
- **Custom binary protocol** with minimal overhead
- **Compression** for control messages (not bulk data)
- **Batching** small transfers to reduce round trips

### Benchmarking

```bash
# Run performance benchmarks
cargo bench

# Profile with flamegraph
cargo install flamegraph
sudo cargo flamegraph --bin p2p-usb-server

# Memory profiling
RUSTFLAGS='-g' cargo build --release
valgrind --tool=massif target/release/p2p-usb-server
```

Expected performance:
- **Latency:** 5-20ms for control transfers (depends on network)
- **Throughput:** 80-90% of USB 2.0 bandwidth over good network
- **Memory:** <50MB RSS per server process
- **CPU:** <5% on Raspberry Pi 4 with 2 active devices

## Troubleshooting

### Server Issues

**Problem:** "Permission denied" when accessing USB devices

**Solution:** Add udev rules or run as root (not recommended for production)
```bash
# Create udev rule
sudo tee /etc/udev/rules.d/99-p2p-usb.rules << EOF
SUBSYSTEM=="usb", MODE="0666"
EOF
sudo udevadm control --reload-rules
```

**Problem:** Devices not appearing in list

**Solution:** Check USB device permissions and verify `libusb` installation
```bash
lsusb -v
sudo p2p-usb-server --list-devices
```

### Client Issues

**Problem:** Cannot connect to server

**Solution:**
1. Verify server node ID is correct
2. Check server's `approved_clients` list includes your client node ID
3. Ensure Iroh relay servers are reachable
4. Check firewall settings (Iroh usually works through NAT)

**Problem:** High latency or timeouts

**Solution:**
- Check network connection quality
- Verify Iroh is establishing direct connection (not relay-only)
- Reduce concurrent USB transfers
- Check server CPU/memory usage

### Debug Logging

```bash
# Server with debug logs
RUST_LOG=p2p_usb=debug,iroh=info p2p-usb-server

# Client with trace logs
RUST_LOG=trace p2p-usb-client

# Log to file
RUST_LOG=debug p2p-usb-server 2>&1 | tee server.log
```

## Roadmap

### Phase 1: Core Functionality (v0.1.0) - COMPLETE
- [x] USB device enumeration and hot-plug detection
- [x] Iroh P2P client-server communication (iroh 0.95)
- [x] USB control, bulk, and interrupt transfers
- [x] EndpointId-based authentication
- [x] Configuration file support (TOML)
- [x] Systemd service integration
- [x] Virtual USB via vhci_hcd (Linux)
- [x] USB/IP protocol implementation
- [x] Kernel driver management (detach/reattach)

### Phase 2: User Experience (v0.2) - IN PROGRESS
- [x] CLI argument parsing
- [x] Comprehensive logging with tracing
- [x] Terminal UI (TUI) for server and client
- [ ] Error recovery and reconnection
- [ ] Performance metrics display
- [ ] Device filtering by VID/PID

### Phase 3: Enhanced Features (v0.3)
- [ ] Isochronous transfers (for audio/video devices)
- [ ] QR code for easy server connection
- [ ] Connection health monitoring
- [ ] Bandwidth usage statistics
- [ ] Hot-plug notification to clients

### Phase 4: Advanced Features (v1.0)
- [ ] Multi-client support (device sharing)
- [ ] Device passthrough policies (time-based access)
- [ ] Bandwidth limiting and QoS
- [ ] macOS client support
- [ ] Windows client support

### Future Considerations
- [ ] USB 3.0 SuperSpeed optimization
- [ ] Web-based management interface
- [ ] Mobile client support
- [ ] Encrypted device storage (for security keys)
- [ ] Audit logging and compliance features

See [docs/PROJECT_STATUS.md](docs/PROJECT_STATUS.md) for detailed progress and [docs/CHANGELOG.md](docs/CHANGELOG.md) for version history.

## Contributing

Contributions are welcome! Please follow these guidelines:

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

### Development Guidelines
- Follow Rust API guidelines
- Add tests for new functionality
- Update documentation
- Run `cargo fmt` and `cargo clippy` before committing
- Keep PRs focused and atomic

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## Acknowledgments

- [Iroh](https://github.com/n0-computer/iroh) - For the amazing P2P networking library
- [rusb](https://github.com/a1ien/rusb) - For USB device access
- [ratatui](https://github.com/ratatui-org/ratatui) - For the excellent TUI framework
- USB/IP kernel developers - For inspiration on USB over network protocols

## Support

- Issues: [GitHub Issues](https://github.com/kimasplund/rust-p2p-usb/issues)
- Discussions: [GitHub Discussions](https://github.com/kimasplund/rust-p2p-usb/discussions)

---

**Built with ❤️ in Rust**
