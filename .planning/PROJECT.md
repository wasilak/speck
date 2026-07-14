# Speck (`spk`)

Ultra-fast, minimalist container runtime for Apple Silicon macOS. Runs containers in microscopic Linux micro-VMs via Apple's native `Virtualization.framework` — Docker-compatible API socket, millisecond cold starts, and network that survives corporate VPNs and Cloudflare WARP.

**Core value:** A container runtime on Apple Silicon that never loses the network — micro-VMs inherit the host's routing/DNS live, surviving corporate VPNs and Cloudflare WARP where Docker Desktop fails.

## Current State

**Shipped v1.2 Hardened Runtime** (2026-07-07 → 2026-07-09): 7 phases, 32 plans, 55 tasks, ~50 commits.

v1.2 delivered hardened test coverage for the core DNS/networking stack (DNS proxy unit tests, vminitd mount simulation, unsafe env mutation sweep), Docker API conformance (9/9 tests passing with live container lifecycle: pull → create → start → exit), daemon lifecycle polish (reliable stop/restart, port/exec e2e), ad-hoc signed development distribution artifacts, and a FIFO-backed container stdout/stderr log relay with live follow-mode streaming.

**Known deferred:** Developer ID signing + notarization + Cask (requires Apple Developer Program). Verification gaps from Phase 17 acknowledged as resolved by subsequent gap-closure phases.

## Current Milestone: v1.3 Transparent Proxy Restoration

**Goal:** Implement locked decision D-21 — `speck.sock` becomes a transparent byte proxy to the real dockerd running inside the guest; SpeckDockerd shrinks from a Docker API reimplementation to a thin allowlisted middleware.

**Target features:**
- Docker API served by transparent proxy to guest dockerd (`/run/speck/dockerd.sock`) over the hardened vsock bridge
- Middleware allowlist only: container-create bind translation (VirtioFS), port-publish tracking for the host netstack, HostIp rewriting, 503 restart gate
- Hijacked/upgraded streams (attach/exec) pass through byte-for-byte
- Exit gate: `scripts/conformance-smoke.sh` 18/18 (baseline 2026-07-11: 8/18) + intact `spk down/up` launchd cycle

**Context (2026-07-09 → 2026-07-11 debugging round):** three systemic transport bugs fixed with regression tests — netstack socketpair buffers (1.7KB/s pulls), control-socket probe deadlock (down/status/doctor hangs), VZ vsock 8KB data loss (docker ps h2 errors). The remaining conformance failures are all consequences of the CONF-01 reimplementation drift that D-21 reverses.

**Out of scope for v1.3:** Developer ID distribution, K3s/CRI, GUI.

## Constraints

- **Platform:** macOS on Apple Silicon (`aarch64-apple-darwin`) only
- **Tech stack — core:** Rust (memory safety, no GC, top performance)
- **Tech stack — macOS bridge:** `objc2` + `objc2-virtualization` for Virtualization.framework
- **Performance:** micro-VM cold start in milliseconds
- **Architecture:** Core must be a library with zero presentation coupling
- **Licensing:** AGPLv3 + CLA

## Goals

1. Boot a minimal Linux kernel + initrd micro-VM in milliseconds
2. Run containerd + BuildKit inside the guest
3. Expose Docker-compatible API socket
4. Host-inheriting networking that works under corporate VPNs/WARP
5. CLI (`spk up/down/ps/build/dashboard`) + future SwiftUI GUI

## Non-Goals

- x86_64 / Rosetta support (Apple-Silicon-only)
- QEMU or any emulator
- Full desktop VM (no GUI in the guest)
- Windows guest support

## Requirements

### Validated

