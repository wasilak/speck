# Local CI mirror — run `make ci` before pushing to catch all failures locally.
# Mirrors .github/workflows/ci.yml exactly (same flags, same env vars, same order).
#
# Targets:
#   make ci        — full CI sequence (what GitHub runs)
#   make fix       — auto-fix fmt + apply clippy suggestions, then re-check
#   make fmt       — check formatting only
#   make clippy    — clippy with -Dwarnings (same as CI)
#   make check     — cargo check cross-compile to aarch64-apple-darwin
#   make lint      — no-print lint for speck-core
#   make test      — cargo test --workspace

export CARGO_TERM_COLOR := always
export RUSTFLAGS := -Dwarnings

.PHONY: ci fix fmt clippy check lint test

ci: fmt clippy check lint test

# Auto-fix: format first, then surface remaining clippy issues
fix:
	cargo fmt --all
	cargo clippy --all-targets --fix --allow-staged -- -Dwarnings

fmt:
	cargo fmt --all -- --check

clippy:
	cargo clippy --all-targets -- -Dwarnings

# Cross-compile check — same target as CI (Ubuntu runner with aarch64-apple-darwin sysroot)
# On macOS/Apple Silicon this is the native target so it's essentially free.
check:
	cargo check --target aarch64-apple-darwin --workspace

lint:
	@if rg --type rust '(print!|println!|eprint!|eprintln!)\s*\(' crates/speck-core/src/; then \
		echo "FAIL: print macros found in speck-core. Use the EventSink trait instead."; \
		exit 1; \
	fi
	@echo "OK: no print macros in speck-core"

test:
	cargo test --workspace

# CI-equivalent test (Linux-compatible crates only, native host target).
# Use this to verify what CI would run, or when on a Linux machine.
test-ci:
	cargo test --target x86_64-unknown-linux-gnu -p speck-core -p speck-guest
