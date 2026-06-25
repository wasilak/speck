---
phase: 02-ffi-bridge
plan: 01
subsystem: speck-vz
tags: [ffi, objc2, dispatch, threading, error-types]
requires: []
provides: [objc2-deps, error-type, vm-thread-infra]
affects: [speck-vz]
tech-stack:
  added:
    - objc2 0.6 (objc2-proc-macros feature)
    - objc2-foundation 0.3 (NSURL, NSError)
    - objc2-virtualization 0.3 (VZVirtualMachineConfiguration, VZVirtualMachine, VZLinuxBootLoader, VZVirtualMachineDelegate)
    - block2 0.6
    - dispatch2 0.3
    - thiserror 2
    - tokio 1.52 (sync feature)
  patterns:
    - Serial GCD dispatch queue per VZVirtualMachine threading requirement
    - mpsc command channel + oneshot reply channel IPC pattern
    - Dedicated OS thread owning the dispatch queue
    - Error enum with thiserror derive
key-files:
  created:
    - crates/speck-vz/src/error.rs
    - crates/speck-vz/src/config.rs
    - crates/speck-vz/src/vm_thread.rs
  modified:
    - crates/speck-vz/Cargo.toml
    - crates/speck-vz/src/lib.rs
decisions:
  - Use dispatch2::DispatchQueue (non-deprecated name) instead of deprecated Queue alias
  - Use tokio::sync::mpsc instead of std::sync::mpsc for tokio compatibility
  - ChannelError takes String instead of #[from] mpsc::SendError (tokio mpsc Sender is parameterized)
  - InternalState is public (returned by VmThread::state()) and re-exported at crate root
  - Use objc2-proc-macros feature instead of nonexistent declare_class feature
metrics:
  duration: ~20min
  completed_date: 2026-06-25
---

# Phase 02 Plan 01: Binding & VmThread Foundation

**One-liner:** Added objc2 ecosystem deps to speck-vz, created Error types, and built VmThread with a serial dispatch queue + mpsc/oneshot command channel — the FFI foundation for all subsequent Phase 2 plans.

## Objective

Add the objc2 ecosystem as dependencies to speck-vz, establish the threading infrastructure (VmThread with dispatch queue + command channel pattern), and create the Error type. The binding decision (objc2-virtualization) is validated empirically — the crate compiles against Xcode 16.4 SDK headers.

## Verification Results

| Check | Result |
|-------|--------|
| `cargo check -p speck-vz` | ✅ PASS (exit 0) |
| `cargo clippy -p speck-vz -- -D warnings` | ✅ PASS (no issues) |
| `grep -q 'objc2-virtualization' Cargo.toml` | ✅ Found |
| `grep -q 'pub enum Error' error.rs` | ✅ Found |
| `grep -q 'struct VmThread' vm_thread.rs` | ✅ Found |
| `grep -q 'VmCommand' vm_thread.rs` | ✅ Found |
| `grep -q 'pub mod error' lib.rs` | ✅ Found |
| `grep -q 'mod vm_thread' lib.rs` | ✅ Found |

## Tasks Executed

### Task 1: Add objc2 ecosystem dependencies to speck-vz

**Files:** `crates/speck-vz/Cargo.toml`

Replaced the minimal dependencies section with the full objc2 ecosystem:
- `objc2` 0.6 with `objc2-proc-macros` feature (for `declare_class!` macro)
- `objc2-foundation` 0.3 with `NSURL`, `NSError` feature flags
- `objc2-virtualization` 0.3 with `VZVirtualMachineConfiguration`, `VZVirtualMachine`, `VZLinuxBootLoader`, `VZVirtualMachineDelegate` feature flags
- `block2` 0.6 (Obj-C block closure support)
- `dispatch2` 0.3 (GCD serial queue)
- `thiserror` 2 (Error derive)
- `tokio` 1.52 with `sync` feature (oneshot channels)

### Task 2: Create Error types

**Files:** `crates/speck-vz/src/error.rs`

`Error` enum with 7 variants: `VmFramework`, `AlreadyRunning`, `NotRunning`, `StartTimeout`, `StopTimeout`, `ChannelError(String)`, `ThreadJoin`. Plus `pub type Result<T>`.

### Task 3: Create VmThread with dispatch queue infrastructure

**Files:** `crates/speck-vz/src/vm_thread.rs`

Core threading and IPC infrastructure:
- `VmCommand` enum (pub(crate)) with `Start { config, reply }`, `Stop { reply }`, `State { reply }`, `Shutdown`
- `InternalState` enum (pub) with `Stopped`, `Starting`, `Running`, `Stopping` + `From<InternalState> for VmState`
- `VmThread` struct (pub, Send + Sync) with `sender: mpsc::Sender` + `thread: Option<JoinHandle>`
- `spawn()` — creates `DispatchQueue::new("com.speck.vm", DispatchQueueAttr::SERIAL)` and a named OS thread
- Thread loop receives commands via `rx.blocking_recv()`, dispatches each via `Queue::exec_sync`
- `send_blocking()` — generic helper sending a command and awaiting the oneshot reply
- Public methods: `start()`, `stop()`, `state()`, `join()`
- `GuestConfig` stub created in `config.rs` so `Start` variant compiles

