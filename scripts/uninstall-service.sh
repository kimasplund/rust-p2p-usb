#!/bin/bash
# uninstall-service.sh
#
# Uninstallation script for P2P USB Server systemd service
# This script must be run with sudo privileges

set -e

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

print_info "Uninstalling P2P USB Server service..."

# Stop and disable service
print_info "Stopping and disabling service..."
systemctl stop p2p-usb-server 2>/dev/null || print_warn "Service was not running"
systemctl disable p2p-usb-server 2>/dev/null || print_warn "Service was not enabled"

# Remove service file
print_info "Removing service file..."
rm -f /etc/systemd/system/p2p-usb-server.service

# Reload systemd
print_info "Reloading systemd daemon..."
systemctl daemon-reload
systemctl reset-failed 2>/dev/null || true

# Remove binary
print_info "Removing binary..."
rm -f /usr/local/bin/p2p-usb-server

print_info "Uninstallation complete!"
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Configuration files and data were NOT removed."
echo ""
echo "If you want to completely remove all data, run:"
echo "  sudo rm -rf /etc/p2p-usb/"
echo ""
echo "To remove Iroh data (if applicable):"
echo "  sudo rm -rf /root/.local/share/iroh/"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
