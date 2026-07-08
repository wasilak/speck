---
gsd_state_version: 1.0
milestone: v1.2
milestone_name: Hardened Runtime
status: completed
last_updated: "2026-07-08T14:13:29.231Z"
last_activity: 2026-07-08
progress:
  total_phases: 4
  completed_phases: 4
  total_plans: 26
  completed_plans: 26
  percent: 100
---

# State — Milestone v1.2 Hardened Runtime

**Status:** Milestone complete

## Project Reference

See: .planning/PROJECT.md (updated 2026-07-06)

**Core value:** A container runtime on Apple Silicon that never loses the network — micro-VMs inherit the host's routing/DNS live, surviving corporate VPNs and Cloudflare WARP where Docker Desktop fails.
**Current focus:** Phase 18 — ad-hoc-development-distribution

## Current Position

Phase: 18
Plan: 3 of 3 complete
Next: Milestone complete
Status: Phase verified — milestone complete
Last activity: 2026-07-08

```
Progress: [████████████████████████████████████████] 100% (4/4 phases)
```

## Milestone v1.2 Phase Overview

| Phase | Name | Requirements | Status |
|-------|------|--------------|--------|
| 15 | Stability Foundation | TEST-01..07 | Complete ✅ |
| 16 | Daemon Polish | DAEMON-01..04 | Complete ✅ |
| 17 | testcontainers Conformance | CONF-01..07 | Complete ✅ |
| 18 | Ad-hoc Development Distribution | DIST-01..04 | Complete ✅ |

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
| Phase 16-daemon-polish P02 | 12min | 2 tasks | 2 files |
| Phase 16-daemon-polish P03 | 12min | 2 tasks | 3 files |
| Phase 16-daemon-polish P05 | 8min | 3 tasks | 4 files |
| Phase 18-developer-id-distribution P03 | 3min | 2 tasks | 3 files |

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

**To resume:** Milestone v1.2 is complete. Start the next milestone or plan the future Developer ID/notarization lane when Apple Developer Program credentials are available.

## Decisions

- [Phase ?]: Syscalls trait uses u64 for flags to allow cross-platform compilation
- [Phase ?]: mount flag constants (MS_BIND=4096, MS_RELATIME=2097152) defined locally to avoid macOS libc dependency
- [Phase ?]: mount_disks integration test excluded due to real filesystem deps; mount_early_filesystems validates mock pattern
- [Phase 15-08]: Added source-inspection safety guard tests per file (no_unsafe_env_mutation) to enforce no raw env mutation in test code
- [Phase 16-03]: spk restart uses external process sequencing (subprocess stop + start) with exit code guards — prevents double-VM on failed stop; PREPARE_RESTART signal sent over control socket for daemon 503 middleware
- [Phase 16-05]: Used std::sync::RwLock instead of parking_lot for middleware VmState (parking_lot not in any crate's dep tree); did not modify guest::docker_api_unix_proxy signature — it's a raw byte proxy, middleware wiring belongs in SpeckDockerd axum layer
- [Phase 18-03]: Active release distribution remains ad-hoc development-only; Developer ID, notarization, stapling, and official Homebrew Cask publication are deferred.
- [Phase 18-03]: GitHub Releases upload `cargo xtask dist` non-notarized development artifacts instead of mutating the legacy Formula.
- [Phase 18-03]: Legacy Formula is preserved as a clearly development-only install route until an equivalent official path exists.
