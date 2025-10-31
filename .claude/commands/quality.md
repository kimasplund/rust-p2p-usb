---
description: Run full quality checks (fmt, clippy, audit, tests)
argument-hint: ""
allowed-tools: [Bash, Read]
model: claude-sonnet-4-5
---

# Quality Checks

Run comprehensive quality checks for rust-p2p-usb project.

## Usage

```bash
/quality              # Run all quality checks
```

## Checks Performed

1. **cargo fmt** - Code formatting
2. **cargo clippy** - Linting
3. **cargo test** - Unit tests
4. **cargo audit** - Security audit

## Implementation

Runs all quality checks in sequence.

```bash
echo "Running quality checks for rust-p2p-usb..."
echo ""

# Track failures
FAILED=0

# 1. Check formatting
echo "1/4 Checking code formatting..."
if cargo fmt --all -- --check; then
  echo "✅ Formatting OK"
else
  echo "❌ Formatting issues found. Run: cargo fmt"
  FAILED=1
fi
echo ""

# 2. Run clippy
echo "2/4 Running clippy lints..."
if cargo clippy --all -- -D warnings; then
  echo "✅ Clippy OK"
else
  echo "❌ Clippy warnings found"
  FAILED=1
fi
echo ""

# 3. Run tests
echo "3/4 Running tests..."
if cargo test --all; then
  echo "✅ Tests OK"
else
  echo "❌ Tests failed"
  FAILED=1
fi
echo ""

# 4. Security audit
echo "4/4 Running security audit..."
if cargo audit; then
  echo "✅ Audit OK"
else
  echo "⚠️  Security vulnerabilities found"
  FAILED=1
fi
echo ""

# Summary
echo "================================"
if [ $FAILED -eq 0 ]; then
  echo "✅ All quality checks passed!"
  exit 0
else
  echo "❌ Some quality checks failed"
  exit 1
fi
```

## Success Criteria

- [ ] Code is properly formatted
- [ ] No clippy warnings
- [ ] All tests pass
- [ ] No security vulnerabilities
