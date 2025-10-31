#!/bin/bash
# install-service.sh
#
# Installation script for P2P USB Server systemd service
# This script must be run with sudo privileges

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

print_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

print_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Check if running as root
if [ "$EUID" -ne 0 ]; then
    print_error "This script must be run with sudo"
    echo "Usage: sudo $0"
    exit 1
fi

print_info "Installing P2P USB Server as systemd service..."

# Check if binary exists
BINARY_PATH="${PROJECT_ROOT}/target/release/p2p-usb-server"
if [ ! -f "$BINARY_PATH" ]; then
    print_error "Binary not found at: $BINARY_PATH"
    print_info "Please build the project first:"
    echo "  cargo build --release"
    exit 1
fi

# Copy binary
print_info "Installing binary to /usr/local/bin/..."
cp "$BINARY_PATH" /usr/local/bin/
chmod +x /usr/local/bin/p2p-usb-server

# Copy service file
print_info "Installing systemd service file..."
SERVICE_FILE="${PROJECT_ROOT}/systemd/p2p-usb-server.service"
if [ ! -f "$SERVICE_FILE" ]; then
    print_error "Service file not found at: $SERVICE_FILE"
    exit 1
fi
cp "$SERVICE_FILE" /etc/systemd/system/

# Create config directory
print_info "Creating configuration directory..."
mkdir -p /etc/p2p-usb

# Copy example config if not exists
EXAMPLE_CONFIG="${PROJECT_ROOT}/examples/server.toml"
CONFIG_PATH="/etc/p2p-usb/server.toml"

if [ ! -f "$CONFIG_PATH" ]; then
    if [ -f "$EXAMPLE_CONFIG" ]; then
        print_info "Creating default config from example..."
        cp "$EXAMPLE_CONFIG" "$CONFIG_PATH"
        chmod 600 "$CONFIG_PATH"  # Secure config file
        print_warn "Created default config at $CONFIG_PATH"
        print_warn "Please edit this file to configure the server"
    else
        print_warn "Example config not found, skipping config creation"
        print_warn "You will need to create $CONFIG_PATH manually"
    fi
else
    print_info "Configuration file already exists at $CONFIG_PATH"
    print_info "Keeping existing configuration"
fi

# Reload systemd
print_info "Reloading systemd daemon..."
systemctl daemon-reload

print_info "Installation complete!"
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Next steps:"
echo ""
echo "  1. Edit the configuration file:"
echo "     sudo nano /etc/p2p-usb/server.toml"
echo ""
echo "  2. Enable the service to start on boot:"
echo "     sudo systemctl enable p2p-usb-server"
echo ""
echo "  3. Start the service:"
echo "     sudo systemctl start p2p-usb-server"
echo ""
echo "  4. Check service status:"
echo "     sudo systemctl status p2p-usb-server"
echo ""
echo "  5. View logs:"
echo "     sudo journalctl -u p2p-usb-server -f"
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
