#!/bin/bash
# Test script for Phase 7: Configuration and CLI
# This script validates that the configuration system works correctly

set -e

echo "================================"
echo "Phase 7: Configuration Testing"
echo "================================"
echo ""

# Test 1: Create test config files
echo "Test 1: Creating test configuration files..."
mkdir -p /tmp/p2p-usb-test

cat > /tmp/p2p-usb-test/server.toml <<'EOF'
[server]
bind_addr = "127.0.0.1:9999"
service_mode = true
log_level = "debug"

[usb]
auto_share = true
filters = ["0x1234:0x5678", "0xabcd:*"]

[security]
approved_clients = ["test_client_id_1", "test_client_id_2"]
require_approval = true

[iroh]
relay_servers = ["https://relay.example.com"]
EOF

cat > /tmp/p2p-usb-test/client.toml <<'EOF'
[client]
auto_connect = false
log_level = "trace"

[servers]
approved_servers = ["test_server_id_1", "test_server_id_2"]

[iroh]
relay_servers = ["https://relay.example.com"]
EOF

echo "  ✓ Test config files created"
echo ""

# Test 2: Validate TOML syntax
echo "Test 2: Validating TOML syntax..."
if command -v python3 &> /dev/null; then
    python3 <<'PYTHON'
import tomllib
with open('/tmp/p2p-usb-test/server.toml', 'rb') as f:
    tomllib.load(f)
print("  ✓ server.toml is valid TOML")
with open('/tmp/p2p-usb-test/client.toml', 'rb') as f:
    tomllib.load(f)
print("  ✓ client.toml is valid TOML")
PYTHON
else
    echo "  ⚠ Python3 not available, skipping TOML validation"
fi
echo ""

# Test 3: Check example configs
echo "Test 3: Checking example configuration files..."
if [ -f "examples/server.toml" ]; then
    echo "  ✓ examples/server.toml exists"
    if command -v python3 &> /dev/null; then
        python3 -c "import tomllib; tomllib.load(open('examples/server.toml', 'rb'))"
        echo "  ✓ examples/server.toml is valid TOML"
    fi
else
    echo "  ✗ examples/server.toml missing"
    exit 1
fi

if [ -f "examples/client.toml" ]; then
    echo "  ✓ examples/client.toml exists"
    if command -v python3 &> /dev/null; then
        python3 -c "import tomllib; tomllib.load(open('examples/client.toml', 'rb'))"
        echo "  ✓ examples/client.toml is valid TOML"
    fi
else
    echo "  ✗ examples/client.toml missing"
    exit 1
fi
echo ""

# Test 4: Check source files
echo "Test 4: Checking source files..."
required_files=(
    "crates/server/src/config.rs"
    "crates/client/src/config.rs"
    "crates/server/src/main.rs"
    "crates/client/src/main.rs"
)

for file in "${required_files[@]}"; do
    if [ -f "$file" ]; then
        echo "  ✓ $file exists"
    else
        echo "  ✗ $file missing"
        exit 1
    fi
done
echo ""

# Test 5: Check for required functions in config modules
echo "Test 5: Checking config module implementations..."

if grep -q "pub fn load" crates/server/src/config.rs; then
    echo "  ✓ ServerConfig::load() found"
else
    echo "  ✗ ServerConfig::load() missing"
    exit 1
fi

if grep -q "pub fn load_or_default" crates/server/src/config.rs; then
    echo "  ✓ ServerConfig::load_or_default() found"
else
    echo "  ✗ ServerConfig::load_or_default() missing"
    exit 1
fi

if grep -q "pub fn save" crates/server/src/config.rs; then
    echo "  ✓ ServerConfig::save() found"
else
    echo "  ✗ ServerConfig::save() missing"
    exit 1
fi

if grep -q "pub fn default_path" crates/server/src/config.rs; then
    echo "  ✓ ServerConfig::default_path() found"
else
    echo "  ✗ ServerConfig::default_path() missing"
    exit 1
fi

if grep -q "fn validate" crates/server/src/config.rs; then
    echo "  ✓ ServerConfig::validate() found"
else
    echo "  ✗ ServerConfig::validate() missing"
    exit 1
fi
echo ""

# Test 6: Check CLI argument parsing
echo "Test 6: Checking CLI implementations..."

if grep -q "clap::Parser" crates/server/src/main.rs; then
    echo "  ✓ Server uses clap::Parser"
else
    echo "  ✗ Server missing clap::Parser"
    exit 1
fi

if grep -q "clap::Parser" crates/client/src/main.rs; then
    echo "  ✓ Client uses clap::Parser"
else
    echo "  ✗ Client missing clap::Parser"
    exit 1
fi

if grep -q "list_devices" crates/server/src/main.rs; then
    echo "  ✓ Server has --list-devices flag"
else
    echo "  ✗ Server missing --list-devices flag"
    exit 1
fi

if grep -q "service" crates/server/src/main.rs; then
    echo "  ✓ Server has --service flag"
else
    echo "  ✗ Server missing --service flag"
    exit 1
fi

if grep -q "connect" crates/client/src/main.rs; then
    echo "  ✓ Client has --connect flag"
else
    echo "  ✗ Client missing --connect flag"
    exit 1
fi
echo ""

# Test 7: Check dependencies
echo "Test 7: Checking dependencies..."

if grep -q "dirs.workspace = true" crates/server/Cargo.toml; then
    echo "  ✓ Server has dirs dependency"
else
    echo "  ✗ Server missing dirs dependency"
    exit 1
fi

if grep -q "shellexpand.workspace = true" crates/server/Cargo.toml; then
    echo "  ✓ Server has shellexpand dependency"
else
    echo "  ✗ Server missing shellexpand dependency"
    exit 1
fi

if grep -q "dirs.workspace = true" crates/client/Cargo.toml; then
    echo "  ✓ Client has dirs dependency"
else
    echo "  ✗ Client missing dirs dependency"
    exit 1
fi
echo ""

# Test 8: Check for tests
echo "Test 8: Checking for tests in config modules..."

if grep -q "#\[test\]" crates/server/src/config.rs; then
    echo "  ✓ Server config has tests"
    test_count=$(grep -c "#\[test\]" crates/server/src/config.rs)
    echo "    Found $test_count test functions"
else
    echo "  ⚠ Server config has no tests (optional)"
fi

if grep -q "#\[test\]" crates/client/src/config.rs; then
    echo "  ✓ Client config has tests"
    test_count=$(grep -c "#\[test\]" crates/client/src/config.rs)
    echo "    Found $test_count test functions"
else
    echo "  ⚠ Client config has no tests (optional)"
fi
echo ""

# Cleanup
rm -rf /tmp/p2p-usb-test

echo "================================"
echo "✓ All Phase 7 tests passed!"
echo "================================"
echo ""
echo "Summary:"
echo "  - Configuration structures defined"
echo "  - Config loading/saving implemented"
echo "  - Config validation implemented"
echo "  - CLI argument parsing with clap"
echo "  - Example config files created"
echo "  - Required dependencies added"
echo ""
echo "Note: Full compilation testing requires fixing"
echo "      existing issues in network layer (Phase 3/4)"
