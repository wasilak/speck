# Milestones

## v1.2 Hardened Runtime (Shipped: 2026-07-09)

**Phases completed:** 7 phases, 32 plans, 55 tasks

**Key accomplishments:**

- Supply-chain approval for mockall 0.15.0, secrecy 0.10.3, and temp-env 0.3.6 before any Cargo dependency changes
- Defined `pub trait Resolver` in speck-net/dns.rs, extracted `resolve_with_table` routing logic, and refactored `spawn_dns_proxy` to accept `Arc&lt;dyn Resolver&gt;` — creating the test seam that Plan 06 uses for DNS proxy tests without network access
- `spk up` now rejects stale managed VM assets with `spk up --pull` guidance and reports the exact `$SPECK_HOME/console.log` path for boot diagnostics.
- Extracted vminitd's inline mount orchestration into a testable `pub mod mount` behind a mockable `Syscalls` trait, with `LibcSyscalls` providing the real Linux implementation and the binary rewired to use it.
- Network-free DNS proxy integration tests covering NXDOMAIN, SERVFAIL, VPN-scoped, and default resolver routing paths using mockall::mock! and in-memory DNS packet fixtures
- Mock-tested mount ordering (mount_disks), root switch (chroot_into_rootfs), and sysctl parameter sequencing (configure_sysctl_params) through the Syscalls trait seam — all 12 tests pass on macOS without root.
- Wrapped RegistryAuth.password in secrecy::SecretString with zeroize-on-drop and proven Debug redaction — base64 encoding preserves the exposed value for Docker registry auth.
- Hardened `kill_via_pid_file` with ESRCH detection (stale PID cleanup) and non-fatal control socket connect in `run_down`; fixed `status.rs` PID path from `speck.pid` to `run/speck.pid`.
- `02f7b088`
- 1. [Plan clarification] Did NOT modify `guest::docker_api_unix_proxy` signature
- 1. [Rule 3 - Blocking] Added pub mod storage in Task 1 instead of Task 3
- 1. [Rule 1 - Adjustment] save_exec called before exec_store.insert
- Port bindings from `HostConfig.PortBindings` are now saved to SQLite on container create and injected into `NetworkSettings.Ports` in the container inspect response.
- Multiplexed Docker log frames via `encode_frame` backed by containerd task status, with `task_logs()` decoupling output-fetch from the handler.
- `spk up` now serves the Docker socket via the production SpeckDockerd axum server instead of a raw vsock byte-bridge, with container existence validation added to network connect/disconnect handlers.
- FIFO-backed container stdout/stderr relay over vminitd vsock port 9005, replacing the logs endpoint stub for real VM runs.
- Follow-mode live log streaming via mpsc/ReceiverStream + offset-based relay reads (CONF-05); HostIp default in port bindings (CONF-06)
- Conformance integration tests + SPECK_SOCK alignment landed; first-ever real-VM integration run passed CONF-03 (3/3 network-404 tests) but exposed a pre-existing host↔guest architecture break that blocks the 5 container-creating tests.
- Guest containerd vsock forwarding, BusyBox-safe blank-disk boot, and initrd rebuild/provisioning now boot a current guest, return `_ping`, and expose the next real conformance error instead of tonic BrokenPipe.
- CR-01 (empty-string HostIp) and CR-02 (bounded log retention) are closed with committed source changes and automated regression coverage.
- Fail-closed Developer ID preflight helpers with exact entitlement/notary parsing and a clean-host Cask validation contract
- Ad-hoc signed release binary packaged into explicitly non-notarized development `.pkg` and tar archive artifacts
- GitHub Release automation now publishes cargo xtask ad-hoc development artifacts with honest non-notarized Homebrew metadata
- Four surgical fixes to the control socket lifecycle: accept loop continues on transient errors (break→continue+backoff), read errors are logged via tracing::warn! instead of silently swallowed by unwrap_or(0), VmState::Restarting auto-resets to Running after 60s lease timeout, and shutdown_gracefully removes run/control.sock alongside speck.pid
- Set containerd Container.Runtime.Name to io.containerd.runc.v2, rebuilt initrd from current guest source, codesigned host binary — container creation no longer returns 400 Bad Request

---

## v1.1 — Production Runtime

**Shipped:** 2026-07-06
**Phases:** 07–14 (8 phases, 26 plans)
**Timeline:** 2026-07-01 → 2026-07-06 (5 days)
**Stats:** ~50 commits, 54 files changed, 7,736 insertions(+), 475 deletions(-), 17,048 Rust LOC total

### Delivered

`spk` graduated from a prototype to a daily-driver container runtime. Containers run end-to-end with working network egress, the daemon survives terminal close, DNS stays alive under VPN and WARP toggles, corporate CA certs inject cleanly, and `spk doctor` gives operators a single command to diagnose the full stack.

### Key Accomplishments

1. End-to-end container flow — network egress, Unix socket DockerClient, port bindings activated at container start
2. Persistent daemon via launchd LaunchAgent re-exec (zero fork/daemonize UB)
3. `$SPECK_HOME/config.yaml` with env > CLI > file > default precedence; vCPU/RAM/disk validation
4. `spk env` + idempotent `spk init` for bash/zsh/fish — one-time DOCKER_HOST setup
5. VPN-proof DNS: VM-internal proxy, SCDynamicStore live reload, split-DNS, NXDOMAIN→SERVFAIL
6. Corporate CA injection into guest OS trust bundle + containerd hosts.toml with fail-fast PEM validation
7. `spk doctor` 8-check health suite + `spk doctor dns <hostname>` resolver trace
8. Homebrew Formula (ad-hoc signed tarball) — `brew install speck-runtime/speck/spk` installs working `spk`

### Known Deferred Items

- DAEMON-04: First-class `spk restart` (workaround: `spk down && spk up`)
- DNS-01/03/05: Live lsof + WARP + VPN toggle verification (requires VPN-connected machine)
- BREW-DEVID: Developer ID + `.pkg` + notarytool + Cask distribution (requires Apple Developer Program)

### Archive

- Roadmap: `.planning/milestones/v1.1-ROADMAP.md`
- Requirements: `.planning/milestones/v1.1-REQUIREMENTS.md`

---

## v1.0 — Foundation (Phases 1–6)

*Shipped earlier — see v1.0 archive when created.*
