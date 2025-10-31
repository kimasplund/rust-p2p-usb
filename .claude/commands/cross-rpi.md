---
description: Cross-compile for Raspberry Pi (aarch64)
argument-hint: ""
allowed-tools: [Bash, Read]
model: claude-sonnet-4-5
---

# Cross-Compile for Raspberry Pi

Cross-compile rust-p2p-usb for Raspberry Pi (aarch64-unknown-linux-gnu).

## Usage

```bash
/cross-rpi              # Cross-compile release build for RPi
```

## Prerequisites

Requires `cross` to be installed:
```bash
cargo install cross
```

## Implementation

Uses `cross` for cross-compilation to aarch64.

```bash
# Check if cross is installed
if ! command -v cross &> /dev/null; then
  echo "❌ cross is not installed"
  echo "Install with: cargo install cross"
  exit 1
fi

echo "Cross-compiling for Raspberry Pi (aarch64)..."
cross build --release --target aarch64-unknown-linux-gnu

echo ""
echo "✅ Cross-compilation complete!"
echo "Binaries location: target/aarch64-unknown-linux-gnu/release/"
echo ""
echo "Transfer to Raspberry Pi:"
echo "  scp target/aarch64-unknown-linux-gnu/release/p2p-usb-server pi@raspberrypi:~/"
echo "  scp target/aarch64-unknown-linux-gnu/release/p2p-usb-client pi@raspberrypi:~/"
```

## Success Criteria

- [ ] Cross-compilation completes successfully
- [ ] aarch64 binaries created
- [ ] No compilation errors
