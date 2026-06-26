---
phase: 05-containerd-buildkit-integration
plan: 02
subsystem: guest
tags: [vminitd, containerd, buildkitd, vsock, af_vsock, sock_forwarder, disk-mount, service-supervision]
requires:
  - phase: 04-guest-networking
    provides: dns_forwarder AF_VSOCK pattern (reference for sock_forwarder)
  - phase: 05-01
    provides: GuestConfig disk path fields (vm will use these in 05-04)
provides:
  - sock_forwarder.rs generic vsock->Unix forwarder module
  - vminitd disk mounting (early filesystems + rootfs + data disk)
  - vminitd service supervision (containerd + buildkitd restart loops)
  - vminitd containerd health-check with /rootfs/run/containerd/containerd.sock polling
  - vminitd READY signal on vsock port 9000 (b"READY\n")
  - vminitd forwarder threads for containerd (9001) and buildkitd (9002) gRPC
affects:
  - 05-04 (host-side VZVirtioBlockDeviceConfiguration + WaitForGuestReady — expects READY signal)
  - 05-05 (integration tests — expects containerd reachable via forwarder)

tech-stack:
  added: []
  patterns:
    - "vsock->Unix socket forwarder with dup() per-thread fd ownership"
    - "vminitd health-check-before-readiness: poll AF_UNIX connect with retry"
    - "vminitd service supervision with restart loop and 1s backoff"

key-files:
  created:
    - crates/speck-guest/src/sock_forwarder.rs
  modified:
    - crates/speck-guest/src/lib.rs
    - crates/speck-guest/src/bin/vminitd.rs

key-decisions:
  - "sock_forwarder uses libc-only I/O (no async, no tokio) matching dns_forwarder.rs pattern exactly"
  - "vsock listen fd stays open across accepts (supports reconnects from host)"
  - "Per-connection dup() gives each direction thread independent fd ownership"
  - "vminitd stays in initrd (no switch_root) — uses /rootfs/... paths for socket file"
  - "VSOCK listen accepts one READY connection then closes (one-shot handshake)"

patterns-established:
  - "unix_connect() — AF_UNIX connect via libc socket+bind+connect with sockaddr_un"
  - "proxy_copy() — bidirectional byte copy via read/write loop with partial-write handling"
  - "wait_for_containerd_socket() — poll AF_UNIX connect with 200ms interval"

requirements-completed: [RUN-06]

duration: 12min
completed: 2026-06-26
---

# Phase 05 Plan 02: Guest-side vsock forwarder + vminitd service supervision

**Generic vsock-to-Unix socket bidirectional forwarder integrated into vminitd, with disk mounting, containerd/buildkitd restart supervision, health-check polling, READY signal, and forwarder threads**

## Performance

- **Duration:** 12 min
- **Started:** 2026-06-26T15:45:00Z
- **Completed:** 2026-06-26T15:57:00Z
- **Tasks:** 2
- **Files modified:** 3

## Accomplishments

- New `sock_forwarder` module (166 lines) provides `pub fn serve(vsock_port, unix_path)` — loops accepting vsock connections, proxying each bidirectionally to a Unix domain socket via two threads (one per direction), with `dup()`-based fd ownership
- Private `unix_connect()` helper creates AF_UNIX SOCK_STREAM connections
- Private `proxy_copy()` helper handles partial writes via read/write loop with 16KB buffer
- vminitd extended to 11 new private functions covering the full boot lifecycle
- `mount_early_filesystems()` mounts /proc (procfs), /sys (sysfs), /dev (devtmpfs) before any disk operations (non-fatal)
- `mount_disks()` mounts /dev/vda→/rootfs as ext4 (fatal on failure), formats /dev/vdb with mke2fs if no filesystem detected (ENODEV), mounts it at /rootfs/var/lib/containerd, creates /rootfs/run/ and /rootfs/tmp/
- `wait_for_containerd_socket()` polls `/rootfs/run/containerd/containerd.sock` via AF_UNIX connect with 50 attempts at 200ms intervals
- `send_ready_signal()` vsock listen/accept/write "READY\n" on configurable port (default 9000)
- `spawn_service_with_restart()` restart-loop thread with 1s backoff for containerd and buildkitd
- Forwarder threads for containerd (port 9001) and buildkitd (port 9002) using `sock_forwarder::serve()`
- Old `vsock_echo::serve(port)` call removed from main (module stays for Phase 3 tests)
- PID 1 never exits (infinite sleep loop)

## Task Commits

Each task was committed atomically:

1. **Task 1: Create sock_forwarder.rs — generic vsock→Unix socket bidirectional forwarder** — `3864bca` (feat)
2. **Task 2: Extend vminitd — disk mounting, service supervision, READY signal, forwarder threads** — `6ccbc24` (feat)

**Plan metadata:** `(committed as part of Task 2 above)`

## Files Created/Modified

- `crates/speck-guest/src/sock_forwarder.rs` — NEW: Generic vsock→Unix socket forwarder module
- `crates/speck-guest/src/lib.rs` — Added `pub mod sock_forwarder` declaration
- `crates/speck-guest/src/bin/vminitd.rs` — Extended from 51→453 lines with full boot lifecycle

## Verification Results

