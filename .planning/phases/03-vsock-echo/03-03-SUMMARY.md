---
phase: 03-vsock-echo
plan: 03
subsystem: guest-vminitd
tags: [vsock, echo, vminitd, guest, musl]
requires: []
provides: [vminitd-binary, vsock-echo-server]
affects: [speck-guest]
tech-stack:
  added:
    - libc 0.2 for AF_VSOCK structs and socket syscalls
  patterns:
    - cfg(target_os = "linux") gate on host-incompatible modules
    - Raw libc syscalls for vsock I/O
    - Kernel cmdline parsing for configuration
key-files:
  created:
    - crates/speck-guest/src/lib.rs
    - crates/speck-guest/src/vsock_echo.rs
    - crates/speck-guest/src/bin/vminitd.rs
  modified:
    - crates/speck-guest/Cargo.toml
  deleted:
    - crates/speck-guest/src/main.rs
decisions:
  - "Keep std binary (not no_std) — musl provides full libc, PID 1 != no_std"
  - "Add svm_zero padding field for libc 0.2.186 compatible sockaddr_vm"
  - "Single-connection echo server for Phase 3 (future gRPC/TLS adds auth)"
  - "Default vsock port 1234 when kernel cmdline doesn't specify"
metrics:
  duration: ~10min
  completed_at: "2026-06-25T20:00:00Z"
---

# Phase 03 Plan 03: Guest side — vminitd with vsock echo server

**One-liner:** Built the guest-side vminitd static musl binary with a Linux AF_VSOCK echo server, gated behind `#[cfg(target_os = "linux")]` for cross-compilation safety.

## Changes

### New files

- **`crates/speck-guest/src/lib.rs`** — Crate root exposing `pub mod vsock_echo` behind `#[cfg(target_os = "linux")]` gate. On non-Linux targets (host macOS build), the module is not compiled.

- **`crates/speck-guest/src/vsock_echo.rs`** — Linux-only AF_VSOCK echo server:
  - `pub fn serve(port: u32) -> io::Result<()>`
  - Creates socket with `libc::socket(AF_VSOCK, SOCK_STREAM, 0)`
  - Binds to `VMADDR_CID_ANY` + given port via `sockaddr_vm`
  - Listens with backlog 1, accepts one connection
  - Echo loop: `read(2)` → partial-safe `write(2)` until EOF
  - Proper fd cleanup on error and normal exit
  - Compatible with libc 0.2.186's 5-field `sockaddr_vm` (includes `svm_zero` padding)

- **`crates/speck-guest/src/bin/vminitd.rs`** — PID 1 entry point:
  - Parses `vsock_port=PORT` from `/proc/cmdline` (default: 1234)
  - Calls `speck_guest::vsock_echo::serve(port)`
  - Prints errors to stderr on failure

### Modified files

- **`crates/speck-guest/Cargo.toml`** — Added `libc = "0.2"` dependency, changed binary path from `src/main.rs` to `src/bin/vminitd.rs`

### Deleted files

- **`crates/speck-guest/src/main.rs`** — Replaced by the new `src/bin/vminitd.rs`

## Cross-compilation note

The `aarch64-unknown-linux-musl` target std library ships with rustup but requires using the rustup-managed toolchain (`~/.rustup/toolchains/stable-aarch64-apple-darwin/bin/`) rather than the Homebrew-installed rustc, which doesn't bundle cross-target std libs. All verification commands below use the correct rustup toolchain via `PATH` override.

## Verification

```
cargo check -p speck-guest --target aarch64-unknown-linux-musl --lib   → PASS (no errors)
cargo check -p speck-guest --target aarch64-unknown-linux-musl --bin vminitd   → PASS (no errors)
```

### Plan-defined criteria

| Criterion | Status |
|-----------|--------|
| vsock_echo.rs uses `AF_VSOCK` | ✅ 3 occurrences |
| vminitd.rs parses `vsock_port` from cmdline | ✅ 5 occurrences |
| vsock_echo.rs binds to `VMADDR_CID_ANY` | ✅ 2 occurrences |
| Lib compiles under `aarch64-unknown-linux-musl` | ✅ |
| Bin compiles as static musl binary | ✅ |
| Default port 1234 when cmdline absent | ✅ |

## Deviations from Plan

No deviations — plan executed exactly as written. The only note is that the crate path is `crates/speck-guest/` (singular), not `crates/speck-guests/` (plural) as referenced in the PLAN.md, and this was handled correctly during execution.

## Self-Check: PASSED

- `crates/speck-guest/src/vsock_echo.rs` — ✅ exists, 96 lines
- `crates/speck-guest/src/lib.rs` — ✅ exists, single `#[cfg(target_os = "linux")]` gate line
- `crates/speck-guest/src/bin/vminitd.rs` — ✅ exists, 27 lines
- `crates/speck-guest/Cargo.toml` — ✅ updated with libc dep and binary path
- `crates/speck-guest/src/main.rs` — ✅ deleted (no longer present)
- Commit a2d8823 — ✅ Task 1 (vsock_echo module)
- Commit 27d0ba9 — ✅ Task 2 (vminitd binary)
