# Systemd Service Integration

This document describes how to run the P2P USB Server as a systemd service for production deployments, particularly on Raspberry Pi and other Linux systems.

## Overview

The P2P USB Server includes full systemd integration with:

- **Type=notify**: Proper startup tracking using sd-notify protocol
- **Watchdog support**: Automatic restart if service becomes unresponsive
- **Graceful shutdown**: Clean connection termination and resource cleanup
- **Automatic restart**: Resilient operation with configurable restart policies
- **Security hardening**: Sandboxing and privilege restrictions
- **Journal integration**: Structured logging via systemd-journald

## Installation

### Prerequisites

1. **Build the server in release mode:**
   ```bash
   cargo build --release
   ```

2. **Ensure you have systemd** (most modern Linux distributions):
   ```bash
   systemctl --version
   ```

### Automated Installation

Run the installation script with sudo:

```bash
sudo ./scripts/install-service.sh
```

This script will:
- Copy the binary to `/usr/local/bin/p2p-usb-server`
- Install the service file to `/etc/systemd/system/`
- Create configuration directory at `/etc/p2p-usb/`
- Copy example configuration (if available)
- Reload systemd daemon

### Manual Installation

If you prefer to install manually:

1. **Copy the binary:**
   ```bash
   sudo cp target/release/p2p-usb-server /usr/local/bin/
   sudo chmod +x /usr/local/bin/p2p-usb-server
   ```

2. **Copy the service file:**
   ```bash
   sudo cp systemd/p2p-usb-server.service /etc/systemd/system/
   ```

3. **Create configuration directory:**
   ```bash
   sudo mkdir -p /etc/p2p-usb
   ```

4. **Copy configuration file:**
   ```bash
   sudo cp examples/server.toml /etc/p2p-usb/server.toml
   sudo chmod 600 /etc/p2p-usb/server.toml
   ```

5. **Reload systemd:**
   ```bash
   sudo systemctl daemon-reload
   ```

## Configuration

### Service Configuration

Edit `/etc/p2p-usb/server.toml` to configure the server:

```toml
[server]
# Enable service mode (headless operation)
service_mode = true

# Log level: trace, debug, info, warn, error
log_level = "info"

[usb]
# Auto-share new devices
auto_share = false

[security]
# List of approved client node IDs
approved_clients = [
    # Add your client node IDs here
]

require_approval = true
```

**Important:** The service file expects the config at `/etc/p2p-usb/server.toml`. If you want a different location, edit the service file's `ExecStart` line.

### Systemd Service File

The service file is located at `/etc/systemd/system/p2p-usb-server.service`:

```ini
[Unit]
Description=P2P USB Server - Share USB devices over the internet
Documentation=https://github.com/yourusername/rust-p2p-usb
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

**Key settings:**

- **Type=notify**: Server notifies systemd when ready (proper startup tracking)
- **WatchdogSec=60s**: Service must send keepalive every 60s or it will restart
- **User=root**: Required for USB device access (see Security section)
- **Restart=on-failure**: Automatically restart if service crashes
- **RestartSec=5s**: Wait 5 seconds before restarting

## Usage

### Enable and Start

Enable the service to start automatically on boot:

```bash
sudo systemctl enable p2p-usb-server
```

Start the service immediately:

```bash
sudo systemctl start p2p-usb-server
```

Do both in one command:

```bash
sudo systemctl enable --now p2p-usb-server
```

### Stop and Disable

Stop the service:

```bash
sudo systemctl stop p2p-usb-server
```

Disable automatic startup:

```bash
sudo systemctl disable p2p-usb-server
```

### Restart

Restart the service (useful after configuration changes):

```bash
sudo systemctl restart p2p-usb-server
```

### Status and Health

Check service status:

```bash
sudo systemctl status p2p-usb-server
```

This shows:
- Service state (active, inactive, failed)
- PID and memory usage
- Recent log entries
- Uptime

Example output:
```
● p2p-usb-server.service - P2P USB Server - Share USB devices over the internet
     Loaded: loaded (/etc/systemd/system/p2p-usb-server.service; enabled; preset: enabled)
     Active: active (running) since Thu 2025-10-31 12:00:00 UTC; 2h 15min ago
       Docs: https://github.com/yourusername/rust-p2p-usb
   Main PID: 12345 (p2p-usb-server)
      Tasks: 8 (limit: 4566)
     Memory: 45.2M
     CGroup: /system.slice/p2p-usb-server.service
             └─12345 /usr/local/bin/p2p-usb-server --service --config /etc/p2p-usb/server.toml

