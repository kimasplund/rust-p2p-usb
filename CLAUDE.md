# CLAUDE.md

## Project Configuration for Claude Code

This file configures Claude Code for the rust-p2p-usb project.

---

## Project Overview

**rust-p2p-usb** is a high-performance Rust application for secure peer-to-peer USB device sharing over the internet using Iroh networking. It enables access to USB devices connected to a remote server (like a Raspberry Pi) from anywhere as if they were plugged in locally.

**Key Characteristics**:
- **Domain**: Systems Programming / Networking
- **Language**: Rust 2024 Edition
- **Performance**: Latency-critical (5-20ms target for USB transfers)
- **Platform**: Linux primary, macOS/Windows secondary
- **Deployment**: Raspberry Pi + systemd service

---

## Agents

This project uses **5** specialized agents located in `.claude/agents/`:

### 1. **rust-expert**
- **Purpose**: Rust best practices, async/await patterns, type safety, error handling
- **Use for**: Core Rust development, tokio async runtime, lifetime management
- **Location**: `.claude/agents/rust-expert.md`

### 2. **rust-latency-optimizer**
- **Purpose**: Ultra-low latency optimization, profiling, zero-allocation patterns
- **Use for**: Optimizing USB transfer hot paths, meeting 5-20ms latency targets
- **Location**: `.claude/agents/rust-latency-optimizer.md`

### 3. **network-latency-expert**
- **Purpose**: Network performance optimization, protocol design, latency analysis
- **Use for**: P2P networking over Iroh, QUIC optimization, connection management
- **Location**: `.claude/agents/network-latency-expert.md`

### 4. **codebase-documenter**
- **Purpose**: Architecture documentation, API documentation, project structure
- **Use for**: Documenting the multi-crate workspace, creating technical docs
- **Location**: `.claude/agents/codebase-documenter.md`

### 5. **root-cause-analyzer**
- **Purpose**: Systematic debugging, root cause analysis, hypothesis generation
- **Use for**: Debugging complex async/network/USB interaction issues
- **Location**: `.claude/agents/root-cause-analyzer.md`

---

## Skills

This project references skills from the **kimsfinance** plugin:

### kimsfinance plugin
- **Skills**:
  - `kimsfinance-benchmark` - Rust performance benchmarking
  - `kimsfinance-profiler` - Rust profiling and analysis
- **Location**: `~/.claude/plugins/kimsfinance/skills/`
- **Registration**: Already registered (no action needed)

**Using Skills**: Just mention the skill name in your request:
```
"Use kimsfinance-benchmark skill to benchmark USB transfer throughput"
"Use kimsfinance-profiler skill to profile network latency"
```

---

## MCPs

This project uses **1** MCP server configured in `.claude/mcp-servers.json`:

### filesystem
- **Purpose**: Standard file operations for config, logs, and project files
- **Scope**: Project root directory

---

## Commands

### Global Commands
Available from `~/.claude/commands/`:

- **`/rust/clippy-fix`** - Auto-fix Rust clippy warnings and format code
- **`/rust/bench-critical`** - Benchmark critical performance paths

### Project Commands
Located in `.claude/commands/`:

1. **`/test [pattern]`**
   - Run project test suite with optional pattern filtering
   - Examples:
     - `/test` - Run all tests
     - `/test usb_device` - Run tests matching "usb_device"

2. **`/bench [target]`**
   - Run performance benchmarks for USB/network throughput
   - Examples:
     - `/bench` - Run all benchmarks
     - `/bench usb_transfer` - Run specific benchmark
     - `/bench --save baseline` - Save results as baseline

3. **`/build [mode]`**
   - Build project in various modes
   - Examples:
     - `/build` - Debug build
     - `/build release` - Optimized release build

4. **`/cross-rpi`**
   - Cross-compile for Raspberry Pi (aarch64-unknown-linux-gnu)
   - Requires `cross` tool: `cargo install cross`

5. **`/quality`**
   - Run comprehensive quality checks (fmt, clippy, audit, tests)
   - Use before commits or pull requests

---

## Project Structure

```
rust-p2p-usb/
├── crates/
│   ├── server/                # Server binary (RPi)
│   │   ├── src/
│   │   │   ├── main.rs
│   │   │   ├── usb/           # USB device management
│   │   │   ├── tui/           # Terminal UI
│   │   │   ├── config.rs
│   │   │   └── service.rs     # Systemd integration
│   │   └── Cargo.toml
│   │
│   ├── client/                # Client binary (laptop)
│   │   ├── src/
│   │   │   ├── main.rs
│   │   │   ├── virtual_usb/   # Virtual USB devices
│   │   │   ├── tui/           # Terminal UI
│   │   │   └── config.rs
│   │   └── Cargo.toml
│   │
│   ├── protocol/              # Shared protocol library
│   │   ├── src/
│   │   │   ├── messages.rs    # Protocol messages
│   │   │   ├── codec.rs       # Serialization
│   │   │   └── types.rs
│   │   └── Cargo.toml
│   │
│   └── common/                # Shared utilities
│       ├── src/
│       │   ├── iroh_ext.rs    # Iroh extensions
│       │   ├── usb_types.rs   # USB types
│       │   ├── error.rs
│       │   ├── rate_limiter.rs # Bandwidth limiting
│       │   └── metrics.rs     # Transfer metrics
│       └── Cargo.toml
│
├── docs/                      # Documentation
├── scripts/                   # Utility scripts
├── systemd/                   # Service files
└── .claude/                   # Claude Code config
```

