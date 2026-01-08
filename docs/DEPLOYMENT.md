# rust-p2p-usb Deployment Guide

**Last Updated**: 2026-01-08
**Version**: 0.1.0
**Confidence**: 90%

This guide covers deploying rust-p2p-usb in production environments, including server deployment on Raspberry Pi and client deployment on Linux workstations.

---

## Table of Contents

1. [Prerequisites](#prerequisites)
2. [Building from Source](#building-from-source)
3. [Pre-built Binaries](#pre-built-binaries)
4. [Server Deployment](#server-deployment)
5. [Client Deployment](#client-deployment)
6. [Security Configuration](#security-configuration)
7. [Network and Firewall](#network-and-firewall)
8. [Troubleshooting](#troubleshooting)

---

## Prerequisites

### Common Requirements

Both server and client require:

- **Rust**: Version 1.90+ (Edition 2024)
  ```bash
  # Install Rust
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

  # Verify version
  rustc --version  # Must be >= 1.90.0
  ```

- **libusb**: USB device access library
  ```bash
  # Ubuntu/Debian
  sudo apt-get install libusb-1.0-0-dev pkg-config

  # Fedora/RHEL
  sudo dnf install libusb1-devel

  # Arch Linux
  sudo pacman -S libusb
  ```

### Server Requirements (Raspberry Pi / Linux Host)

- **Operating System**: Linux (Ubuntu 20.04+, Raspberry Pi OS, Debian 11+)
- **Architecture**: aarch64 (ARM64) or x86_64
- **Memory**: Minimum 256MB RAM (512MB recommended)
- **USB**: Physical USB ports with connected devices to share
- **systemd**: For service management (optional but recommended)

### Client Requirements (Linux Workstation)

- **Operating System**: Linux with kernel 4.x+ (for vhci_hcd support)
- **Kernel Module**: `vhci-hcd` (USB/IP Virtual Host Controller)
  ```bash
  # Load the vhci-hcd module
  sudo modprobe vhci-hcd

  # Verify it loaded
  lsmod | grep vhci

  # Make persistent across reboots
  echo "vhci-hcd" | sudo tee /etc/modules-load.d/vhci-hcd.conf
  ```

- **Root or CAP_SYS_ADMIN**: Required for vhci-hcd operations

---

## Building from Source

### Standard Build

```bash
# Clone the repository
git clone https://github.com/kimasplund/rust-p2p-usb.git
cd rust-p2p-usb

# Build in release mode (optimized)
cargo build --release

# Binaries are in:
#   target/release/p2p-usb-server
#   target/release/p2p-usb-client
```

### Cross-Compilation for Raspberry Pi

For building on x86_64 for deployment on ARM64 Raspberry Pi:

```bash
# Install cross (requires Docker)
cargo install cross

# Build for Raspberry Pi (aarch64)
cross build --release --target aarch64-unknown-linux-gnu

# Binaries are in:
#   target/aarch64-unknown-linux-gnu/release/p2p-usb-server
#   target/aarch64-unknown-linux-gnu/release/p2p-usb-client
```

Alternatively, build directly on the Raspberry Pi:

```bash
# On the Raspberry Pi
git clone https://github.com/kimasplund/rust-p2p-usb.git
cd rust-p2p-usb
cargo build --release
```

### Optimized Build for Raspberry Pi 4

For best performance on RPi4:

```bash
# Set target CPU for optimization
RUSTFLAGS="-C target-cpu=cortex-a72" cargo build --release
```

---

## Pre-built Binaries

Pre-built binaries are available on the [GitHub Releases](https://github.com/kimasplund/rust-p2p-usb/releases) page for:

- `x86_64-unknown-linux-gnu` (Linux x86_64)
- `aarch64-unknown-linux-gnu` (Raspberry Pi / ARM64)

Download and install:

```bash
# Example for Raspberry Pi
wget https://github.com/kimasplund/rust-p2p-usb/releases/download/v0.1.0/p2p-usb-server-aarch64-linux
chmod +x p2p-usb-server-aarch64-linux
sudo mv p2p-usb-server-aarch64-linux /usr/local/bin/p2p-usb-server
```

---

## Server Deployment

### 1. Install the Binary

```bash
# From build
sudo cp target/release/p2p-usb-server /usr/local/bin/
sudo chmod +x /usr/local/bin/p2p-usb-server
```

### 2. Create Configuration Directory

```bash
sudo mkdir -p /etc/p2p-usb
```

### 3. Create Configuration File

```bash
sudo nano /etc/p2p-usb/server.toml
```

Example configuration:

```toml
[server]
# Run headless (no TUI) when used as service
service_mode = true

# Log level: trace, debug, info, warn, error
log_level = "info"

[usb]
# Auto-share new USB devices when plugged in
# Set to false for manual control via TUI
auto_share = false

# Device filters (only share matching devices)
# Format: "0xVID:0xPID" or "0xVID:*" for vendor wildcard
# Empty list = all devices available (but not auto-shared)
filters = [
    # "0x046d:*",     # All Logitech devices
    # "0x1234:0x5678" # Specific device
]

[security]
# List of approved client EndpointIds
# Get client's EndpointId from: p2p-usb-client (startup log)
approved_clients = [
    # "abc123def456..."  # Add client EndpointIds here
]

# Require clients to be in approved_clients list
# IMPORTANT: Set to true for production!
require_approval = true

[iroh]
# Optional: Custom Iroh relay servers
# Uses default Iroh relays if not specified
# relay_servers = ["https://your-relay.example.com"]

# Optional: Path to secret key for stable EndpointId
# If not set, generates a new key on first run and stores in XDG config
# secret_key_path = "/etc/p2p-usb/server.key"

[audit]
# Enable audit logging for security compliance
enabled = false
# path = "/var/log/p2p-usb/audit.log"
# level = "standard"  # all, standard, security, off
```

### 4. Set Up systemd Service

Install the service file:

```bash
# Using the included install script
sudo ./scripts/install-service.sh

# Or manually
sudo cp systemd/p2p-usb-server.service /etc/systemd/system/
sudo systemctl daemon-reload
```

The service file (`/etc/systemd/system/p2p-usb-server.service`):

```ini
[Unit]
Description=P2P USB Server - Share USB devices over the internet
Documentation=https://github.com/kimasplund/rust-p2p-usb
After=network-online.target
Wants=network-online.target

[Service]
Type=notify
User=root
Group=root
ExecStart=/usr/local/bin/p2p-usb-server --service --config /etc/p2p-usb/server.toml
Restart=on-failure
RestartSec=5s
TimeoutStartSec=30s
TimeoutStopSec=10s
WatchdogSec=60s

# Environment
Environment="RUST_LOG=info"

# Security hardening
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/sys/bus/usb /dev/bus/usb
ReadOnlyPaths=/etc/p2p-usb

# Logging
StandardOutput=journal
StandardError=journal
SyslogIdentifier=p2p-usb-server

[Install]
WantedBy=multi-user.target
```

### 5. Enable and Start the Service

```bash
# Enable at boot
sudo systemctl enable p2p-usb-server

# Start now
sudo systemctl start p2p-usb-server

# Check status
sudo systemctl status p2p-usb-server
```

### 6. Get Your Server EndpointId

The server logs its EndpointId on startup. You'll need this for client configuration:

```bash
# View startup logs
sudo journalctl -u p2p-usb-server | grep EndpointId

# Output example:
# Server EndpointId: abc123def456789...
```

### 7. View Logs

```bash
# Real-time logs
sudo journalctl -u p2p-usb-server -f

# Recent logs
sudo journalctl -u p2p-usb-server --since "1 hour ago"

# All logs
sudo journalctl -u p2p-usb-server
```

---

## Client Deployment

### 1. Ensure vhci-hcd Kernel Module is Loaded

```bash
# Load the module
sudo modprobe vhci-hcd

# Verify
lsmod | grep vhci
# Should show: vhci_hcd

# Check vhci status
cat /sys/devices/platform/vhci_hcd.0/status
```

Make persistent across reboots:

```bash
echo "vhci-hcd" | sudo tee /etc/modules-load.d/vhci-hcd.conf
```

### 2. Install the Binary

```bash
sudo cp target/release/p2p-usb-client /usr/local/bin/
sudo chmod +x /usr/local/bin/p2p-usb-client
```

### 3. Create Configuration File

```bash
mkdir -p ~/.config/p2p-usb
nano ~/.config/p2p-usb/client.toml
```

Example configuration:

```toml
[client]
# Automatically connect to configured servers on startup
auto_connect = true

# Log level
log_level = "info"

[servers]
# Legacy format: list of approved server EndpointIds
approved_servers = [
    # "abc123def456..."  # Server EndpointId
]

# New format: configured servers with names and options
[[servers.configured]]
# Friendly name for the server
name = "pi5-home"
# Server's EndpointId (get from server startup log)
node_id = "abc123def456789..."
# Auto-connect mode: Manual, Auto, AutoWithDevices
auto_connect = "AutoWithDevices"
# Auto-attach filter patterns (VID:PID or product name substring)
auto_attach = [
    "046d:*",        # All Logitech devices
    "YubiKey",       # Any device with "YubiKey" in name
]

[[servers.configured]]
name = "office-server"
node_id = "def789abc123..."
auto_connect = "Manual"

[iroh]
# Optional: Path to secret key for stable EndpointId
# secret_key_path = "~/.config/p2p-usb/client.key"
```

### 4. Running the Client

**Interactive TUI Mode** (default):

```bash
# Run with TUI
p2p-usb-client

# Connect to specific server
p2p-usb-client --connect pi5-home

# Or use full EndpointId
p2p-usb-client --connect abc123def456...
```

**Headless Mode** (for scripting):

```bash
# Connect and stay connected until Ctrl+C
p2p-usb-client --connect pi5-home --headless
```

### 5. Verify Virtual USB Devices

After attaching a device:

```bash
# List USB devices
lsusb

# Check vhci status
cat /sys/devices/platform/vhci_hcd.0/status

# View kernel messages
dmesg | tail -20
```

---

## Security Configuration

### Endpoint Allowlists

The primary security mechanism is EndpointId allowlists. Each Iroh endpoint has a unique cryptographic identity (Ed25519 public key).

**Server-side** (`/etc/p2p-usb/server.toml`):

```toml
[security]
# Only allow these clients to connect
approved_clients = [
    "client-endpoint-id-1...",
    "client-endpoint-id-2...",
]
require_approval = true
```

**Client-side** (`~/.config/p2p-usb/client.toml`):

```toml
[servers]
# Only connect to these servers
approved_servers = [
    "server-endpoint-id-1...",
]
```

### Running Server as Non-Root (Advanced)

By default, the server runs as root for USB access. To run as a non-root user:

1. **Create udev rules** (`/etc/udev/rules.d/99-p2p-usb.rules`):

   ```
   # Grant USB access to p2p-usb group
   SUBSYSTEM=="usb", MODE="0660", GROUP="p2p-usb"
   ```

2. **Create user and group**:

   ```bash
   sudo groupadd p2p-usb
   sudo useradd -r -s /bin/false -g p2p-usb p2p-usb
   ```

3. **Reload udev rules**:

   ```bash
   sudo udevadm control --reload-rules
   sudo udevadm trigger
   ```

4. **Update service file**:

   ```ini
   [Service]
   User=p2p-usb
   Group=p2p-usb
   ```

**Note**: Some USB operations may still require root. Test thoroughly.

### Device Policies (Advanced)

Fine-grained device access control:

```toml
[[device_policies]]
# Brother printers only during business hours
device_filter = "04f9:*"
allowed_clients = ["specific-client-id"]
time_windows = ["09:00-17:00"]
max_session_duration = "1h"
sharing_mode = "shared"
```

---

## Network and Firewall

### Iroh P2P Networking

rust-p2p-usb uses Iroh for P2P networking, which includes:

- **QUIC Protocol**: UDP-based, encrypted (TLS 1.3)
- **NAT Traversal**: Automatic via Iroh relay servers
- **No Port Forwarding Required**: Usually works behind NAT

### Firewall Considerations

**Outbound (usually allowed by default)**:

- UDP port 443 (QUIC to Iroh relays)
- UDP any port (direct P2P connections)

**If direct connections fail**, Iroh falls back to relay servers, which work through most firewalls.

**For optimal performance** (direct P2P):

```bash
# Allow all outbound UDP (typically default)
# If using UFW:
sudo ufw allow out proto udp
```

### Verifying Connectivity

```bash
# Check if direct connection established
# Look in server logs for:
# "Direct connection established" vs "Using relay"
sudo journalctl -u p2p-usb-server | grep -i connection
```

---

## Troubleshooting

### Server Issues

**Binary not found**:
```
Failed to execute command: No such file or directory
```
- Ensure binary is at `/usr/local/bin/p2p-usb-server`
- Run `which p2p-usb-server` to verify

**Config file not found**:
```
Failed to load configuration
```
- Create `/etc/p2p-usb/server.toml`
- Copy from `examples/server.toml` in the repository

**USB permission denied**:
```
Error accessing USB devices: Permission denied
```
- Ensure service runs as root
- Or set up udev rules for non-root operation
- Check USB device permissions: `ls -la /dev/bus/usb/`

**Service keeps restarting**:
```bash
# Check for crash reasons
sudo journalctl -u p2p-usb-server -p err

# Common causes:
# - Invalid TOML configuration
# - Network connectivity issues
# - USB device access problems
```

### Client Issues

**vhci-hcd not loaded**:
```
Failed to initialize Virtual USB Manager
```
- Load the module: `sudo modprobe vhci-hcd`
- Check kernel support: `modinfo vhci-hcd`
- May need to install `linux-modules-extra-$(uname -r)`

**Device enumeration stalls**:
```
Device attaches but enumeration hangs
```
- Known issue with some USB devices under heavy load
- Retry the attach operation
- Check `dmesg` for USB errors

**Connection timeout**:
```
Failed to connect to server
```
- Verify server EndpointId is correct
- Check server is running: `sudo systemctl status p2p-usb-server`
- Check network connectivity
- Ensure client is in server's `approved_clients`

**Permission denied for vhci**:
```
Permission denied accessing /sys/devices/platform/vhci_hcd.0
```
- Client operations require root or CAP_SYS_ADMIN
- Run with `sudo` or set appropriate capabilities

### Common Log Analysis

**Server**:
```bash
# Debug logging
sudo systemctl stop p2p-usb-server
RUST_LOG=debug /usr/local/bin/p2p-usb-server --service
```

**Client**:
```bash
# Debug logging
RUST_LOG=debug p2p-usb-client --connect pi5-home
```

### USB Device Not Appearing

1. Check server logs for device enumeration
2. Verify device is shared on server (TUI or auto_share)
3. Check client's auto_attach filters match the device
4. Verify vhci-hcd status: `cat /sys/devices/platform/vhci_hcd.0/status`
5. Check kernel messages: `dmesg | tail -50`

### Getting Help

- Open an issue: [GitHub Issues](https://github.com/kimasplund/rust-p2p-usb/issues)
- Check existing docs in `/docs/` directory
- Review `ARCHITECTURE.md` for system design details

---

## Quick Reference

### Server Commands

```bash
# Start TUI mode (interactive)
p2p-usb-server

# Start service mode (headless)
p2p-usb-server --service

# List USB devices
p2p-usb-server --list-devices

# Custom config
p2p-usb-server --config /path/to/config.toml

# Debug logging
p2p-usb-server --log-level debug

# systemd commands
sudo systemctl start p2p-usb-server
sudo systemctl stop p2p-usb-server
sudo systemctl restart p2p-usb-server
sudo systemctl status p2p-usb-server
sudo journalctl -u p2p-usb-server -f
```

### Client Commands

```bash
# Start TUI mode (interactive)
p2p-usb-client

# Connect to server by name
p2p-usb-client --connect pi5-home

# Connect by EndpointId
p2p-usb-client --connect abc123def456...

# Headless mode
p2p-usb-client --connect pi5-home --headless

# Custom config
p2p-usb-client --config /path/to/config.toml

# Debug logging
p2p-usb-client --log-level debug

# Save default config
p2p-usb-client --save-config
```

### Useful System Commands

```bash
# Load vhci module
sudo modprobe vhci-hcd

# List USB devices
lsusb
lsusb -v  # Verbose

# Check vhci status
cat /sys/devices/platform/vhci_hcd.0/status

# USB kernel messages
dmesg | grep -i usb

# Check USB permissions
ls -la /dev/bus/usb/
```

---

**See Also**:
- [ARCHITECTURE.md](./ARCHITECTURE.md) - System design and implementation details
- [SYSTEMD.md](./SYSTEMD.md) - Detailed systemd service configuration
- [DEVELOPMENT.md](./DEVELOPMENT.md) - Developer guide for contributing
