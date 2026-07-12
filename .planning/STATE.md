---
gsd_state_version: 1.0
milestone: v1.2
milestone_name: Hardened Runtime
status: completed
last_updated: "2026-07-09T13:02:44.171Z"
last_activity: 2026-07-09 — Milestone v1.2 completed and archived
progress:
  total_phases: 7
  completed_phases: 7
  total_plans: 32
  completed_plans: 32
  percent: 100
---

# State — Milestone v1.2 Hardened Runtime

**Status:** v1.2 milestone complete

## Deferred Items

Items acknowledged and deferred at milestone close on 2026-07-09:

| Category | Item | Status |
|----------|------|--------|
| verification_gap | Phase 17: follow=true log live streaming (CONF-05) | gaps_found (addressed in Phase 17.1/19) |
| verification_gap | Phase 17: HostIp default to "0.0.0.0" (CONF-06) | gaps_found (addressed in Phase 19) |
| verification_gap | Phase 17: Network connect/disconnect existence validation | gaps_found (addressed in Phase 17.1) |
| verification_gap | Phase 17.1: Follow-stream verification gap | gaps_found (addressed in Phase 19) |

## Project Reference

See: .planning/PROJECT.md (updated 2026-07-06)

**Core value:** A container runtime on Apple Silicon that never loses the network — micro-VMs inherit the host's routing/DNS live, surviving corporate VPNs and Cloudflare WARP where Docker Desktop fails.
**Current focus:** Phase 19 — close-testcontainers-conformance-gaps-inserted

## Current Position

Phase: Milestone v1.2 complete
Plan: —
Status: Awaiting next milestone
Last activity: 2026-07-09 — Milestone v1.2 completed and archived

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
| Phase 18.1-close-gap-daemon-stop-restart-control-socket-reliability P01 | 14min | 3 tasks | 1 files |
| Phase 17.1-close-testcontainers-conformance-gaps-follow-stream-hostip-d P01 | 1min | 3 tasks | 4 files |
| Phase 17.1-close-testcontainers-conformance-gaps-follow-stream-hostip-d P03 | 94min | 3 tasks | 5 files |
| Phase 17.1-close-testcontainers-conformance-gaps-follow-stream-hostip-d P04 | 20min | 2 tasks | 5 files |

## Accumulated Context

### Roadmap Evolution

- Phase 18.1 inserted after Phase 18: Close gap: daemon stop/restart control socket reliability (URGENT)
- Phase 17.1 inserted after Phase 17: Close testcontainers conformance gaps (follow-stream, HostIp default, network validation) (URGENT)

### Key Decisions (v1.2)

- [Phase 17.1-01]: Skipped cherry-pick of orphaned commit 1b8416ba — its relay_created guard was already applied as b43fadcf on main ancestry, and its networks.rs deletions would undo network 404 validation (CONF-03)
- [Phase 17.1-03]: Use the dockerd-managed containerd socket path under `/rootfs/run/docker/containerd/containerd.sock` when the standalone `/rootfs/run/containerd/containerd.sock` path is absent.
- [Phase 17.1-03]: Keep resize-tool provisioning working without Docker Desktop by falling back to `skopeo` + Alpine APK extraction.
- [Phase 17.1-04]: Empty-string HostIp normalizes via `filter(|ip| !ip.is_empty()).unwrap_or_else(...)` — handles both null and `""` uniformly. RetainedLogBuffer uses absolute offset semantics. Full live conformance suite gate deferred (requires interactive `spk up --foreground`).

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

## Quick Tasks Completed

| ID | Description | Date | Commits |
|----|-------------|------|---------|
| 260709-u4q | Fix pull/push throughput collapse: 4MB net socketpair buffers, unclamp advertised TCP window, 1MB smoltcp buffers | 2026-07-10 | 4ac3a9e0, 8c044e46 |
| 260710-cpz | Fix control-socket probe deadlock (down/status/doctor); 1MiB port-publish buffers; egress drop logging; entitlement warning | 2026-07-10 | f7506798, ab256842, 8fc5e1e5 |
| 260710-qzm | 4MB buffers on vsock connection fds (fixes host→guest direction) | 2026-07-10 | c0d60885 |
| 260711-d5v | Decouple vsock bridge reads from downstream writes — fixes VZ 8KB guest→host truncation that broke docker ps and all gRPC responses >8KB | 2026-07-11 | a2e21312, facaf67d |

## Session Continuity

**To resume:** Milestone v1.2 is complete. All Phase 17.1 gap-closure plans executed. Ready for milestone wrap-up audit or the next milestone / v1.3 planning.

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
- [Phase ?]: Block-scoped RwLock write guard in PREPARE_RESTART to avoid non-Send guard across .await
- [Phase ?]: Source-inspection tests locate error handlers via unique log message strings rather than Err(e) position

## Operator Next Steps

- Start the next milestone with /gsd-new-milestone
- **v1.3 direction is LOCKED (D-21, 2026-07-11):** restore transparent byte-proxy to guest dockerd; SpeckDockerd shrinks to allowlisted middleware (binds/ports/HostIp/503). See CLAUDE.md "Architecture Invariants". Definition of done: `scripts/conformance-smoke.sh` passes 18/18 (baseline 2026-07-11: 8/18).
