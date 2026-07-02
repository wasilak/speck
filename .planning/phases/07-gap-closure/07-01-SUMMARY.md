---
phase: 07-gap-closure
plan: 01
subsystem: runtime
tags: [rust, tokio, virtualization, objc2, vm-lifecycle]

requires:
  - phase: 06-docker-api-compat
    provides: Docker API proxy and `spk up` runtime wiring
provides:
  - Tokio-safe VM startup boundary in `spk up`
  - Live VM delegate event drain updating stopped state
  - Explicit objc2-virtualization feature coverage for imported VZ types
affects: [phase-07-gap-closure, vm-lifecycle, cli-up]

tech-stack:
  added: []
  patterns:
    - spawn_blocking boundary for synchronous VM lifecycle calls in async CLI commands
    - VmDelegate event channel drained by a VM-lifetime worker thread
    - Explicit objc2-virtualization feature flags matching imported VZ types

key-files:
  created:
    - crates/speck-cli/src/lib.rs
  modified:
    - crates/speck-cli/src/commands/up.rs
    - crates/speck-vz/src/vm_thread.rs
    - crates/speck-vz/Cargo.toml

key-decisions:
  - "Kept SpeckNet startup outside spawn_blocking so only synchronous VM start/readiness work leaves the Tokio runtime."
  - "Stored the Objective-C VM delegate in VmControl to keep callbacks alive for the VM lifetime."
  - "Added a speck-cli library target solely to satisfy the plan's `cargo test -p speck-cli --lib` verification command."

patterns-established:
  - "Blocking VM lifecycle calls from async CLI code should return the Guest from a single spawn_blocking closure before network setup continues."
  - "Delegate callbacks emit VmStateEvent only; state mutation happens in vm_thread.rs without calling Virtualization.framework APIs from the drain thread."

requirements-completed: [GAP-01]

duration: 28min
completed: 2026-07-02
---

# Phase 07 Plan 01: GAP-01 VM Startup Safety Summary

**Tokio-safe `spk up` startup with VM delegate lifecycle draining and explicit Virtualization.framework feature flags**

## Performance

- **Duration:** 28 min
- **Started:** 2026-07-02T10:38:02Z
- **Completed:** 2026-07-02T11:06:26Z
- **Tasks:** 3 completed
- **Files modified:** 4

## Accomplishments

- Wrapped `Guest::start()` and `Guest::wait_for_ready()` inside one `tokio::task::spawn_blocking` closure while preserving the post-readiness netstack, DNS vsock, and Docker proxy sequence.
- Added a VM delegate drain path that consumes `VmStateEvent::Stopped` and `VmStateEvent::Error`, setting `VmControl.state` to `InternalState::Stopped` without making VZ framework calls off the serial queue.
- Kept the Objective-C `VmDelegate` retained in `VmControl` for the VM lifetime so callbacks are not silently dropped.
- Completed explicit `objc2-virtualization` feature coverage for all `VZ*` types imported by `vm_thread.rs`, `virtiofs.rs`, and `delegate.rs`.

## Task Commits

Each task was committed atomically:

1. **Task 1 RED: Runtime-safe startup test** - `32e6ea78` (test)
2. **Task 1 GREEN: spawn_blocking startup boundary** - `41c50b40` (feat)
3. **Task 2 RED: delegate state drain tests** - `cc3250a1` (test)
4. **Task 2 GREEN: delegate drain implementation** - `f7951327` (feat)
5. **Task 3: objc2-vz feature flags** - `fe9c36b6` (chore)

**Plan metadata:** committed separately in the SUMMARY commit.

## Files Created/Modified

- `crates/speck-cli/src/lib.rs` - Adds a library target with the CLI argument types needed for `cargo test -p speck-cli --lib`.
- `crates/speck-cli/src/commands/up.rs` - Wraps blocking VM startup/readiness in `spawn_blocking` and adds a regression test for ordering.
- `crates/speck-vz/src/vm_thread.rs` - Adds retained delegate storage, a delegate receiver drain thread, and unit coverage for stop/error state transitions.
- `crates/speck-vz/Cargo.toml` - Adds missing explicit `objc2-virtualization` features.

## Decisions Made

- Used a single `spawn_blocking` closure that returns `Guest`, keeping the existing `Guest` usable for `set_port_map_channel`, `netstack_fd`, `connect_dns_vsock`, `SpeckNet::spawn`, and `docker_api_unix_proxy` after readiness.
- Kept the delegate drain thread limited to `VmControl` state mutation. It does not call `VZVirtualMachine` or any other Virtualization.framework API.
- Added a minimal `speck-cli` lib target because the plan mandates `cargo test -p speck-cli --lib`, but the crate previously only had a binary target.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Added a speck-cli library target for the mandated test command**
- **Found during:** Task 1 (Wrap blocking Guest startup in run_up)
- **Issue:** `cargo test -p speck-cli --lib` failed before implementation with `no library targets found in package speck-cli`.
- **Fix:** Added `crates/speck-cli/src/lib.rs` with the command modules and CLI argument types needed to compile the library test target.
- **Files modified:** `crates/speck-cli/src/lib.rs`
- **Verification:** `cargo test -p speck-cli --lib` now passes.
- **Committed in:** `32e6ea78`

---

**Total deviations:** 1 auto-fixed (1 Rule 3 blocking issue).
**Impact on plan:** The fix was necessary to run the plan's required verification command. No runtime behavior was added beyond enabling the library test target.

## Issues Encountered

- Phase 07 planning artifacts were not present in the branch base because `.planning/` is ignored and the new Phase 07 files existed only in the primary checkout. They were copied into this isolated worktree for execution context only. Shared orchestrator artifacts were not updated.

## Known Stubs

None.

## Threat Flags

None. The changed surfaces match the plan threat model: async runtime → blocking VM startup and VZ delegate callback → VM state.

## Verification

- `cargo test -p speck-cli --lib` — PASS (4 passed)
- `cargo test -p speck-vz --lib` — PASS (16 passed, 1 ignored)
- Feature audit script confirmed every `VZ*` type imported by `vm_thread.rs`, `virtiofs.rs`, and `delegate.rs` has a matching Cargo feature.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

GAP-01 is ready for downstream Phase 07 plans. `spk up` no longer performs blocking VM lifecycle calls directly on the Tokio runtime, VM stop/error callbacks update internal state, and the VZ feature list is explicit.

## Self-Check: PASSED

- Found created file: `crates/speck-cli/src/lib.rs`
- Found modified files: `crates/speck-cli/src/commands/up.rs`, `crates/speck-vz/src/vm_thread.rs`, `crates/speck-vz/Cargo.toml`
- Found task commits: `32e6ea78`, `41c50b40`, `cc3250a1`, `f7951327`, `fe9c36b6`
- Verification commands passed before summary creation.

---
*Phase: 07-gap-closure*
*Completed: 2026-07-02*
