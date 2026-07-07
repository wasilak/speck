---
gsd_state_version: 1.0
milestone: v1.2
milestone_name: Hardened Runtime
status: executing
last_updated: "2026-07-07T17:10:00.000Z"
last_activity: 2026-07-07 -- Phase 16 Plan 01 completed (Restarting variant)
progress:
   total_phases: 4
    completed_phases: 1
    total_plans: 9
    completed_plans: 9
    percent: 25
verification: passed
review: issues_found (advisory — non-blocking)
---

# State — Milestone v1.2 Hardened Runtime

**Status:** Phase 16 — Plan 01 complete

## Project Reference

See: .planning/PROJECT.md (updated 2026-07-06)

**Core value:** A container runtime on Apple Silicon that never loses the network — micro-VMs inherit the host's routing/DNS live, surviving corporate VPNs and Cloudflare WARP where Docker Desktop fails.
**Current focus:** Phase 15 — stability-foundation (complete)

## Current Position

Phase: 16 (daemon-polish) — IN PROGRESS
Plan: 1 of TBD
Status: Plan 01 complete (VmState::Restarting variant added)
Last activity: 2026-07-07 -- Phase 16 Plan 01 completed (Restarting variant)

```
Progress: [████████████████████] 25% (1/4 phases)
```

## Milestone v1.2 Phase Overview

| Phase | Name | Requirements | Status |
|-------|------|--------------|--------|
| 15 | Stability Foundation | TEST-01..07 | Complete ✅ |
| 16 | Daemon Polish | DAEMON-01..04 | In Progress |
| 17 | testcontainers Conformance | CONF-01..07 | Not started |
| 18 | Developer ID Distribution | DIST-01..04 | Not started |

## Performance Metrics

| Metric | v1.1 actual | v1.2 target |
|--------|-------------|-------------|
| Phases | 8 | 4 |
| Plans | 26 | TBD |
| Timeline | 5 days | TBD |
| Phase 15 P02 | 4 min | 2 tasks | 4 files |
| Phase 15-stability-foundation P04 | 40 | - tasks | - files |
| Phase 15-stability-foundation P07 | 6min | 2 tasks | 3 files |
| Phase 15-stability-foundation P08 | 15min | 3 tasks | 4 files |

## Accumulated Context

### Key Decisions (v1.2)

- CONSOLE-01 already implemented in `vm_thread.rs` — TEST-04 is verification + ticket closure, not new work
- STATE-01/02 scope: moby already persists container/image state via BoltDB; SpeckDockerd only needs to persist its own network/volume name→ID metadata (JSON files, not SQLite)
- CONF-01 wires SpeckDockerd axum server as the intercepting Docker API layer (replaces raw vsock byte-bridge)
- `spk restart` is external process sequencing (stop + start) with `503 Retry-After`; in-process VM restart is out of scope for v1.2
- JSON with atomic rename (write-to-tmp + rename) preferred over SQLite for network/volume metadata — scope is narrow, no query capability needed

### Watch-out Items

- `notarytool submit --wait` exits 0 on rejection — CI must parse JSON `.status`, not `$?`
- `mockall` `#[automock]` must appear **before** `#[async_trait]`, not after
- Serial console reader thread must be started **before** `VZVirtualMachine.start()` — pipe buffer fills on boot burst
- `com.apple.security.virtualization` entitlement must be `<true/>` (boolean), not `<string>true</string>` — wrong type is silent SIGKILL after notarization
- `spk restart` must await stop reply channel before issuing start — races GCD completion handler otherwise

### Open Questions (from research)

1. App Store Connect API key for CI notarization vs Apple ID + app-specific password — relevant to Phase 18
2. Should `spk doctor` show last N lines of `console.log` on boot failure? (trivial; confirm scope before Phase 15 planning)
3. Should `Syscalls` trait in `vminitd.rs` be `pub(crate)` only or `pub` for separate test binary? — relevant to Phase 15
4. `PUT /containers/{id}/wait?condition=not-running` — in scope for CONF-07 or v1.3?

## Session Continuity

**To resume:** Run `/gsd-discuss-phase 16` or `/gsd-plan-phase 16` to plan Daemon Polish.

## Decisions

- [Phase ?]: Syscalls trait uses u64 for flags to allow cross-platform compilation
- [Phase ?]: mount flag constants (MS_BIND=4096, MS_RELATIME=2097152) defined locally to avoid macOS libc dependency
- [Phase ?]: mount_disks integration test excluded due to real filesystem deps; mount_early_filesystems validates mock pattern
- [Phase 15-08]: Added source-inspection safety guard tests per file (no_unsafe_env_mutation) to enforce no raw env mutation in test code