- `cargo check --target aarch64-unknown-linux-musl -p speck-guest` — ✅ PASS
- `cargo check --target aarch64-unknown-linux-musl --bin vminitd` — ✅ PASS
- `cargo clippy --target aarch64-unknown-linux-musl -p speck-guest -- -Dwarnings` — ⚠️ NOTE: clippy on `aarch64-unknown-linux-musl` requires `build-std` (nightly-only feature). `cargo check` passes cleanly for same target.

## Acceptance Criteria Verification

| Criterion | Status |
|-----------|--------|
| `sock_forwarder::serve(port, path)` loops accepting vsock and proxying to Unix socket | ✅ |
| `sock_forwarder.rs` starts with `#![cfg(target_os = "linux")]` | ✅ |
| `sock_forwarder.rs` contains `pub fn serve` | ✅ |
| `sock_forwarder.rs` contains `fn unix_connect` | ✅ |
| `lib.rs` contains `pub mod sock_forwarder` | ✅ |
| vminitd contains `fn mount_disks` | ✅ |
| vminitd contains `fn send_ready_signal` | ✅ |
| vminitd contains `fn wait_for_containerd_socket` | ✅ |
| vminitd contains `fn spawn_service_with_restart` | ✅ |
| vminitd contains `fn parse_cmdline_containerd_vsock_port` | ✅ |
| vminitd contains `fn parse_cmdline_ready_vsock_port` | ✅ |
| vminitd contains `sock_forwarder::serve` in main() | ✅ |
| main() ends with infinite sleep loop | ✅ |
| Old `vsock_echo::serve(port)` call removed from main() | ✅ |

## Decisions Made

- **Followed plan as specified** — no architectural decisions deviated from the PLAN.md
- Used `Vec<&'static str>` for `spawn_service_with_restart` args parameter instead of `&'static [&'static str]` due to Rust lifetime constraints (temporary array references don't have `'static` lifetime). Same behavior, same compile-time constants, owned Vec moved into thread closure.
- `sockaddr_un.sun_path` is `[u8; 108]` (not `[i8; 108]`) on the musl target — direct byte copy used without transmute.

## Deviations from Plan

None — plan executed as written.

## Threat Flags

| Flag | File | Description |
|------|------|-------------|
| `threat_flag: execution` | `crates/speck-guest/src/bin/vminitd.rs` (`mount_disks` at line 256) | `libc::system("mke2fs -t ext4 /dev/vdb")` introduces a shell execution path, but the command string is a compile-time constant and is only invoked when /dev/vdb has no filesystem (ENODEV on first boot). No user-controlled input. Acceptable for Phase 5. |

## Issues Encountered

- `sockaddr_un.sun_path` on the musl target is `[u8; 108]` not `[i8; 108]` — fixed by using direct byte indexing instead of `&mut [i8]` reference.
- `cargo clippy` on `aarch64-unknown-linux-musl` requires nightly `build-std` feature; the `cargo check` verification passes cleanly. This is a pre-existing infrastructure limitation, not a code issue.
- Workspace `cargo check` fails for `speck-guest` on macOS host (expected — `sockaddr_vm` struct differs between macOS libc and Linux musl). The crate is `#[cfg(target_os = "linux")]` gated and designed for cross-compilation only.

## Stub Tracking

None — all code paths are fully wired:
- `sock_forwarder::serve()` loops accepting connections (not a one-shot)
- `wait_for_containerd_socket()` properly polls and returns bool
- `send_ready_signal()` properly writes and reports failure
- `spawn_service_with_restart()` runs actual process lifecycle

## Known Stubs

None.

## Next Phase Readiness

- **Phase 05-03** (rootfs CI + fetch) can proceed independently — vminitd now expects `/rootfs/usr/bin/containerd` etc.
- **Phase 05-04** (host-side disk attach + WaitForGuestReady) can now rely on the guest READY signal protocol — vminitd will bind ready_port (9000), accept host connection, and write "READY\n" after containerd is healthy
- vminitd has cmdline parsing for `containerd_vsock_port`, `buildkitd_vsock_port`, `ready_vsock_port` — the host must pass these as kernel cmdline arguments
- All forwarder Unix socket paths use `/rootfs/...` prefix (no switch_root in Phase 5)

## Self-Check: PASSED

- ✅ `crates/speck-guest/src/sock_forwarder.rs` exists (5.3K)
- ✅ `.planning/phases/05-containerd-buildkit-integration/05-02-SUMMARY.md` exists (8.5K)
- ✅ Commit `3864bca` — feat(05-02): add generic vsock→Unix socket bidirectional forwarder
- ✅ Commit `6ccbc24` — feat(05-02): extend vminitd with disk mounting, service supervision, READY signal, forwarders
- ✅ Commit `f07b432` — docs(05-02): complete Phase 5 Plan 2
- ✅ `cargo check --target aarch64-unknown-linux-musl -p speck-guest` passes
- ✅ `cargo check --target aarch64-unknown-linux-musl --bin vminitd` passes
- ✅ ROADMAP.md updated (05-02 marked complete, 2/5 plans)
- ✅ STATE.md updated (completed_plans: 18, percent: 86)

---

*Phase: 05-containerd-buildkit-integration*
*Completed: 2026-06-26*
