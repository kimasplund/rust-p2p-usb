---
description: Run performance benchmarks for USB/network throughput
argument-hint: [bench-target]
allowed-tools: [Bash, Read, Write, Grep]
model: claude-sonnet-4-5
---

# Performance Benchmarks

Run performance benchmarks for rust-p2p-usb, focusing on USB throughput and network latency.

## Usage

```bash
/bench                   # Run all benchmarks
/bench usb_transfer      # Run specific benchmark
/bench --save baseline   # Save results as baseline
```

## Implementation

This project uses `cargo bench` with criterion for benchmarking.

```bash
# Run benchmarks
if [ -z "$1" ]; then
  # Run all benchmarks
  cargo bench --all
elif [ "$1" = "--save" ] && [ -n "$2" ]; then
  # Save baseline
  cargo bench --all -- --save-baseline "$2"
else
  # Run specific benchmark
  cargo bench "$@"
fi
```

## Performance Targets

- **USB Latency**: 5-20ms for control transfers
- **Throughput**: 80-90% of USB 2.0 bandwidth
- **Memory**: <50MB RSS per server process
- **CPU**: <5% on Raspberry Pi 4

## Success Criteria

- [ ] All benchmarks complete successfully
- [ ] Results meet performance targets
- [ ] No performance regressions vs baseline
