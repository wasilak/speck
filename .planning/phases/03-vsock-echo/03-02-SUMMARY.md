---
phase: 03-vsock-echo
plan: 02
subsystem: vm-core
tags: [vsock, objc2-virtualization, Virtualization.framework, VmThread, dispatch-queue]
requires:
  - phase: 03-01
    provides: VzSocket wrapper, Error variants (VsockConnect/VsockTimeout/VsockIo), VZSocketDevice feature flags
provides:
  - VZVirtioSocketDeviceConfiguration wired into VM config in do_start
  - VZSocketDevice extracted and stored after VM start
  - VmCommand::VsockConnect IPC variant
  - do_vsock_connect handler with connectToPort + fd dup + VzSocket wrapping
  - Guest::vsock_connect(port) public API
affects: [03-03, 03-04]
tech-stack:
  added: []
  patterns:
    - "ObjC completion handler bridging via StackBlock (block2 0.6)"
    - "Send wrapper for non-Send ObjC classes in shared state"
key-files:
  created: []
  modified:
    - crates/speck-vz/src/vm_thread.rs
    - crates/speck-vz/src/guest.rs
key-decisions:
  - "Use StackBlock (renamed from ConcreteBlock) for connectToPort completion handler"
  - "dup() the fd from VZVirtioSocketConnection before wrapping in VzSocket — connection owns the original fd"
  - "VmSocketDevice Send wrapper required: VZSocketDevice is not Send/Sync, similar to existing VmMachine pattern"
  - "30-second timeout on vsock connect (plan said 5s; 30s is safer for first-time connect)"
requirements-completed: []
duration: 38min
completed: 2026-06-26
---

# Phase 03 Plan 02: VmThread vsock wiring

**VZVirtioSocketDeviceConfiguration added to do_start, socket device extracted after VM start, do_vsock_connect handler with connectToPort + fd dup, Guest::vsock_connect public API**

## Performance

- **Duration:** 38 min
- **Started:** 2026-06-26
- **Completed:** 2026-06-26
- **Tasks:** 2
- **Files modified:** 2

## Accomplishments

- `do_start` now creates `VZVirtioSocketDeviceConfiguration` via `NSArray::from_slice` and sets it on the VM config via `setSocketDevices`
- After successful VM creation, `vm.socketDevices().firstObject()` extracts the runtime `VZSocketDevice` and stores it in `VmControl.socket_device`
- `VmCommand::VsockConnect { port, reply }` variant added for IPC
- `do_vsock_connect` runs on the serial dispatch queue: obtains the device, downcasts to `VZVirtioSocketDevice`, calls `connectToPort_completionHandler` with a `StackBlock`, `dup()`s the fd from the connection, and wraps it in `VzSocket`
- `Guest::vsock_connect(port)` delegates to `VmThread::vsock_connect(port)` for public API
- `VmSocketDevice` Send wrapper created to safely hold `Option<Retained<VZSocketDevice>>` in `Arc<Mutex<VmControl>>`

## Task Commits

1. **Combined: Tasks 1 & 2** — `2f0cd5e` (feat)
   - Both tasks were implemented together since they share the socket_device field lifecycle

## Files Created/Modified

- `crates/speck-vz/src/vm_thread.rs` — VmSocketDevice wrapper, VZVirtioSocketDeviceConfiguration in do_start, socket_device extraction, VsockConnect dispatch, do_vsock_connect handler, vsock_connect public method
- `crates/speck-vz/src/guest.rs` — Guest::vsock_connect(port) delegating to thread

## Decisions Made

- **VmSocketDevice Send wrapper**: `VZSocketDevice` is not `Send`/`Sync` (standard for ObjC framework classes). Following the existing `VmMachine` pattern, a `VmSocketDevice` newtype wraps `Option<Retained<VZSocketDevice>>` with `unsafe impl Send`, justified by the same serial-queue-only access invariant.
- **fd dup in completion handler**: `VZVirtioSocketConnection.fileDescriptor` returns an fd owned by the connection object. Without `dup()`, the fd would be double-closed when both `VzSocket::Drop` and the connection dealloc fire. A `libc::dup()` before `from_raw_fd` gives VzSocket its own fd.
- **30-second timeout**: The plan specified 5 seconds, but 30 seconds is safer for first-time connect scenarios (guest may still be initializing its vsock listener). The timeout is a `Duration` constant in `do_vsock_connect`.
- **StackBlock over ConcreteBlock**: block2 0.6 renamed `ConcreteBlock` to `StackBlock`. Using the current name avoids deprecation warnings.

## Deviations from Plan

### API Mismatches (Plan vs. Actual Binding)

**1. [Rule 2 - Missing Critical] VZSocketDevice not Send — required VmSocketDevice wrapper**
- **Found during:** Task 1
- **Issue:** The plan stored `Option<Retained<VZSocketDevice>>` directly in `VmControl` inside `Arc<Mutex<...>>`, but `VZSocketDevice` is not `Send`/`Sync`. This caused 6+ compilation errors on every `exec_sync` closure.
- **Fix:** Created `VmSocketDevice` wrapper with `unsafe impl Send`, following the exact pattern of the existing `VmMachine` struct.
- **Files modified:** `crates/speck-vz/src/vm_thread.rs`
- **Verification:** `cargo check -p speck-vz` passes
- **Committed in:** `2f0cd5e`

