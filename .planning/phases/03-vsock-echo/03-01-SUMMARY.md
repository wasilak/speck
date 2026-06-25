---
phase: 03
plan: 01
subsystem: speck-vz
tags: [vsock, virtio-socket, foundation, vminitd]
requires: []
provides: [vsock-wrap, vsock-port-config]
affects: [speck-vz]
tech-stack:
  added: [libc]
  patterns: [unsafe-fd-wrapper, drop-close]
key-files:
  created:
    - crates/speck-vz/src/vsock.rs
  modified:
    - crates/speck-vz/Cargo.toml
    - crates/speck-vz/src/error.rs
    - crates/speck-vz/src/config.rs
    - crates/speck-vz/src/lib.rs
key-decisions:
  - VzSocket uses raw libc read/write/close instead of std::os::unix::io::OwnedFd for minimum dependency. The fd ownership is handled manually with a Drop impl.
  - Default vsock port is 1234 — the same convention vminitd will use inside the guest.
duration: 3 min 34 sec
completed: 2026-06-25T19:59:35Z
task_count: 1
file_count: 5
---

# Phase 03 Plan 01: Foundation — Summary

Vsock foundation for the speck-vz crate: feature flags, error variants, `VzSocket` wrapper around raw file descriptors, and `vsock_port` config field.

## Changes

### Feature flags (`Cargo.toml`)
- Added `libc = "0.2"` dependency
- Added to `objc2-virtualization` features:
  - `VZSocketDeviceConfiguration`
  - `VZSocketDevice`
  - `VZVirtioSocketDeviceConfiguration`
  - `VZVirtioSocketDevice`
  - `VZVirtioSocketConnection`

### Error variants (`error.rs`)
- `VsockConnect(String)` — connect to guest vsock port failed
- `VsockTimeout` — no guest listening on the vsock port
- `VsockIo(std::io::Error)` — I/O error on vsock socket read/write

### New module (`vsock.rs`)
- `VzSocket` wraps a raw file descriptor obtained from `VZVirtioSocketConnection::fileDescriptor`
- `pub(crate) unsafe fn from_raw_fd()` — constructor (takes ownership of fd)
- `read(&self, buf)` — libc `read` syscall
- `write(&self, buf)` — libc `write` syscall
- `Drop` impl calls `libc::close` safely

### Config (`config.rs`)
- `GuestConfig.vsock_port: u32` field (default `1234`)
- `GuestConfigBuilder::vsock_port(mut self, port) -> Self` builder method
- Both `Default` impls set `vsock_port: 1234`

### Re-exports (`lib.rs`)
- `mod vsock;`
- `pub use vsock::VzSocket;`

## Task Execution

| # | Task | Type | Status | Commit |
|---|------|------|--------|--------|
| 1 | Add vsock foundation | auto | ✅ Complete | `1fe2a66` |

## Verification

- `cargo check -p speck-vz` — ✅ passes (0 errors, 1 expected `dead_code` warning for `from_raw_fd`)

## Deviations from Plan

None. Plan executed exactly as written.

## Known Stubs

- `VzSocket::from_raw_fd` is `pub(crate)` and currently unused (warning). It will be called by `Guest::vsock_connect()` in a later plan.

## Threat Flags

None — no new network endpoints, auth paths, or trust-boundary changes added.
