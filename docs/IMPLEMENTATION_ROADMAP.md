# rust-p2p-usb Implementation Roadmap

**Based on**: ARCHITECTURE.md  
**Total Duration**: 8-10 weeks (single developer, full-time)  
**Parallel Opportunities**: Yes (see notes)

---

## Phase 0: Project Setup

**Duration**: 1-2 days  
**Dependencies**: None  
**Agent**: Manual or `rust-expert`

### Tasks

1. Initialize Cargo workspace
   ```bash
   cargo new --lib crates/protocol
   cargo new --lib crates/common
   cargo new --bin crates/server
   cargo new --bin crates/client
   ```

2. Create workspace Cargo.toml
   ```toml
   [workspace]
   members = ["crates/*"]
   resolver = "2"
   
   [workspace.package]
   edition = "2024"
   rust-version = "1.90"
   ```

3. Add dependencies to each crate (see ARCHITECTURE.md § Crate Dependencies)

4. Create module structure (empty files with TODOs)

5. Setup CI/CD (.github/workflows/ci.yml)
   - cargo fmt --check
   - cargo clippy -- -D warnings
   - cargo test --all
   - cargo build --all --release

6. Create development commands in .claude/commands/
   - /test, /build, /bench, /quality

7. Write README.md with project overview

### Deliverables

- [ ] `cargo build` succeeds for all crates
- [ ] `cargo test` runs (no tests yet, but works)
- [ ] CI pipeline green
- [ ] README.md complete

### Success Criteria

```bash
cargo build --all
cargo test --all
cargo clippy --all -- -D warnings
cargo fmt --check --all
```

---

## Phase 1: Protocol Foundation

**Duration**: 2-3 days  
**Dependencies**: Phase 0  
**Agent**: `rust-expert`

### Tasks

1. Define all message types in `protocol/src/messages.rs`
   - Message, MessagePayload enum
   - DeviceInfo, DeviceId, DeviceHandle
   - UsbRequest, UsbResponse, TransferType
   - Error types (AttachError, DetachError, UsbError)

2. Implement codec in `protocol/src/codec.rs`
   - Serialize with postcard
   - Deserialize with postcard
   - Helper functions (encode_message, decode_message)

3. Add protocol versioning
   - ProtocolVersion struct
   - Version compatibility checks

4. Write unit tests (target: 90%+ coverage)
   - Roundtrip serialization for each message type
   - Version mismatch handling
   - Error type serialization

5. Benchmark serialization overhead
   - Control transfer (64 bytes)
   - Bulk transfer (4096 bytes)
   - Establish baseline

### Deliverables

- [ ] All protocol types defined
- [ ] Serialization works (postcard)
- [ ] Unit tests pass (90%+ coverage)
- [ ] Benchmarks baseline established
- [ ] Documentation (rustdoc)

### Testing Commands

```bash
cargo test -p protocol
cargo tarpaulin -p protocol --out Html
cargo bench -p protocol
```

---

## Phase 2: USB Subsystem (Server)

**Duration**: 4-5 days  
**Dependencies**: Phase 1  
**Agent**: `rust-expert`

### Tasks

1. Implement async channel bridge (`common/src/channel.rs`)
   - UsbCommand enum
   - UsbEvent enum
   - UsbBridge (Tokio side)
   - UsbWorker (USB thread side)
   - create_usb_bridge() factory

2. Implement USB worker thread (`server/src/usb/worker.rs`)
   - UsbWorkerThread struct
   - libusb_handle_events() loop
   - Command processing (recv_blocking)
   - Event sending (send_blocking)
   - Panic guards for FFI callbacks

3. Implement device manager (`server/src/usb/manager.rs`)
   - Device enumeration
   - Hot-plug callbacks (libusb_hotplug_register_callback)
   - Device state tracking (HashMap<DeviceId, OpenDevice>)

4. Implement transfer handling (`server/src/usb/transfer.rs`)
   - Control transfers (synchronous)
   - Interrupt transfers (synchronous for v1)
   - Bulk transfers (synchronous for v1)
   - Error mapping (rusb::Error → UsbError)

5. Write integration tests
   - Use virtual USB devices (Linux dummy_hcd)
   - Test enumeration
   - Test hot-plug events
   - Test transfers

### Deliverables

- [ ] USB thread runs independently
- [ ] Device enumeration works
- [ ] Hot-plug detection works
- [ ] All transfer types work
- [ ] Integration tests pass
- [ ] No panics in FFI callbacks

### Testing Commands

```bash
# Unit tests
cargo test -p server --lib

# Integration tests (requires dummy_hcd module)
sudo modprobe dummy_hcd
cargo test -p server --test usb_integration
```