- ✓ Boot Linux micro-VM via `VZLinuxBootLoader` (direct kernel, no QEMU) — v1.0
- ✓ Host↔guest vsock communication (`VZVirtioSocketDevice`) — v1.0
- ✓ Docker-compatible API socket (`axum` + `hyper_util` Unix socket) — v1.0
- ✓ Container lifecycle: run/stop/rm/ps/exec/logs/attach — v1.0
- ✓ Image pull/push via containerd ImageService over vsock — v1.0
- ✓ `spk build` via BuildKit over vsock — v1.0
- ✓ VirtioFS volume mounts (`-v host:guest`) — v1.0
- ✓ Port publishing (`-p host:container`) via SmoltCP TCP re-origination — v1.0
- ✓ `ratatui` TUI dashboard (`spk dashboard`) — v1.0
- ✓ Ad-hoc codesign with `com.apple.security.virtualization` entitlement — v1.0
- ✓ Homebrew Formula (ad-hoc path) — v1.0
- ✓ GAP-01: `spawn_blocking` fix + VM delegate drain thread + objc2-vz feature flags — v1.1
- ✓ GAP-02/03/04: SpeckNet wired in run_up, DockerClient Unix socket, port map activation — v1.1
- ✓ DAEMON-01/02/03/05: `spk up` daemonizes via launchd LaunchAgent re-exec; `--foreground` for CI; `spk down`; liveness via control socket — v1.1
- ✓ SHELL-01/02/03: `spk env`, idempotent `spk init` for bash/zsh/fish, DOCKER_HOST conflict detection — v1.1
- ✓ CFG-01/02/03 + VMCFG-01/02/03/04: `$SPECK_HOME/config.yaml` with env > CLI > file > default; vCPU/RAM/disk validated — v1.1
- ✓ DNS-01–06: VM-internal DNS proxy via vsock/SCDynamicStore; split-DNS; live reload; NXDOMAIN→SERVFAIL — v1.1
- ✓ CERT-01–04: CA cert injection into guest trust bundle + containerd hosts.toml; fail-fast PEM validation — v1.1
- ✓ DOCTOR-01–03: `spk doctor` 8-check health suite; `spk doctor dns <hostname>` trace — v1.1
- ✓ BREW-01–03: Homebrew Formula (ad-hoc signed tarball) installs working `spk` with entitlement preserved — v1.1
- ✓ DNS-TEST-01: Unit tests for DNS proxy — v1.2
- ✓ VMINIT-TEST-01: Unit tests for vminitd mount/sysctl via Syscalls trait — v1.2
- ✓ CONSOLE-01: Wire serial console capture to `$SPECK_HOME/console.log` — v1.2
- ✓ VERSION-01: Guest version check at startup — v1.2
- ✓ UNSAFE-01: Replace unsafe `set_var`/`remove_var` with `temp-env` — v1.2
- ✓ STATE-01/02: Persist dockerd container/exec/network/volume state to disk — v1.2
- ✓ CONFORM-01: Docker API conformance tests (bollard-based) — v1.2
- ✓ DAEMON-RESTART: First-class `spk restart` subcommand — v1.2
- ✓ DAEMON-DOWN: `spk down` reliable outside launchd — v1.2
- ✓ PORT-E2E: Port publishing verified end-to-end — v1.2
- ✓ EXEC-E2E: `spk exec` verified end-to-end — v1.2

### Active

*(None — next milestone not yet defined.)*

### Out of Scope

- **Intel / x86_64 support** — Apple Silicon only, zero legacy debt
- **Host `:53` DNS binding** — architectural constraint D-09, never
- **double-fork / `daemonize` crate** — UB after Apple framework init, never
- **`VZNATNetworkDeviceAttachment`** — breaks WARP, decision D-02 is permanent
- **`com.apple.vm.networking` bridged networking** — Apple-gated entitlement, not on the critical path
- **Per-container CA injection post-start** — breaks image integrity
- **Synthetic subnets** — NET-06 constraint, never
- **QEMU / emulation** — defeats millisecond-start brand promise

## Context

**Shipped v1.1** (2026-07-01 → 2026-07-06): 8 phases, 26 plans, ~50 commits, 54 files changed, 7,736 insertions.
**Shipped v1.2** (2026-07-07 → 2026-07-09): 7 phases, 32 plans, 55 tasks, ~50 commits.
**Codebase:** ~17,000 lines of Rust across `speck-vz`, `speck-net`, `speck-cli`, `speck-core`, `speck-guest`.
**State:** `spk up` starts a background daemon; containers run end-to-end with Docker-compatible API; DNS survives VPN toggles; 9/9 conformance tests pass; daemon start/stop/restart works; development distribution via ad-hoc signed artifacts.
**Known limitations:** Developer ID signing + notarization + Homebrew Cask not yet delivered (requires Apple Developer Program). `spk restart` is stop+start subprocess, not in-process VM restart.
**Next:** Planning v1.3.

## Decisions

