---
gsd_state_version: 1.0
milestone: v1.1
milestone_name: — Production Runtime
status: executing
last_updated: "2026-07-05T10:25:38.906Z"
last_activity: 2026-07-04 -- Phase 12 execution started
progress:
  total_phases: 8
  completed_phases: 5
  total_plans: 21
  completed_plans: 20
  percent: 63
---

# State — Milestone v1.1 Production Runtime

**Status:** Ready to execute

## Current Position

Phase: 12 (corporate-ca) — EXECUTING
Plan: 3 of 3
Status: Ready to execute
Last activity: 2026-07-04 -- Phase 12 execution started

Progress: 3/8 phases complete [████████░░░░░░░░░░░░░] 38%

## Milestone v1.1 Phase Overview

| Phase | Name | Requirements | Status |
|-------|------|--------------|--------|
| 07 | Gap Closure | GAP-01..04 | Complete |
| 08 | Config File & VM Resources | CFG-01..03, VMCFG-01..04 | Complete |
| 09 | Daemon Lifecycle | DAEMON-01..05 | Complete |
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
- Phase 08 Plan 01: Keep `SPECK_VM_*` out of clap env annotations so env vars can override explicit CLI flags.
- Phase 08 Plan 01: Keep YAML parsing isolated in `speck-cli::config`; `speck-vz` remains format-agnostic.
- Phase 08 Plan 01: Validate `SPECK_LOG_LEVEL` into `EffectiveConfig.log_level` so Plan 08-02 can initialize tracing from the resolved value.
- Phase 08 Plan 02: `spk up` resolves config before tracing and passes `EffectiveConfig` into `run_up`; `run_up` does not resolve config internally.
- Phase 08 Plan 02: Host data disk sizing is grow-only via `File::set_len`; shrink returns `disk shrink not supported:` without truncation.
- Phase 08 Plan 02: CPU/memory changes with active runtime holders fail fast using `$SPECK_HOME/run/vm-config.json` old/new values.

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
- Phase 09 can use `$SPECK_HOME/run/vm-config.json` as an initial resource-state source until the daemon control socket exists.

### Todos

- [ ] Start Phase 10 planning: `/gsd-plan-phase 10`

### Blockers

None.

## Performance Metrics

| Phase | Plan | Duration | Notes |
|-------|------|----------|-------|
| Phase 06 P04 | 26 min | 2 tasks | 12 files |
| Phase 06 P05 | 17min | 2 tasks | 7 files |
| Phase 06.2 P06.2-07 | 5 min | 2 tasks | 2 files |
| Phase 06.2 P06.2-08 | resume close-out | 2 tasks | 12 files |
| Phase 06.2 P06.2-09 | 6 min | 2 tasks | 3 files |
| Phase 08 P01 | 18min | 3 tasks | 4 files |
| Phase 08 P03 | 24min | 3 tasks | 3 files |
| Phase 08 P02 | 11min | 3 tasks | 5 files |
| Phase 10 P01 | 8min | 2 tasks | 6 files |
| Phase 10 P02 | 15min | 2 tasks | 5 files |
| Phase 12 P01 | 3min | 3 tasks | 3 files |
| Phase 12 P02 | 14min | - tasks | - files |

## Decisions

- Phase 08 Plan 02: Initialize `spk up` tracing from `EffectiveConfig.log_level` after config resolution and before startup warnings/run_up dispatch.
- Phase 08 Plan 02: Use grow-only `File::set_len` data disk reconciliation and reject shrink with `disk shrink not supported:`.
- Phase 08 Plan 02: Persist effective VM resources in `$SPECK_HOME/run/vm-config.json` and compare CPU/memory when runtime holders exist.
- Phase 09 Plan 02: `spk up` defaults to launchd re-exec; `--foreground` bypasses daemonization and `SPECK_DAEMONIZED=1` selects file logging plus prevents re-exec loops.
- Phase 09 Plan 02: daemon liveness is exposed only through `$SPECK_HOME/run/control.sock` responding with `PONG`, not through a PID file or command channel.
- [Phase ?]: Phase 10 Plan 01: EnvShell is a two-variant enum (Posix/Fish) with #[derive(Clone, Copy)] so callers can pass it to render_env and render_speck_home without cloning — Idiomatic zero-cost fix for a fieldless enum
- [Phase ?]: Phase 10 Plan 02: Sentinel replace-or-append (BEGIN/END marker) for bash/zsh dotfile writes; fish uses dedicated conf.d file overwrite; --set-docker-host is explicit opt-in consent for persistence
- [Phase ?]: Structural PEM validation instead of pem crate
- [Phase ?]: SHA256 content hash dedup for CA cert files
- [Phase ?]: CA_CERTS_TAG = speck-ca-certs