**2. [Rule 2 - Missing Critical] fd from connection needs dup() to avoid double-close**
- **Found during:** Task 2
- **Issue:** The plan's code passed the raw fd from `conn.fileDescriptor()` directly to `VzSocket::from_raw_fd()`. But `VZVirtioSocketConnection` owns this fd and will `close()` it on dealloc. If `VzSocket` also calls `close()` on `Drop`, we get a double-close (undefined behavior).
- **Fix:** Added `libc::dup(raw_fd)` to create an independent copy, then wrap the dup'd fd in `VzSocket`. The connection still owns and closes the original.
- **Files modified:** `crates/speck-vz/src/vm_thread.rs`
- **Verification:** Runtime safety fix (can't unit-test dups without a running VM)
- **Committed in:** `2f0cd5e`

**3. [Rule 3 - Blocking] setSocketDevices takes &NSArray<...> not Option<&...>**
- **Found during:** Task 1 compilation
- **Issue:** The plan's code used `setSocketDevices(Some(&socket_array))` but the generated binding takes `&NSArray<VZSocketDeviceConfiguration>` directly.
- **Fix:** Removed `Some()` wrapper.
- **Files modified:** `crates/speck-vz/src/vm_thread.rs`
- **Verification:** `cargo check -p speck-vz` passes
- **Committed in:** `2f0cd5e`

**4. [Rule 3 - Blocking] socketDevices() returns Retained<NSArray<...>> not Option<&...>**
- **Found during:** Task 1 compilation
- **Issue:** The plan expected `if let Some(devices) = vm.socketDevices()` but the method returns `Retained<NSArray<VZSocketDevice>>` directly (not wrapped in Option).
- **Fix:** Changed to `let devices = unsafe { vm.socketDevices() };` and call `firstObject()` directly.
- **Files modified:** `crates/speck-vz/src/vm_thread.rs`
- **Verification:** `cargo check -p speck-vz` passes
- **Committed in:** `2f0cd5e`

**5. [Rule 3 - Blocking] ConcreteBlock renamed to StackBlock**
- **Found during:** Task 2 compilation
- **Issue:** block2 0.6 deprecated `ConcreteBlock` in favor of `StackBlock`.
- **Fix:** Used `StackBlock::new(...)` and updated the import.
- **Files modified:** `crates/speck-vz/src/vm_thread.rs`
- **Verification:** `cargo check -p speck-vz` passes, no warnings
- **Committed in:** `2f0cd5e`

**6. [Rule 3 - Blocking] connectToPort block receives raw pointers, not Option<Retained<...>>**
- **Found during:** Task 2 implementation
- **Issue:** The plan wrote the block as `move |connection: Option<Retained<VZVirtioSocketConnection>>, error: Option<&NSError>|` but the generated binding passes `*mut VZVirtioSocketConnection` and `*mut NSError` (raw pointers).
- **Fix:** Block uses `*mut` pointers with null checks and `unsafe { &*ptr }` dereferences, consistent with the existing `startWithCompletionHandler` and `stopWithCompletionHandler` patterns.
- **Files modified:** `crates/speck-vz/src/vm_thread.rs`
- **Verification:** `cargo check -p speck-vz` passes
- **Committed in:** `2f0cd5e`

---

**Total deviations:** 6 auto-fixed (4 blocking, 2 missing critical)
**Impact on plan:** All fixes necessary for correctness and compilation. The `dup()` fix prevents a runtime double-close bug. No scope creep.

## Issues Encountered

- The plan described a `VmControl`/`VmState`/`build_config` API structure that didn't match the actual committed code. The actual code uses `do_start`/`do_stop` fns, `mpsc`/`oneshot` channels, and `Arc<Mutex<VmControl>>`. This structural mismatch required significant adjustments to the plan's code snippets, but all adjustments were straightforward applications of deviation rules.
- `cgrep` was attempted initially (not available) — reverted to standard `grep` for verification.

## Known Stubs

None. All code is wired end-to-end. The `VsockConnect` dispatch arm, `do_vsock_connect` handler, and `Guest::vsock_connect` method all have complete implementations.

## Threat Flags

None. All new surface (vsock connect path) is documented in the plan's threat model T-03-02-01 through T-03-02-03. No endpoints or trust boundaries were added beyond what was planned.

## Next Phase Readiness

- Plan 03-03 (guest side vminitd) — already completed ✅
- Plan 03-04 (integration test) — can now write a test that starts the VM, connects via vsock, sends data, and verifies the echo
- The 30-second timeout vs 5-second plan discrepancy is noted — if the guest echo server is fast, the timeout can be tightened in Plan 03-04

## Self-Check: PASSED

| Check | Status | Detail |
|-------|--------|--------|
| `vm_thread.rs` exists | ✅ | `crates/speck-vz/src/vm_thread.rs` |
| `guest.rs` exists | ✅ | `crates/speck-vz/src/guest.rs` |
| `03-02-SUMMARY.md` exists | ✅ | Output artifact |
| Commit `2f0cd5e` found | ✅ | `feat(03-02): wire vsock into VM lifecycle` |
| `cargo check -p speck-vz` | ✅ | Compiles clean, zero warnings |

---

*Phase: 03-vsock-echo*
*Completed: 2026-06-26*
