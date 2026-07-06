# Speck (`spk`)

Ultra-fast, minimalist container runtime for Apple Silicon macOS. Runs containers in microscopic Linux micro-VMs via Apple's native `Virtualization.framework` — Docker-compatible API socket, millisecond cold starts, and network that survives corporate VPNs and Cloudflare WARP.

**Core value:** A container runtime on Apple Silicon that never loses the network — micro-VMs inherit the host's routing/DNS live, surviving corporate VPNs and Cloudflare WARP where Docker Desktop fails.

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

### Active (v1.2 candidates)

- [ ] **DAEMON-RESTART**: First-class `spk restart` subcommand (currently: `spk down && spk up`)
- [ ] **BREW-DEVID**: Developer ID + `.pkg` + notarytool + Cask distribution (deferred from v1.1: requires Apple Developer Program)
- [ ] **DNS-LIVE-VERIFY**: Live lsof + WARP + VPN-toggle verification of DNS-01/03/05 (deferred: requires VPN-connected machine)
- [ ] **TESTCONTAINERS**: Verified testcontainers-rust compatibility (bollard conformance suite gated on codesign)

### Out of Scope

- **Intel / x86_64 support** — Apple Silicon only, zero legacy debt
- **Host `:53` DNS binding** — architectural constraint D-09, never
- **double-fork / `daemonize` crate** — UB after Apple framework init, never
- **`VZNATNetworkDeviceAttachment`** — breaks WARP, decision D-02 is permanent
- **`com.apple.vm.networking` bridged networking** — Apple-gated entitlement, not on the critical path
- **Per-container CA injection post-start** — breaks image integrity
- **Synthetic subnets** — NET-06 constraint, never
- **QEMU / emulation** — defeats millisecond-start brand promise

## Context (after v1.1)

**Shipped v1.1** (2026-07-01 → 2026-07-06): 8 phases, 26 plans, ~50 commits, 54 files changed, 7,736 insertions.  
**Codebase:** 17,048 lines of Rust across `speck-vz`, `speck-net`, `speck-cli`, `speck-core`, `speck-guest`.  
**State:** `spk up` starts a background daemon; containers run; Docker socket works; DNS survives VPN toggles.  
**Known limitations:** `spk down` fails to stop processes not registered with launchd (boot via `cargo run` directly). First-class `spk restart` not yet implemented.  
**Next:** v1.2 planning — decide priority between testcontainers conformance, Developer ID distribution, and K3s/Compose.

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
*Last updated: 2026-07-06 after v1.1 milestone*
