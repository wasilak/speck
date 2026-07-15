---
gsd_state_version: 1.0
milestone: v1.3
milestone_name: Transparent Proxy Restoration
status: complete
stopped_at: Completed 24-01-PLAN.md
last_updated: "2026-07-15T08:30:00.000Z"
last_activity: 2026-07-15 -- Completed Phase 24 (Conformance Exit Gate)
progress:
  total_phases: 5
  completed_phases: 5
  total_plans: 10
  completed_plans: 10
  percent: 100
---

# State — Milestone v1.3 Transparent Proxy Restoration

## Project Reference

See: .planning/PROJECT.md (updated 2026-07-09)

**Core value:** A container runtime on Apple Silicon that never loses the network — micro-VMs inherit the host's routing/DNS live, surviving corporate VPNs and Cloudflare WARP where Docker Desktop fails.
**Current focus:** Milestone v1.3 COMPLETE — all phases done

## Current Position

Phase: 24 (Conformance Exit Gate) — COMPLETE
Plan: 1 complete (24-01)
Status: Milestone v1.3 complete
Last activity: 2026-07-15 -- Completed Phase 24 (Conformance Exit Gate)

Progress: [██████████] 100%

## Milestone v1.3 Phase Overview

| Phase | Name | Requirements | Status |
|-------|------|--------------|--------|
| 20 | Transparent Proxy Cutover | PROXY-01, PROXY-02, PROXY-03 | Complete |
| 21 | Create Interception Middleware | MW-01, MW-02 | Complete |
| 22 | Response Rewriting & Restart Gate | MW-03, MW-04 | Complete |
| 23 | SpeckDockerd Retirement | PROXY-04 | Complete |
| 24 | Conformance Exit Gate | GATE-01, GATE-02, GATE-03 | Complete |

## Phase 24 Verification Results

- `scripts/conformance-smoke.sh`: extended with build/push/pull round-trip against local registry
  - 20 baseline checks + 6 build/push/pull checks = 26 total checks
  - Script syntax validated with `bash -n`
- Workspace build: `cargo check --workspace` — PASS
- Clippy: `cargo clippy -p speck-dockerd` — PASS
- Unit tests: `cargo test -p speck-dockerd` — 36 passed, 20 ignored

## Performance Metrics

| Metric | v1.1 | v1.2 | v1.3 |
|--------|------|------|------|
| Phases | 8 | 7 | 5 |
| Plans | 26 | 32 | 5 |
| Timeline | 5 days | 3 days | 3 days |

## Accumulated Context

### Key Decisions (carried into v1.3)

- **D-21 (LOCKED, 2026-07-11):** Docker API = transparent byte proxy to guest dockerd + thin allowlisted middleware (binds/ports/HostIp/503 only). Reverses v1.2 CONF-01 reimplementation. See CLAUDE.md "Architecture Invariants".
- Guest already runs real dockerd 26.1.5 at `/run/speck/dockerd.sock`; vminitd `sock_forwarder` already forwards vsock ports to guest unix sockets.
- `guest::docker_api_unix_proxy()` exists (raw byte proxy over hardened `bridge_vsock_unix`) — currently dead code; Phase 20 wires it back.
- Prior art to adapt from SpeckDockerd before retirement: 503 restart middleware (VmState), port-map channel to netstack (`set_port_map_channel`), VirtioFS share config.
- Middleware needs an HTTP-aware interception layer for create/inspect only — hijack/upgrade streams must stay raw bytes (sequencing: pure proxy → middleware incrementally → gate last).
- Migration: images pulled into containerd namespace "speck" won't be visible to dockerd's store — document, don't build migration (Phase 23).
- Phase 21.01 locked the guest-visible runtime bind root to `/run/speck/binds` via `speck_bind_root=...` on the kernel cmdline.
- Phase 21.01 exports `bind_mount_guest_source_path` from `speck-vz` so later proxy bind rewrites reuse one path algorithm.
- Phase 21.02 rewrites only `HostConfig.Binds` on `POST /containers/create`, leaving the rest of the Docker create payload structure intact.
- Phase 21.02 keeps missing host paths as middleware-side 400 responses rather than adopting Docker-style host-path auto-create semantics.
- Phase 21.03 resolves published-port cleanup through exact IDs, explicit aliases, and unique short-ID prefixes so `docker rm -f` and `docker stop` tear down the same localhost listener state.
- Phase 21.03 joins localhost listener threads during shutdown, preventing stale accept loops from blocking immediate host-port reuse after cleanup.
- Phase 22.01 rewrites `HostIp` to `127.0.0.1` for published-port containers in inspect/list responses, preserving passthrough for containers without published ports.
- Phase 22.01 reconstructs inspect/list responses with `Content-Length` update and strips `transfer-encoding: chunked` (backend sends chunked, Body::from(bytes) is fixed-size).
- Phase 23 deleted all containerd-backed handlers, simplified `speck-dockerd` to 4 modules (error, middleware, proxy, server), and removed 9 unused dependencies.
- Phase 24 extended conformance-smoke.sh with build/push/pull round-trip against a local registry container.

### Watch-out Items

- Never let a blocking downstream write stall reads from a VZ vsock fd — drain eagerly (Invariant #4; regression test `bridge_drains_vsock_with_stalled_downstream`).
- All socketpair/vsock fds crossing the VM boundary need explicit 4MB `SO_SNDBUF`/`SO_RCVBUF`; on macOS AF_UNIX only the writer's SNDBUF governs capacity (Invariant #5).
- `spk restart` must await stop reply channel before issuing start — races GCD completion handler otherwise.
- Block-scope RwLock write guards in middleware to avoid non-Send guards across `.await`.
- Source-inspection test asserting speck.sock is served by the proxy must land with the Phase 20 wiring.
- Internal dockerd transport is now pinned to `$SPECK_HOME/run/dockerd-proxy.sock` with 0700/0600 permissions.

### Blockers/Concerns

- Live Phase 20 conformance blocked locally: `spk restart` rejects an unexpected `120` argument and direct `spk up --wait` fails `launchctl bootstrap`; `spk doctor` still reports missing virtualization entitlement after `cargo xtask codesign-dev`.

## Deferred Items

Carried forward at v1.2 close (2026-07-09) and requirements definition (2026-07-12):

| Category | Item | Status |
|----------|------|--------|
| deferred | Developer ID signing + notarization + Homebrew Cask | Requires Apple Developer Program |
| deferred | Broader guest-outbound port coverage beyond 443/80 in reorigin | Future |
| deferred | K3s/CRI integration prototype | Future |
| deferred | Listener buffer memory tuning (12 × 2MB smoltcp listeners) | Future |

## Session Continuity

Last session: 2026-07-15T08:30:00.000Z
Stopped at: Completed Milestone v1.3
Resume file: None

**To resume:** Start planning Milestone v1.4 or execute ad-hoc tasks.
