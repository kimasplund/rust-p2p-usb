---
description: Run project test suite with optional pattern filtering
argument-hint: [test-pattern]
allowed-tools: [Bash, Read, Grep, Glob]
model: claude-sonnet-4-5
---

# Project Test Suite

Run tests for the rust-p2p-usb project with optional test pattern filtering.

## Usage

```bash
/test                    # Run all tests
/test usb_device         # Run tests matching "usb_device"
/test --workspace        # Run tests for entire workspace
```

## Implementation

This project uses `cargo test` for testing.

```bash
# Determine test command based on arguments
if [ -z "$1" ]; then
  # Run all tests
  cargo test --all
else
  # Run tests matching pattern
  cargo test "$@"
fi
```

## Success Criteria

- [ ] All tests pass
- [ ] No test failures or errors
- [ ] Output shows test results clearly
