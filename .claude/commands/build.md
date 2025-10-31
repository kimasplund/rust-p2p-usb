---
description: Build project (dev/release/cross-compile)
argument-hint: [mode]
allowed-tools: [Bash, Read]
model: claude-sonnet-4-5
---

# Build Project

Build the rust-p2p-usb project in various modes.

## Usage

```bash
/build              # Build in debug mode
/build release      # Build optimized release
/build dev          # Build in debug mode (alias)
```

## Implementation

Uses `cargo build` with appropriate flags.

```bash
case "${1:-dev}" in
  release)
    echo "Building release with optimizations..."
    cargo build --release --all
    ;;
  dev|debug)
    echo "Building debug version..."
    cargo build --all
    ;;
  *)
    echo "Unknown build mode: $1"
    echo "Usage: /build [dev|release]"
    exit 1
    ;;
esac

echo ""
echo "Build complete!"
echo "Binaries location: target/${1:-debug}/"
```

## Success Criteria

- [ ] Build completes without errors
- [ ] All workspace crates compile successfully
- [ ] Binaries created in target directory