---

## Performance Targets

- **USB Latency**: 5-20ms for control transfers (network dependent)
- **Throughput**: 80-90% of USB 2.0 bandwidth over good network
- **Memory**: <50MB RSS per server process
- **CPU**: <5% on Raspberry Pi 4 with 2 active devices

---

## Development Workflow

### Starting Development
1. Use `/build` to verify project compiles
2. Use `/test` to run test suite
3. Use specific agents for specialized tasks:
   - `rust-expert` for core development
   - `rust-latency-optimizer` for performance work
   - `network-latency-expert` for networking

### Before Committing
1. Run `/quality` to ensure code meets standards
2. Run `/test` to verify all tests pass
3. Use `/rust/clippy-fix` for automated cleanup

### Performance Work
1. Use `/bench` to establish baseline
2. Use `kimsfinance-profiler` skill for detailed profiling
3. Use `rust-latency-optimizer` agent for optimization
4. Compare with `/bench --save baseline`

### Cross-Compilation
1. Install cross: `cargo install cross`
2. Use `/cross-rpi` to build for Raspberry Pi
3. Transfer binaries to RPi for testing

---

## Key Dependencies

- **iroh** - P2P networking with NAT traversal
- **rusb** - USB device access (libusb bindings)
- **ratatui** - Terminal user interface
- **tokio** - Async runtime
- **serde** - Serialization framework
- **anyhow** - Error handling
- **clap** - CLI argument parsing
- **tracing** - Structured logging

---

## Security Considerations

- Endpoint approval system (allowlists for client/server node IDs)
- End-to-end encryption via Iroh QUIC (TLS 1.3)
- Per-device sharing granularity on server
- Minimal privileges (udev rules instead of root when possible)

---

## Troubleshooting

### Common Issues

1. **USB Permission Denied**
   - Solution: Setup udev rules (see `scripts/setup-udev.sh`)
   - Or run with appropriate privileges

2. **Build Failures**
   - Check libusb installation: `pkg-config --libs libusb-1.0`
   - Ensure Rust 1.90+ with edition 2024 support

3. **Performance Issues**
   - Use `/bench` to identify bottlenecks
   - Use `rust-latency-optimizer` agent for optimization
   - Check network latency with `network-latency-expert`

4. **Cross-Compilation Issues**
   - Ensure `cross` is installed: `cargo install cross`
   - Check Docker is running (required by cross)

---

## Project-Specific Instructions for Claude

When working on this project:

1. **Performance First**: This is a latency-critical application. Always consider performance implications of code changes.

2. **Async Patterns**: Use tokio idioms correctly. Avoid blocking operations in async contexts.

3. **Error Handling**: Use `anyhow` for application errors, `thiserror` for library errors. Always provide context.

4. **Testing**: Write tests for all core functionality. Benchmark performance-critical paths.

5. **Documentation**: Keep inline documentation up to date, especially for protocol and USB handling code.

6. **Security**: Be mindful of security implications, especially around USB device access and network communication.

7. **Cross-Platform**: While Linux is primary, maintain compatibility considerations for macOS/Windows where feasible.

---

## Current Implementation Status

**Project Stage**: Alpha - Feature Complete (Testing Phase)
**Overall Completion**: ~85%

### What's Working
- Iroh 0.95 P2P networking with EndpointId authentication
- USB device enumeration and management (server)
- USB/IP virtual device creation via vhci_hcd (Linux client)
- Control, Bulk, and Interrupt transfers
- Configuration files and CLI
- Systemd service mode
- Full TUI for both server and client with metrics display
- Rate limiting with atomic try_consume/rollback
- Connection health monitoring (RTT, quality assessment)
- Notification aggregation for TUI responsiveness
- USB 3.0 helpers (port_range_for_speed, optimal_urb_buffer_size)
- Multi-server configuration with all_servers() merging

### Not Yet Implemented
- Isochronous transfers (infrastructure present, disabled)
- macOS/Windows client (stubs only)
- Performance benchmarking suite

### Key Recent Changes (January 2025)
- Upgraded from iroh 0.28 to iroh 0.95
- Added `endpoint.online()` for connection readiness
- Implemented USB/IP import protocol handshake
- Added CMD_UNLINK support for transfer cancellation
- Fixed USB speed codes and port allocation for USB 3.0+
- Added kernel driver detachment/reattachment
- Integrated rate limiter for bandwidth control
- Added notification aggregator for batching rapid device events
- Implemented health monitoring with RTT tracking and connection quality
- Added USB 3.0 helpers for speed-aware port allocation
- Wired detach_all_from_server() for clean disconnect handling
- Enhanced TUI with TX/RX, latency, and throughput metrics

See `/docs/PROJECT_STATUS.md` for detailed status and `/docs/CHANGELOG.md` for change history.

---

**Last Updated**: 2026-01-07
**Initialized by**: init-workspace-v2
**Project Stage**: Alpha - Feature Complete (Testing Phase)