---

## Phase 3: Network Layer (Server)

**Duration**: 4-5 days  
**Dependencies**: Phase 1, 2  
**Agent**: `network-latency-expert` or `rust-expert`

### Tasks

1. Implement Iroh server (`server/src/network/server.rs`)
   - NetworkServer struct
   - Endpoint creation (Iroh)
   - Accept connection loop
   - Allowlist checking
   - Spawn per-client tasks

2. Implement client session (`server/src/network/session.rs`)
   - ClientSession struct
   - Stream handler tasks
   - Message routing (ListDevices, AttachDevice, SubmitTransfer)
   - USB event forwarding

3. Implement stream multiplexing (`server/src/network/streams.rs`)
   - Open streams per device (control, interrupt, bulk)
   - Stream lifecycle management
   - Request/response matching (RequestId)

4. Write integration tests
   - Two Iroh endpoints in same process
   - Mock USB bridge (in-memory channels)
   - Test allowlist enforcement
   - Test message flow

### Deliverables

- [ ] Server accepts Iroh connections
- [ ] Client sessions managed correctly
- [ ] QUIC streams multiplex messages
- [ ] Allowlist blocks unauthorized clients
- [ ] Integration tests pass

### Testing Commands

```bash
cargo test -p server --test network_integration
```

---

## Phase 4: Network Layer (Client)

**Duration**: 3-4 days  
**Dependencies**: Phase 1, 3  
**Agent**: `network-latency-expert` or `rust-expert`  
**Can run in parallel with Phase 3** (different developer)

### Tasks

1. Implement Iroh client (`client/src/network/client.rs`)
   - NetworkClient struct
   - Endpoint creation
   - Connect to server (NodeId)
   - Request methods (list_devices, attach_device, submit_transfer)

2. Implement server session (`client/src/network/session.rs`)
   - ServerSession struct
   - Connection management
   - Reconnection with exponential backoff
   - Stream lifecycle

3. Write integration tests
   - Connect to real server (from Phase 3)
   - Test device listing
   - Test attach/detach
   - Test transfers
   - Test reconnection

### Deliverables

- [ ] Client connects to server
- [ ] Device listing works
- [ ] Attach/detach works
- [ ] Transfers work
- [ ] Reconnection works
- [ ] Integration tests pass

### Testing Commands

```bash
# Integration test with server
cargo test -p client --test network_integration

# Manual test
cargo run -p server &
SERVER_PID=$!
cargo run -p client -- --server <NODE_ID>
kill $SERVER_PID
```

---

## Phase 5: Virtual USB (Client - Linux Only)

**Duration**: 5-7 days  
**Dependencies**: Phase 4  
**Agent**: `rust-expert` (kernel interactions complex)  
**Risk**: High complexity (kernel API)

### Tasks

1. Research Linux usbfs/gadgetfs API
   - Read kernel documentation
   - Study configfs gadget framework
   - Understand ioctl interface

2. Implement virtual USB (`client/src/virtual_usb/linux.rs`)
   - VirtualUsbDevice struct
   - Create gadget via configfs
   - Register device descriptors
   - Handle USB requests from kernel
   - Forward to network client

3. Implement platform abstraction (`client/src/virtual_usb/mod.rs`)
   - Platform trait
   - Linux implementation
   - Stubs for macOS/Windows (future)

4. Write integration tests
   - Test device appears in lsusb
   - Test applications can open device
   - Test control transfers

### Deliverables

- [ ] Virtual USB device appears in `lsusb`
- [ ] Applications can enumerate device
- [ ] Control transfers work
- [ ] Integration tests pass

### Testing Commands

```bash
# Requires CAP_SYS_ADMIN or root
sudo -E cargo test -p client --test virtual_usb_integration

# Manual test
sudo -E cargo run -p client -- --server <NODE_ID>
lsusb -v  # Check device appears
```

### Fallback Plan

If gadgetfs/usbfs too complex:
- Implement userspace proxy (no kernel device)
- Applications connect via TCP socket
- Still functional, just not transparent

---

## Phase 6: TUI (Server & Client)

**Duration**: 3-4 days  
**Dependencies**: Phase 3, 4  
**Agent**: `rust-expert`  
**Can start once Phase 3 OR 4 complete** (partial dependency)

### Tasks

1. Design TUI layouts
   - Server: Device list + Active sessions + Logs
   - Client: Available devices + Attached devices + Status

2. Implement server TUI (`server/src/tui/`)
   - app.rs: Application state
   - ui.rs: Ratatui rendering
   - events.rs: Keyboard input (crossterm)