Oct 31 12:00:00 raspberrypi systemd[1]: Starting P2P USB Server...
Oct 31 12:00:01 raspberrypi p2p-usb-server[12345]: Server NodeId: ed25519:abc123...
Oct 31 12:00:01 raspberrypi systemd[1]: Started P2P USB Server.
```

## Logging

### View Logs

View all logs:

```bash
sudo journalctl -u p2p-usb-server
```

Follow logs in real-time (like `tail -f`):

```bash
sudo journalctl -u p2p-usb-server -f
```

View logs from the last hour:

```bash
sudo journalctl -u p2p-usb-server --since "1 hour ago"
```

View logs from today:

```bash
sudo journalctl -u p2p-usb-server --since today
```

View logs between dates:

```bash
sudo journalctl -u p2p-usb-server --since "2025-10-31 00:00:00" --until "2025-10-31 23:59:59"
```

Show only errors:

```bash
sudo journalctl -u p2p-usb-server -p err
```

### Log Levels

The log level can be set in three ways (in order of precedence):

1. **Environment variable in service file:**
   ```ini
   Environment="RUST_LOG=debug"
   ```

2. **Configuration file:**
   ```toml
   [server]
   log_level = "info"
   ```

3. **Default:** `info`

Available levels (from most to least verbose):
- `trace` - Very detailed, includes internal operations
- `debug` - Detailed information for debugging
- `info` - General operational messages (default)
- `warn` - Warning messages
- `error` - Error messages only

To change the log level, edit `/etc/systemd/system/p2p-usb-server.service`:

```ini
Environment="RUST_LOG=debug"
```

Then reload and restart:

```bash
sudo systemctl daemon-reload
sudo systemctl restart p2p-usb-server
```

## Troubleshooting

### Service Won't Start

1. **Check service status:**
   ```bash
   sudo systemctl status p2p-usb-server
   ```

2. **View detailed error logs:**
   ```bash
   sudo journalctl -u p2p-usb-server -xe
   ```

3. **Common issues:**

   **Binary not found:**
   ```
   Failed to execute command: No such file or directory
   ```
   - Ensure binary is at `/usr/local/bin/p2p-usb-server`
   - Reinstall with `sudo ./scripts/install-service.sh`

   **Config file not found:**
   ```
   Failed to load configuration
   ```
   - Create `/etc/p2p-usb/server.toml`
   - Copy from `examples/server.toml`

   **Permission denied:**
   ```
   Error accessing USB devices: Permission denied
   ```
   - Service must run as root (see Security section)
   - Check udev rules if trying to run as non-root user

### Service Keeps Restarting

If the service keeps restarting, check:

1. **View restart count:**
   ```bash
   sudo systemctl status p2p-usb-server | grep "Main PID"
   ```

2. **Check for crashes in logs:**
   ```bash
   sudo journalctl -u p2p-usb-server -p err
   ```

3. **Common causes:**
   - Configuration error (invalid TOML)
   - Network connectivity issues (can't reach Iroh relay)
   - USB device access issues
   - Watchdog timeout (service frozen)

### Watchdog Timeouts

If you see watchdog timeout errors:

```
p2p-usb-server.service: Watchdog timeout (limit 1min)
```

This means the service failed to send keepalive messages for 60 seconds. This can happen if:
- Service is deadlocked
- Server is under heavy load
- Network stack is blocking

**Solutions:**

1. **Increase watchdog timeout** (edit service file):
   ```ini
   WatchdogSec=120s
   ```

2. **Disable watchdog** (not recommended):
   ```ini
   # WatchdogSec=60s  (comment out)
   ```

3. **Investigate root cause** using profiling tools

### High CPU or Memory Usage

Check resource usage:

```bash
systemctl status p2p-usb-server
```

For detailed monitoring:

```bash
# CPU usage
pidstat -p $(systemctl show -p MainPID --value p2p-usb-server) 1

