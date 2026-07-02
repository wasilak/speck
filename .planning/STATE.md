---
gsd_state_version: 1.0
milestone: v1.1
milestone_name: — Production Runtime
status: executing
last_updated: "2026-07-02T15:20:13.415Z"
last_activity: 2026-07-02 -- Phase 07 execution started
progress:
  total_phases: 8
  completed_phases: 1
  total_plans: 5
  completed_plans: 5
  percent: 13
---

# State — Milestone v1.1 Production Runtime

**Status:** Executing Phase 07

## Current Position

Phase: 07 (gap-closure) — EXECUTING
Plan: 1 of 5
Status: Executing Phase 07
Last activity: 2026-07-02 -- Phase 07 execution started

Progress: 0/8 phases complete [░░░░░░░░░░░░░░░░░░░░] 0%

## Milestone v1.1 Phase Overview

| Phase | Name | Requirements | Status |
|-------|------|--------------|--------|
| 07 | Gap Closure | GAP-01..04 | Not started |
| 08 | Config File & VM Resources | CFG-01..03, VMCFG-01..04 | Not started |
| 09 | Daemon Lifecycle | DAEMON-01..05 | Not started |
| 10 | Shell Integration | SHELL-01..03 | Not started |
| 11 | VPN-Proof DNS | DNS-01..06 | Not started |
| 12 | Corporate CA Injection | CERT-01..04 | Not started |
| 13 | Diagnostics | DOCTOR-01..03 | Not started |
| 14 | Homebrew Distribution | BREW-01..03 | Not started |

## Accumulated Context

### Decisions (v1.1)

- D-09: DNS — never bind host `:53`; VM-internal proxy routes to `SCDynamicStore` resolvers via vsock/high-port; WARP resolvers (`127.0.2.2/3`) are respected via split-DNS domain routing
- DNS Phase (11) is highest technical risk: SCDynamicStore must use `SCDynamicStoreSetDispatchQueue` — not a raw OS thread, not a tokio task. Wrong model = silent no-op with zero DNS change notifications.
- Daemon Phase (09) must use launchd LaunchAgent re-exec pattern — `fork()` after Apple framework init is UB (Mach ports not inheritable, libdispatch atfork poison). The `daemonize` crate is banned.
- CA injection (Phase 12) must happen before containerd/buildkitd start — containerd reads TLS config at startup only (containerd issue #3071). Post-startup injection silently fails.
- Homebrew (Phase 14) requires a signed `.pkg` artifact shipped as a Cask — Homebrew re-signs binaries, orphaning the notarization staple on a plain `.tar.gz`. `installer -pkg` does not re-sign.

### New Crates for v1.1

| Crate | Location | Purpose |
|-------|----------|---------|
| `tracing-appender` 0.2.5 | `speck-cli` | Rolling log file sink for daemon mode |
| `serde_yaml` 0.9 | `speck-cli` | Deserialize `$SPECK_HOME/config.yaml` |
| `system-configuration` 0.7.0 | `speck-net` | SCDynamicStore live DNS watcher |
| `core-foundation` 0.10.1 | `speck-net` | CFString/CFArray for SCDynamicStore dispatch queue |
| `pem` 3.0.6 | `speck-core` | Parse/split PEM blocks for CA injection |

### Watch Out For

- `system-configuration` + `core-foundation` version compatibility with `objc2-foundation` — run `cargo tree` before adding to `speck-net` to detect CFType conflicts
- Disk resize in VMCFG-03 requires guest-side `growpart + resize2fs` — resizing the `.img` only changes the block device size
- DOCKER_HOST conflict: `spk init` writing `DOCKER_HOST` globally redirects all Docker-aware tools; must be opt-in

### Todos

- [ ] Start Phase 07 planning: `/gsd-plan-phase 7`

### Blockers

None.

## Previous Milestone (v1.0) — Complete

All 32 plans across 6 phases complete. Phase 06.1 (5 integration bug fixes) in progress when v1.1 planning started — Phase 07 picks up its remaining 2 plans as the first order of business.

## Performance Metrics

| Phase | Plan | Duration | Notes |
|-------|------|----------|-------|
| Phase 06 P04 | 26 min | 2 tasks | 12 files |
| Phase 06 P05 | 17min | 2 tasks | 7 files |
| Phase 06.2 P06.2-07 | 5 min | 2 tasks | 2 files |
| Phase 06.2 P06.2-08 | resume close-out | 2 tasks | 12 files |
| Phase 06.2 P06.2-09 | 6 min | 2 tasks | 3 files |