3. Implement client TUI (`client/src/tui/`)
   - app.rs: Application state
   - ui.rs: Ratatui rendering
   - events.rs: Keyboard input

4. Add keyboard shortcuts
   - Arrow keys: Navigation
   - Enter: Select/Attach
   - d: Detach
   - q: Quit

5. Add status bar
   - Connection status
   - NodeId
   - Active devices

### Deliverables

- [ ] Server TUI shows devices and clients
- [ ] Client TUI shows devices
- [ ] Keyboard navigation works
- [ ] UI responsive (no blocking)
- [ ] Error messages display correctly

### Testing

Manual testing (visual inspection):
- Test in different terminal sizes (80x24, 120x40, etc.)
- Test with many devices
- Test with long device names
- Test error message display

---

## Phase 7: Configuration & CLI

**Duration**: 1-2 days  
**Dependencies**: Phase 3, 4  
**Agent**: `rust-expert`  
**Can develop anytime** (independent)

### Tasks

1. Define config file formats
   - server-config.toml (allowlist, device sharing)
   - client-config.toml (server NodeId, preferences)

2. Implement config parsing (`server/src/config.rs`, `client/src/config.rs`)
   - Load TOML files
   - Validate fields
   - Sensible defaults

3. Implement CLI parsing (clap)
   - --config <path>
   - --help
   - --version
   - Server-specific flags
   - Client-specific flags

4. Write unit tests
   - Config parsing
   - Invalid config handling
   - CLI flag precedence

### Deliverables

- [ ] Config files parsed correctly
- [ ] CLI flags work
- [ ] Defaults work if config missing
- [ ] Help text is clear
- [ ] Unit tests pass

### Testing Commands

```bash
cargo run -p server -- --help
cargo run -p server -- --config /path/to/config.toml
cargo run -p client -- --server ed25519:abc123...
```

---

## Phase 8: Systemd Integration (Server)

**Duration**: 2-3 days  
**Dependencies**: Phase 3  
**Agent**: `rust-expert`

### Tasks

1. Create systemd service file (`systemd/rust-p2p-usb-server.service`)
   - Type=notify (sd-notify integration)
   - User=usb-proxy
   - Restart=always

2. Implement sd-notify (`server/src/service.rs`)
   - Notify systemd when ready
   - Watchdog support

3. Write installation script (`scripts/install-systemd.sh`)
   - Copy binary to /usr/local/bin
   - Copy service file to /etc/systemd/system
   - Create usb-proxy user
   - Enable and start service

4. Write udev rules setup (`scripts/setup-udev.sh`)
   - Create 99-rust-p2p-usb.rules
   - Grant USB access to usb-proxy group
   - Reload udev rules

5. Test on Raspberry Pi
   - Deploy binary
   - Install service
   - Test auto-start on boot
   - Check journalctl logs

### Deliverables

- [ ] Server runs as systemd service
- [ ] Auto-starts on boot
- [ ] Logs to journalctl
- [ ] udev rules grant USB access
- [ ] Non-root operation works

### Testing Commands

```bash
# On Raspberry Pi
./scripts/install-systemd.sh
systemctl status rust-p2p-usb-server
journalctl -u rust-p2p-usb-server -f
sudo reboot  # Test auto-start
```

---

## Phase 9: Integration Testing & Optimization

**Duration**: 4-5 days  
**Dependencies**: Phase 2-8 (all)  
**Agents**: `rust-latency-optimizer`, `network-latency-expert`, `root-cause-analyzer`

### Tasks

1. Setup test environment
   - Raspberry Pi server
   - Laptop client
   - Various USB devices (keyboard, mouse, storage)

2. End-to-end testing
   - Attach real keyboard, type in client app
   - Attach real mouse, move in client app
   - Transfer file from USB storage
   - Hot-plug devices during transfers
   - Network interruption recovery

3. Performance measurement
   - Instrument code with tracing
   - Measure latency (tracing spans)
   - Measure throughput (bytes/second)
   - Compare against targets (5-20ms, 38-43 MB/s)

4. Profiling
   - CPU profiling (perf, flamegraph)
   - Memory profiling (heaptrack)
   - Identify hot paths

5. Optimization (if needed)
   - Zero-allocation in hot paths
   - Buffer pooling
   - Reduced serialization
   - QUIC tuning

6. Stress testing
   - 1000 transfers/second
   - 24 hour continuous operation
   - Multiple concurrent devices

7. Write integration test suite
   - Automated end-to-end tests
   - Performance regression tests

### Deliverables

