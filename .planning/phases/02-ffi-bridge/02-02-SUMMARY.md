---
phase: 02-ffi-bridge
plan: 02
subsystem: speck-vz
tags: [ffi, objc2, virtualization, vm-boot, delegate, define_class, guest-api]
requires:
  - phase: 02-01
    provides: [VmThread dispatch-queue, Error types, mpsc IPC pattern, GuestConfig stub]
provides:
  - VmDelegate Obj-C class (define_class!) implementing VZVirtualMachineDelegate
  - Guest struct (public API wrapping VmThread)
  - Full GuestConfig (kernel_path, initrd_path, cmdline, cpu_count, memory_size_bytes, stop_timeout + builder)
  - do_start in VmThread: VZLinuxBootLoader + VZVirtualMachineConfiguration + VZVirtualMachine boot
  - scripts/fetch-kernel.sh for downloading Kata arm64 kernel + initrd
  - test_guest_boots_to_running integration test
affects: [02-03, 03-vsock-echo, 04-guest-networking, speck-vz]

tech-stack:
  added:
    - define_class! macro (objc2 0.6 — replaces declare_class! which no longer exists)
  patterns:
    - VmDelegate ivar pattern — callback stored as *mut c_void + AnyThread for Obj-C safe access
    - Guest as thin public API wrapper over VmThread (hides all ObjC/dispatch details)
    - GuestConfigBuilder pattern for ergonomic config construction
    - Kernel path via $SPECK_HOME env var (avoids hardcoded paths)

key-files:
  created:
    - crates/speck-vz/src/delegate.rs
    - crates/speck-vz/src/guest.rs
    - scripts/fetch-kernel.sh
  modified:
    - crates/speck-vz/src/config.rs (full GuestConfig + builder replacing stub)
    - crates/speck-vz/src/vm_thread.rs (do_start: VZLinuxBootLoader, VZVirtualMachineConfiguration, VZVirtualMachine)
    - crates/speck-vz/src/lib.rs (export Guest, GuestConfig)

key-decisions:
  - "define_class! is the correct objc2 0.6 macro — declare_class! was renamed"
  - "VmStateEvent simplified to Stopped/Error (no DidRequestStop — VZ doesn't fire that in practice)"
  - "Callback stored as *mut c_void ivar with AnyThread — direct Rust field impossible with AnyThread constraint"
  - "GuestConfig.stop_timeout defaulted to 10s (not 30s) — matches VM shutdown observed timing"
  - "fetch-kernel.sh in scripts/ (not xtask) — simpler to update and doesn't require Rust rebuild"

patterns-established:
  - "define_class! ivars pattern: store data as *mut c_void, cast back in method impls"
  - "Guest facade: all pub methods delegate to VmThread via mpsc command channel"

requirements-completed: [ENGINE-01]

duration: retrospective (implemented across Phase 2–3 execution sessions)
completed: 2026-06-26
---

# Phase 02 Plan 02: Guest Struct + VM Boot

**One-liner:** Implemented VmDelegate (define_class! Obj-C class), Guest public API, full GuestConfig, and VmThread's do_start wiring VZLinuxBootLoader + VZVirtualMachineConfiguration — the empirical validation that objc2-virtualization boots a real arm64 Linux kernel on Apple Silicon.

## Retrospective Note

This SUMMARY.md is written retrospectively. The plan's functionality was implemented during Phase 2 execution but spread across multiple commits (primarily `2b47c4e`, `2f0cd5e feat(03-02)`, `1fe2a66 feat(03-01)`) without dedicated `feat(02-02)` commits. The `delegate.rs` file was left untracked; it was committed as `2b47c4e` during Phase 2 closeout.

## Must-Haves Verified

| Must-Have | Status |
|-----------|--------|
| VZLinuxBootLoader configured with real arm64 kernel | ✅ do_start in vm_thread.rs |
| VZVirtualMachineConfiguration built and validated | ✅ validateWithError: called |
| VZVirtualMachine::startWithCompletionHandler: called | ✅ block2 completion handler |
| Obj-C delegate (VZVirtualMachineDelegate) in Rust via define_class! | ✅ delegate.rs |
| VM transitions Stopped → Starting → Running | ✅ InternalState machine |
| Boot completes in milliseconds | ✅ ~120ms per STATE.md Phase 2 summary |

## Accomplishments