| # | Decision | Outcome | Milestone |
|---|----------|---------|-----------|
| D-01 | `objc2-virtualization` 0.3.x over `swift-bridge` or `cxx` | ✓ Good — zero Swift toolchain overhead | v1.0 |
| D-02 | `VZFileHandleNetworkDeviceAttachment` + user-space smoltcp over `VZNATNetworkDeviceAttachment` | ✓ Good — host routing inherited live | v1.0 |
| D-03 | Kata prebuilt arm64 kernel (not custom-built) | ✓ Good — fast, container-tuned, virtio built-in | v1.0 |
| D-04 | Core is a library (`speck-vz`); CLI is a separate consumer | ✓ Good — clean separation | v1.0 |
| D-05 | `aarch64-apple-darwin` only; no cross-compile | ✓ Good — zero legacy debt | v1.0 |
| D-06 | No `println!` in core libraries — use `tracing` events | ✓ Good — clean presentation boundary | v1.0 |
| D-07 | Ad-hoc signing for dev; Developer ID + notarization deferred | ✓ Good — unblocked distribution for dev use | v1.1 |
| D-08 | Kernel/data at `$SPECK_HOME` (default `~/.local/share/speck`), no sudo | ✓ Good — no privilege escalation | v1.0 |
| D-09 | DNS: never bind host `:53`; VM-internal proxy via vsock → SCDynamicStore | ✓ Good — core differentiator preserved | v1.1 |
| D-10 | Daemon via launchd LaunchAgent re-exec (not `fork()` or `daemonize` crate) | ✓ Good — required: fork after Mach port init is UB | v1.1 |
| D-11 | CA cert tag `speck-ca-certs` as compile-time constant (not env var) | ✓ Good — no runtime misconfiguration possible | v1.1 |
| D-12 | Duplicate CA certs: silent SHA256 dedup (no warning) | — Pending feedback | v1.1 |
| D-13 | `update-ca-certificates` called without timeout; failure is non-fatal | — Revisit if slow bundles reported | v1.1 |
| D-14 | `spk doctor dns` uses host `getaddrinfo` (same path as vsock proxy) — no container lifecycle needed | ✓ Good — simpler, equally correct | v1.1 |
| D-15 | `mockall` 0.15.0 approved for test-only deps via supply-chain audit | ✓ Good — enables mock-based unit tests without network/root | v1.2 |
| D-16 | `secrecy::SecretString` for registry password (zeroize-on-drop, base64-preserving Debug) | ✓ Good — no credential leakage in logs | v1.2 |
| D-17 | `Syscalls` trait in vminitd for testable mount/chroot/sysctl orchestration | ✓ Good — 12 tests pass on macOS without root | v1.2 |
| D-18 | Phase 17.1/19 gap-closure inserted phases (decimal numbering) for follow-stream/HostIp/network validation | ✓ Good — clear insertion semantics without renumbering | v1.2 |
| D-19 | Containerd transfer service for image pull (with unpack) — works | ✓ Good — container lifecycle works end-to-end | v1.2 |
| D-20 | Ad-hoc release distribution only; Developer ID deferred | ✓ Good — ships dev artifact pipeline; DEVID requires Apple Program | v1.2 |
| D-21 | **LOCKED:** Docker API = transparent byte proxy to guest dockerd + thin allowlisted middleware (binds/ports/HostIp/503 only). Reverses v1.2 CONF-01 (SpeckDockerd endpoint reimplementation against containerd), which caused the docker CLI 29.x conformance bug pile. Conformance gate: `scripts/conformance-smoke.sh` must pass. See CLAUDE.md "Architecture Invariants". | ✗ CONF-01 drifted — corrected | v1.3 |

## Evolution

This document evolves at phase transitions and milestone boundaries.

**After each phase transition** (via `/gsd-transition`):
1. Requirements invalidated? → Move to Out of Scope with reason
2. Requirements validated? → Move to Validated with phase reference
3. New requirements emerged? → Add to Active
4. Decisions to log? → Add to Key Decisions
5. "What This Is" still accurate? → Update if drifted

**After each milestone** (via `/gsd-complete-milestone`):
1. Full review of all sections
2. Core Value check — still the right priority?
3. Audit Out of Scope — reasons still valid?
4. Update Context with current state

---

*Last updated: 2026-07-09 after v1.2 milestone*