- [ ] End-to-end tests pass
- [ ] Latency <20ms (on good network)
- [ ] Throughput >80% USB 2.0 (on good network)
- [ ] No memory leaks (valgrind)
- [ ] No panics under stress
- [ ] Integration test suite

### Testing Commands

```bash
# Latency measurement
cargo run -p client -- --server <NODE_ID> --measure-latency

# Throughput test
dd if=/dev/zero of=/mnt/usb-storage/test.bin bs=1M count=100
# Measure time, calculate MB/s

# Stress test
cargo test --release --test stress -- --ignored

# Profiling
cargo flamegraph --bin rust-p2p-usb-server
```

---

## Phase 10: Documentation & Release

**Duration**: 2-3 days  
**Dependencies**: All phases  
**Agent**: `codebase-documenter`

### Tasks

1. Write protocol specification (`docs/PROTOCOL.md`)
   - Detailed message formats
   - Wire protocol examples
   - Error handling
   - Versioning strategy

2. Write deployment guide (`docs/DEPLOYMENT.md`)
   - Raspberry Pi setup (OS installation, cross-compilation)
   - Server configuration (allowlists, device sharing)
   - Client setup (Linux, future macOS/Windows)
   - Troubleshooting common issues

3. Write development guide (`docs/DEVELOPMENT.md`)
   - Building from source
   - Running tests
   - Contributing guidelines
   - Agent usage

4. Add rustdoc comments to all public APIs
   - Document all public types
   - Document all public functions
   - Add examples where appropriate

5. Create demo video
   - Show server setup
   - Show client connection
   - Show device attachment
   - Show typing on remote keyboard

6. Prepare GitHub release
   - Tag v0.1.0
   - Create release notes (changelog)
   - Upload binaries (Linux x86_64, aarch64)

7. (Optional) Publish to crates.io
   - Publish protocol crate
   - Publish common crate
   - Server/client remain local (binaries)

### Deliverables

- [ ] PROTOCOL.md complete
- [ ] DEPLOYMENT.md complete
- [ ] DEVELOPMENT.md complete
- [ ] All APIs documented (rustdoc)
- [ ] Demo video created
- [ ] GitHub release v0.1.0
- [ ] (Optional) Published to crates.io

### Commands

```bash
# Generate documentation
cargo doc --all --no-deps --open

# Package release binaries
cargo build --release --target x86_64-unknown-linux-gnu
cargo build --release --target aarch64-unknown-linux-gnu

# Create GitHub release
gh release create v0.1.0 \
  --title "rust-p2p-usb v0.1.0" \
  --notes-file CHANGELOG.md \
  target/x86_64-unknown-linux-gnu/release/rust-p2p-usb-server \
  target/aarch64-unknown-linux-gnu/release/rust-p2p-usb-server
```

---

## Parallel Development Opportunities

### Independent Phases (Can Run Simultaneously)

1. **Phase 3 + Phase 4** (Network Layer Server + Client)
   - Different developers can work on each
   - Integration test at end

2. **Phase 6 (TUI)** can start once Phase 3 OR 4 is done
   - Mock network layer if needed

3. **Phase 7 (Config & CLI)** can start anytime
   - Independent of other phases

### Suggested Team Assignments (if multiple developers)

**Developer 1**: Core USB & Protocol
- Phase 0, 1, 2
- Then: Phase 9 (optimization)

**Developer 2**: Network Server
- Phase 3, 8
- Then: Phase 9 (integration testing)

**Developer 3**: Network Client + Virtual USB
- Phase 4, 5
- Then: Phase 9 (integration testing)

**Developer 4**: UI & Tooling
- Phase 6, 7
- Then: Phase 10 (documentation)

---

## Risk Mitigation Checkpoints

### After Phase 2 (USB Subsystem)
**Checkpoint**: USB enumeration and transfers work on Raspberry Pi

**If fails**: Major architecture issue, revisit runtime design

### After Phase 5 (Virtual USB)
**Checkpoint**: Virtual device appears in `lsusb` on client

**If fails**: Implement userspace proxy fallback

### After Phase 9 (Performance)
**Checkpoint**: Latency <20ms and throughput >38 MB/s on LAN

**If fails**: Profile and optimize, use `rust-latency-optimizer` agent

---

## Success Criteria Summary

Phase complete when:
- [ ] All tasks completed
- [ ] All tests pass
- [ ] All deliverables checked off
- [ ] Documentation updated
- [ ] No clippy warnings
- [ ] Code formatted (rustfmt)

Final release ready when:
- [ ] All 10 phases complete
- [ ] End-to-end tests pass on Raspberry Pi
- [ ] Performance targets met (on good network)
- [ ] Documentation complete
- [ ] Demo video created

---

**END OF ROADMAP**