# Memory usage
pmap -x $(systemctl show -p MainPID --value p2p-usb-server)
```

### Service Won't Stop

If service hangs during shutdown:

1. **Check timeout setting** in service file:
   ```ini
   TimeoutStopSec=10s
   ```

2. **Force stop:**
   ```bash
   sudo systemctl kill p2p-usb-server
   ```

3. **Force stop with SIGKILL:**
   ```bash
   sudo systemctl kill -s SIGKILL p2p-usb-server
   ```

## Security

### Running as Root

The service runs as **root** by default because USB device access requires elevated privileges. This is common for USB-related services.

**Why root is needed:**
- Access to `/dev/bus/usb/*` devices
- Read/write to `/sys/bus/usb/*` for device enumeration
- Hot-plug event monitoring

### Security Hardening

The service file includes several security restrictions:

- **NoNewPrivileges=true**: Prevents privilege escalation
- **PrivateTmp=true**: Isolated /tmp directory
- **ProtectSystem=strict**: Read-only access to most of the filesystem
- **ProtectHome=true**: No access to user home directories
- **ReadWritePaths**: Explicitly allow only USB device paths

### Running as Non-Root (Advanced)

To run without root, you need udev rules:

1. **Create udev rule** (`/etc/udev/rules.d/99-p2p-usb.rules`):
   ```
   # Grant USB access to usb-proxy group
   SUBSYSTEM=="usb", MODE="0660", GROUP="usb-proxy"
   ```

2. **Create user and group:**
   ```bash
   sudo groupadd usb-proxy
   sudo useradd -r -s /bin/false -g usb-proxy usb-proxy
   sudo usermod -a -G usb-proxy usb-proxy
   ```

3. **Reload udev rules:**
   ```bash
   sudo udevadm control --reload-rules
   sudo udevadm trigger
   ```

4. **Edit service file:**
   ```ini
   User=usb-proxy
   Group=usb-proxy
   ```

5. **Restart service:**
   ```bash
   sudo systemctl daemon-reload
   sudo systemctl restart p2p-usb-server
   ```

**Note:** This approach may not work for all USB devices and operations.

## Auto-Start on Boot

To ensure the service starts automatically after reboot:

1. **Enable the service:**
   ```bash
   sudo systemctl enable p2p-usb-server
   ```

2. **Verify it's enabled:**
   ```bash
   sudo systemctl is-enabled p2p-usb-server
   ```
   Should output: `enabled`

3. **Test by rebooting:**
   ```bash
   sudo reboot
   ```

4. **After reboot, check status:**
   ```bash
   sudo systemctl status p2p-usb-server
   ```

### Boot Dependencies

The service waits for network to be available before starting:

```ini
After=network-online.target
Wants=network-online.target
```

This ensures Iroh networking can initialize properly.

## Monitoring and Maintenance

### Regular Health Checks

Create a cron job to monitor service health:

```bash
# /etc/cron.hourly/check-p2p-usb
#!/bin/bash
if ! systemctl is-active --quiet p2p-usb-server; then
    echo "P2P USB Server is not running!" | mail -s "Service Alert" admin@example.com
fi
```

### Log Rotation

Systemd journal automatically rotates logs. Configure limits in `/etc/systemd/journald.conf`:

```ini
[Journal]
SystemMaxUse=500M
SystemKeepFree=1G
SystemMaxFileSize=100M
MaxRetentionSec=1week
```

Apply changes:

```bash
sudo systemctl restart systemd-journald
```

### Backup Configuration

Regularly backup your configuration:

```bash
sudo tar czf /backup/p2p-usb-config-$(date +%Y%m%d).tar.gz /etc/p2p-usb/
```

## Uninstallation

To completely remove the service:

```bash
sudo ./scripts/uninstall-service.sh
```

Or manually:

```bash
# Stop and disable
sudo systemctl stop p2p-usb-server
sudo systemctl disable p2p-usb-server

# Remove files
sudo rm /etc/systemd/system/p2p-usb-server.service
sudo rm /usr/local/bin/p2p-usb-server

# Reload systemd
sudo systemctl daemon-reload
```

To also remove configuration:

```bash
sudo rm -rf /etc/p2p-usb/
```

## Advanced Configuration

### Custom Systemd Overrides

To override service settings without editing the main file:

```bash
sudo systemctl edit p2p-usb-server
```

This creates an override file at `/etc/systemd/system/p2p-usb-server.service.d/override.conf`.

Example override to increase watchdog timeout:

```ini
[Service]
WatchdogSec=120s
```

Apply changes:

```bash
sudo systemctl daemon-reload
sudo systemctl restart p2p-usb-server
```

### Multiple Instances

To run multiple server instances (e.g., on different ports):

1. **Create instance service file:**
   ```bash
   sudo cp /etc/systemd/system/p2p-usb-server.service \
          /etc/systemd/system/p2p-usb-server@.service
   ```

2. **Edit to use instance name:**
   ```ini
   ExecStart=/usr/local/bin/p2p-usb-server --service --config /etc/p2p-usb/server-%i.toml
   ```

3. **Create instance configs:**
   ```bash
   sudo cp /etc/p2p-usb/server.toml /etc/p2p-usb/server-1.toml
   sudo cp /etc/p2p-usb/server.toml /etc/p2p-usb/server-2.toml
   ```

4. **Start instances:**
   ```bash
   sudo systemctl enable --now p2p-usb-server@1
   sudo systemctl enable --now p2p-usb-server@2
   ```

## Integration with sd-notify

The server implements the sd-notify protocol for proper systemd integration:

### Notification Types

1. **READY=1**: Sent when server is fully initialized and accepting connections
2. **STOPPING=1**: Sent when graceful shutdown begins
3. **WATCHDOG=1**: Sent every 30s (half of WatchdogSec) to prove service is alive
4. **STATUS=...**: Custom status messages visible in `systemctl status`

### Implementation

The notifications are implemented in `crates/server/src/service.rs`:

- `notify_ready()` - Call after initialization
- `notify_stopping()` - Call before shutdown
- `notify_watchdog()` - Called automatically by background task
- `notify_status(msg)` - Send custom status updates

### Debugging Notifications

To verify sd-notify is working:

```bash
# Enable debug logging for systemd
sudo SYSTEMD_LOG_LEVEL=debug systemctl restart p2p-usb-server

# Check notifications in journal
sudo journalctl -u p2p-usb-server | grep notify
```

## Performance Tuning

### Resource Limits

Configure resource limits in the service file:

```ini
[Service]
# Memory limit (hard cap)
MemoryMax=100M

# CPU quota (percentage)
CPUQuota=50%

# Maximum tasks (threads)
TasksMax=100
```

### Nice and Priority

Adjust process priority:

```ini
[Service]
# CPU scheduling priority (-20 to 19, lower = higher priority)
Nice=-5

# I/O scheduling class (realtime, best-effort, idle)
IOSchedulingClass=best-effort
IOSchedulingPriority=0
```

## References

- [systemd service documentation](https://www.freedesktop.org/software/systemd/man/systemd.service.html)
- [systemd sd-notify documentation](https://www.freedesktop.org/software/systemd/man/sd_notify.html)
- [systemd hardening guide](https://www.freedesktop.org/software/systemd/man/systemd.exec.html#Sandboxing)
- [journalctl documentation](https://www.freedesktop.org/software/systemd/man/journalctl.html)

---

**Need help?** Open an issue at [GitHub Issues](https://github.com/yourusername/rust-p2p-usb/issues)