### Task 4: Update lib.rs and verify compilation

**Files:** `crates/speck-vz/src/lib.rs`, `crates/speck-vz/src/config.rs`

Updated `lib.rs` to declare modules (`pub mod error`, `pub mod config`, `mod vm_thread`) and re-export `Error`, `Result`, `EventSink`, `EngineEvent`, `VmState`, `VmThread`, `InternalState`. Preserved `version()` and `accepts_sink()` stubs from Phase 1.

Created `config.rs` stub with `GuestConfig { kernel_path, cpu_count, memory_size_bytes }` — placeholder until 02-02.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 — Bug] Fixed nonexistent `declare_class` feature name**

- **Found during:** Task 1, compile check
- **Issue:** objc2 0.6.x does not have a `declare_class` feature — this was renamed to `objc2-proc-macros` in the objc2 ecosystem
- **Fix:** Changed `features = ["declare_class"]` to `features = ["objc2-proc-macros"]` in Cargo.toml
- **Files modified:** `crates/speck-vz/Cargo.toml`

**2. [Rule 2 — Missing critical] Used `DispatchQueue` instead of deprecated `Queue` alias**

- **Found during:** Implementation
- **Issue:** The plan specified `Queue::serial()` which uses the deprecated `Queue` alias. dispatch2 0.3 renamed `Queue` → `DispatchQueue` and replaced `Queue::serial()` with `DispatchQueue::new(label, DispatchQueueAttr::SERIAL)`.
- **Fix:** Used the non-deprecated `DispatchQueue` API throughout
- **Files modified:** `crates/speck-vz/src/vm_thread.rs`

**3. [Rule 2 — Missing critical] Used `tokio::sync::mpsc` instead of `std::sync::mpsc`**

- **Found during:** Implementation
- **Issue:** The plan specified `std::sync::mpsc` but tokio is already a dependency for `oneshot` channels. Using tokio's mpsc provides `blocking_recv` which integrates better with the mixed sync/async pattern.
- **Fix:** Used `tokio::sync::mpsc` throughout; changed `ChannelError` variant to `String` (tokio's mpsc `SendError<_>` wraps the value differently than std's)
- **Files modified:** `crates/speck-vz/src/vm_thread.rs`, `crates/speck-vz/src/error.rs`

**4. [Rule 3 — Blocking] Made `VmThread` and `InternalState` public instead of pub(crate)**

- **Found during:** Implementation
- **Issue:** Public API methods on `VmThread` return `InternalState`, requiring it to be at least as visible as the struct. Plan specified both as `pub(crate)`.
- **Fix:** Made `InternalState` public, re-exported both `VmThread` and `InternalState` from `lib.rs`
- **Files modified:** `crates/speck-vz/src/vm_thread.rs`, `crates/speck-vz/src/lib.rs`

## Key Decisions

1. **`InternalState` is public** — returned from `VmThread::state()`, so external callers need the type. Mirrors `VmState` from speck-core with `From` conversion.

2. **`GuestConfig` stub in `config.rs`** — minimal placeholder allowing the `Start` command to compile. Will be replaced by the full builder in 02-02.

3. **`#[allow(dead_code)]` on `start()`, `stop()`, `state()`** — these methods exist for the public API contract but won't be called until 02-02 or later. The `VmThread` struct itself is already used via the `pub use` re-export.

4. **`move` closures into `exec_sync`** — `oneshot::Sender` is moved into the closure and consumed by `.send()`. The `Send` requirement of `exec_sync` is satisfied because `Sender<T>` is `Send` when `T: Send`.

## Commits

| Hash | Message |
|------|---------|
| `c606539` | `feat(02-01): add objc2 binding deps, Error types, and VmThread foundation` |

## No Auth Gates

No authentication gates were encountered — all dependencies are local crates and public crates.io packages.

## No Known Stubs

The `config.rs` `GuestConfig` is intentionally a minimal placeholder documented as such. All other code is production-quality with no TODO/FIXME stubs.

## Threat Flags

None — all threat-model-mitigated items (T-02-01-01 through T-02-01-03) are correctly addressed in the implementation:
- **T-02-01-01** (Tampering): Start checks state before dispatching
- **T-02-01-02** (DoS): Channel errors propagated via `ChannelError` variant; `join()` detects panics
- **T-02-01-03** (Tampering): All VZVirtualMachine access goes through `exec_sync` on the serial queue (pattern established, actual access in 02-02)

## Self-Check: PASSED

- ✅ `cargo check -p speck-vz` exit 0
- ✅ `cargo clippy -p speck-vz -- -D warnings` exit 0
- ✅ All 5 files created/modified as specified
- ✅ Commit `c606539` verified in git log
- ✅ No unexpected file deletions
