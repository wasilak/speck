---
gsd_state_version: 1.0
milestone: v1.4
milestone_name: Daily Driver
status: in_progress
stopped_at: Phase 0 hygiene done; Phase 26 not yet planned
last_updated: "2026-10-11T12:00:00.000Z"
last_activity: 2026-10-11 -- Closed v1.3, opened v1.4 Daily Driver, egress baseline captured
progress:
  total_phases: 5
  completed_phases: 0
  total_plans: 0
  completed_plans: 0
  percent: 0
---

# State — Milestone v1.4 Daily Driver

## Project Reference

See: .planning/PROJECT.md (updated 2026-10-11)

**Core value:** A container runtime on Apple Silicon that never loses the network — micro-VMs inherit the host's routing/DNS live, surviving corporate VPNs and Cloudflare WARP where Docker Desktop fails.
**Current focus:** Phase 26 (General Egress) — needs plan-phase

## Current Position

Phase: 26 (General Egress) — not started
Plan: none yet
Status: Phase 0 (hygiene + baseline) complete
Last activity: 2026-10-11 -- v1.3 closed; HEAD build installed; conformance 26/26; egress baseline captured

Progress: [░░░░░░░░░░] 0%

## Phase 0 Results (2026-10-11)

- Installed `~/bin/spk` was a stale 2026-07-16 build (missing `c725d50` WouldBlock fix and Phase 25) while guest initrd was from HEAD. Rebuilt release from `be27106`, signed (`cargo xtask sign release`), installed, restarted. **Gotcha:** `cp` over the signed binary in place → SIGKILL (rc=137) on launch; `rm` first, then copy.
- `scripts/conformance-smoke.sh`: 26/26 on the HEAD build.
- Egress baseline (`scripts/egress-smoke.sh`, new): 5 passed / 9 failed. PASS: TCP 443/80, DNS A, https wget, `apk add`. FAIL: TCP 22/5432/9200/8443/27017, UDP 123, DNS TXT/SRV/MX. This is the Phase 26/28 red baseline.
- M0 corporate-Mac baseline: **pending (owner, manual)** — decides whether TLS-04 (explicit proxy/PAC) is needed.

## Milestone v1.4 Phase Overview

| Phase | Name | Requirements | Status |
|-------|------|--------------|--------|
| 26 | General Egress | EGRESS-01..06, OPS-04 | Not started |
| 27 | Corporate TLS & Proxy | TLS-01..04 | Not started |
| 28 | Resilience | RES-01..04 | Not started |
| 29 | Operational Polish | OPS-01..03 | Not started |
| 30 | Dogfood Gate | DOG-01 | Not started |

## 2026-10-11 Review Findings (source for v1.4)

- Egress only TCP 443/80 — `crates/speck-net/src/lib.rs:108-115`; no UDP/ICMP/IPv6
- smoltcp sockets never removed from SocketSet (no `sockets.remove` in `crates/speck-net/src`)
- CA injection: `vminitd.rs:887-918` appends to `ca-certificates.crt`, then `update-ca-certificates --fresh` regenerates it — appended CAs are lost; CA never reaches containers/BuildKit
- `run_up` waits only for SIGINT/SIGTERM (`up.rs:~880`); VM death and netstack exit (`up.rs:783`, 85 log hits, one unprompted on 2026-08-18) are only logged
- Reorigin: no connect timeout, `close()` instead of RST (`reorigin.rs:180-183,257`), 256-bridge cap, 5 ms busy poll
- DNS: A/AAAA only, single-threaded (`dns.rs:80`), no TCP fallback
- `.expect("lock poisoned")` across `speck-dockerd/src/proxy.rs` and `restart_503.rs:59`; unbounded thread-per-connection forwards; publish only on 127.0.0.1
- Guest: dockerd respawn without backoff; `sysctl_write not yet implemented` → sysctl tuning is a silent no-op
- Distribution gap: public `initrd-1.3.0` (2026-07-09) predates the transparent proxy and Phase 25; the dev Mac runs a hand-repacked initrd → OPS-04, pulled into Phase 26
- ICMP echo is faked by the netstack: pings to any IP (even 192.0.2.1 / 10.255.255.1) answer in ~0.2 ms → EGRESS-06
- `SO_SNDBUF/SO_RCVBUF` EINVAL at boot is a harmless race on the READY probe (port 9000), not on data fds

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

- Corporate-network behaviour can only be verified manually on the owner's work Mac (dev Mac has no WARP/proxy). M0 checklist must run before Phase 27 planning.
- 2026-10-11: GSD abandoned for this project (token cost vs value). Work is tracked in Task Master only; `.planning/` is a read-only archive + decision log. See CLAUDE.md "Workflow & Tooling".

## Deferred Items

Carried forward at v1.2 close (2026-07-09) and requirements definition (2026-07-12):

| Category | Item | Status |
|----------|------|--------|
| deferred | Developer ID signing + notarization + Homebrew Cask | Requires Apple Developer Program |
| promoted | Broader guest-outbound port coverage beyond 443/80 in reorigin | → v1.4 EGRESS-01 |
| deferred | K3s/CRI integration prototype | Future |
| promoted | Listener buffer memory tuning (12 × 2MB smoltcp listeners) | → v1.4 EGRESS-02 |

### Roadmap Evolution

- Phase 25 added (2026-07-17): Correct Guest Boot — real switch_root (MS_MOVE-based mount namespace relocation) matching standard Linux/Colima boot semantics, replacing the chroot-only shortcut in vminitd (`crates/speck-guest/src/mount.rs::switch_root_to_rootfs`). Root cause and requirement surfaced during debug session `.planning/debug/resolved/buildx-http2-frame-too-large.md`. Note: milestone v1.3 was left formally un-transitioned (SUMMARY.md docs for phases 22/24 were never backfilled, PROJECT.md still shows v1.3 as "Current Milestone") — the underlying work is verified complete (26/26 conformance, live testing), this is a documentation-process gap, not open engineering work. Phase 25 proceeds without a full v1.3 close-out per explicit user decision (2026-07-17).

## Session Continuity

Last session: 2026-10-11
Stopped at: Phase 0 complete; v1.4 roadmap written
Resume file: None

**To resume:** this file is no longer updated — use `task-master next`. Phase 26 (General Egress) is next; the owner runs the M0 checklist (`docs/manual-testing.md`) on the corporate Mac.