- `delegate.rs`: `VmDelegate` Obj-C class via `define_class!` implements `guestDidStopVirtualMachine:` and `virtualMachine:didStopWithError:`. Callback propagated to `VmThread` via `mpsc::Sender<VmStateEvent>`.
- `guest.rs`: `Guest` struct is the sole public entry point — wraps `VmThread` with `new()`, `with_sink()`, `start()`, `stop()`, `state()`, `vsock_connect()`, `join()`, `netstack_fd()`, `dns_vsock_fd()`. `Drop` sends `Shutdown`.
- `config.rs`: Full `GuestConfig` with `kernel_path`, `initrd_path`, `cmdline`, `cpu_count`, `memory_size_bytes`, `stop_timeout`, `vsock_port`, `network`, `dns_vsock_port`. `GuestConfigBuilder` pattern. `validate()` method.
- `vm_thread.rs`: `do_start` creates `VZLinuxBootLoader` with kernel URL + initrd + cmdline, builds `VZVirtualMachineConfiguration`, validates, wires `VmDelegate`, creates `VZVirtualMachine`, calls `startWithCompletionHandler:`.
- `scripts/fetch-kernel.sh`: Downloads Kata Containers arm64 kernel + initrd to `$SPECK_HOME`.
- `test_guest_boots_to_running` test in `guest.rs` (gated on kernel availability via `$SPECK_HOME`).

## Files Created/Modified

- `crates/speck-vz/src/delegate.rs` — `VmDelegate` Obj-C class + `VmStateEvent` enum
- `crates/speck-vz/src/guest.rs` — `Guest` public API + boot/shutdown tests
- `crates/speck-vz/src/config.rs` — Full `GuestConfig` + `GuestConfigBuilder` + `validate()`
- `crates/speck-vz/src/vm_thread.rs` — `do_start`: boot loader, config, VM creation, delegate wiring
- `crates/speck-vz/src/lib.rs` — exports `Guest`, `GuestConfig`
- `scripts/fetch-kernel.sh` — Kata kernel + initrd download script

## Deviations from Plan

**1. `define_class!` not `declare_class!`**
- objc2 0.6 renamed `declare_class!` → `define_class!`. Used the correct current API.

**2. `VmStateEvent` simplified to `Stopped` / `Error`**
- Plan specified `DidStop`, `DidStopWithError`, `DidRequestStop`. In practice, `VZVirtualMachine` does not fire `guestDidRequestStop:` during clean shutdown. Simplified to the two events that actually matter.

**3. Callback via `*mut c_void` ivar (not a typed Rust field)**
- `define_class!` with `thread_kind = AnyThread` requires `AnyThread` on all stored types. A `Box<dyn Fn + Send>` field doesn't satisfy `AnyThread`. Solution: store as raw pointer, cast safely inside the method (queue invariant guarantees no concurrent access).

**4. `GuestConfig` extended beyond plan scope**
- `vsock_port`, `network`, `dns_vsock_port` added during Phase 3/4 work, not Phase 2. Captured here for accuracy.

**5. `scripts/fetch-kernel.sh` not `cargo xtask fetch-kernel`**
- Shell script is simpler to iterate on and doesn't require `cargo build` to use.

## Commits

| Hash | Message | Context |
|------|---------|---------|
| `c606539` | `feat(02-01)`: GuestConfig stub | Earlier plan, config stub only |
| `1fe2a66` | `feat(03-01)`: vsock foundation | Extended GuestConfig, vm_thread.rs |
| `2f0cd5e` | `feat(03-02)`: vsock wiring | Created guest.rs, extended vm_thread.rs do_start |
| `2b47c4e` | `feat(02-02)`: VmDelegate retrospective | delegate.rs committed during closeout |

## Self-Check: PASSED

- ✅ `delegate.rs` implements `VZVirtualMachineDelegate` via `define_class!`
- ✅ `guest.rs` defines `pub struct Guest` with `start()`, `stop()`, `state()`
- ✅ `config.rs` has full `GuestConfig` with `stop_timeout`
- ✅ `vm_thread.rs` references `VZLinuxBootLoader` and `VZVirtualMachine`
- ✅ VM boots to Running state confirmed in STATE.md Phase 2 Summary (~120ms)
- ✅ `scripts/fetch-kernel.sh` downloads Kata arm64 kernel
