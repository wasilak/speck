# ROADMAP

---

## Milestone v1.0 — Foundation (Complete)

### Phase 1: Foundation (crates, CI, xtask, codesign)

- **Status:** ✅ Complete
- **Goal:** Project scaffold, build system, CI pipeline, codesign entitlement

### Phase 2: FFI/Bridge + Bare VM Boot

- **Status:** ✅ Complete
- **Goal:** Boot a minimal Linux kernel to Running state via Virtualization.framework

### Phase 3: Vsock Echo — Host↔Guest Communication

- **Status:** ✅ Complete (4/4 plans)
- **Goal:** End-to-end host↔guest communication via virtio-vsock. Host connects to guest and performs a byte-level echo round-trip.
- **Design:** [`docs/superpowers/specs/2026-06-25-vsock-echo-design.md`](../docs/superpowers/specs/2026-06-25-vsock-echo-design.md)
- **Plans:**
  - [x] 03-01-PLAN.md — Foundation: feature flags, error types, VzSocket wrapper, vsock_port config
  - [x] 03-02-PLAN.md — VmThread wiring: vsock device in VM config, socket device extraction, vsock_connect
  - [x] 03-03-PLAN.md — Guest side: vminitd with AF_VSOCK echo server (static musl binary)
  - [x] 03-04-PLAN.md — Integration test: host→guest echo round-trip (written, compiles, gated on codesigning)

### Phase 4: Guest Networking

- **Status:** ✅ Complete (6/6 plans)
- **Goal:** Host-inheriting networking via `VZFileHandleNetworkDeviceAttachment` + user-space netstack
- **Plans:**
  - [x] 04-01-PLAN.md — speck-net crate scaffold + NetworkConfig type + workspace member (Wave 1)
  - [x] 04-02-PLAN.md — FdDevice (smoltcp Device trait) + SmoltcpInterface + SpeckNet::spawn() poll loop (Wave 2)
  - [x] 04-03-PLAN.md — VM network device wiring: VZFileHandleNetworkDeviceAttachment, socketpair, host_fd, DNS vsock port (Wave 2)
  - [x] 04-04-PLAN.md — TCP re-origination + DHCP server + vsock DNS proxy + MTU/MSS clamping (Wave 3)
  - [x] 04-05-PLAN.md — Integration test: ARP round-trip + DNS proxy via vsock (Wave 3, human-verify)
  - [x] 04-06-PLAN.md — Guest-side DNS forwarder in vminitd (Wave 3)

### Phase 5: containerd + BuildKit Integration

- **Status:** ✅ Complete (5/5 plans)
- **Goal:** Boot containerd + BuildKit inside the micro-VM supervised by vminitd (PID 1), reachable from the host via gRPC over vsock. Success = host pulls alpine image via containerd ImageService.
- **Requirements:** RUN-06
- **Plans:** 5/5 plans complete
- Plans:
  - [x] 05-01-PLAN.md — GuestConfig extensions (5 new fields) + Error variants + objc2-vz feature flags (Wave 1)
  - [x] 05-02-PLAN.md — Guest-side sock_forwarder + vminitd disk mounting + containerd/buildkitd supervision + READY signal (Wave 1)
  - [x] 05-03-PLAN.md — scripts/fetch-rootfs.sh + xtask init extension + .github/workflows/build-rootfs.yml (Wave 1)
  - [x] 05-04-PLAN.md — VmThread disk attachment (VZVirtioBlockDeviceConfiguration) + WaitForGuestReady command + containerd-client dev-dep (Wave 2, has checkpoint)
  - [x] 05-05-PLAN.md — Guest::wait_for_ready() + Guest::containerd_unix_proxy() + #[ignore]'d integration tests (Wave 3)

### Phase 6: Docker API Compat Layer

- **Status:** ✅ Complete (11/11 plans)
- **Goal:** Full Speck v1 product — Docker-compatible API socket, container lifecycle, full CLI, VirtioFS volumes, spk build via BuildKit, distribution signing
- **Requirements:** DOCKER-01, DOCKER-02, DOCKER-03, DOCKER-04, DOCKER-05, RUN-01, RUN-02, RUN-03, RUN-04, RUN-05, RUN-07, RUN-08, CLI-01, CLI-02, CLI-03, CLI-04, CLI-05, CLI-06, STORAGE-01, STORAGE-02, STORAGE-03, STORAGE-04, BUILD-01, BUILD-02, BUILD-03, DIST-01, DIST-02, DIST-03
- **Plans:** 11/11 plans complete
- Plans:
  - [x] 06-01-PLAN.md — speck-core types: Container, Image, Volume, Network domain types (Wave 1)
  - [x] 06-02-PLAN.md — speck-dockerd scaffold: axum server, hyper_util Unix socket + upgrades, stream.rs frame encode/decode, router + handler stubs (Wave 1)
  - [x] 06-03-PLAN.md — Docker API: system (/_ping, /version, /info) + container lifecycle + exec (Wave 2)
  - [x] 06-04-PLAN.md — Docker API: attach hijack, logs streaming, image pull/push with registry auth, events SSE, networks + volumes (Wave 2)
  - [x] 06-05-PLAN.md — Port publishing: PortPublishBridge in speck-net + smoltcp active-connect to guest IP (Wave 2)
  - [x] 06-06-PLAN.md — VirtioFS volumes: multi-device VZVirtioFileSystemDeviceConfiguration + vminitd auto-mount + Ryuk docker.sock symlink (Wave 3)
  - [x] 06-07-PLAN.md — BuildKit: vendored proto + tonic codegen + POST /build handler + Guest::buildkitd_unix_proxy() (Wave 3)
  - [x] 06-08-PLAN.md — CLI: full speck-cli with clap v4, indicatif, anstream theme, DockerClient, all subcommands (Wave 4)
  - [x] 06-09-PLAN.md — spk dashboard: ratatui TUI with container list + log tail + keyboard navigation (Wave 5)
  - [x] 06-10-PLAN.md — Codesigning + CI: xtask codesign-dev, release.yml Developer ID + notarytool, Homebrew Formula (Wave 5)
  - [x] 06-11-PLAN.md — testcontainers conformance: bollard api_conformance.rs + integration_06.rs end-to-end (Wave 5)

### Phase 06.1: Fix 5 integration blockers — Unix socket, netstack wiring, proxy loop, port map, virtiofs (INSERTED)

- **Status:** 🔧 In Progress (3/5 plans — gap closure in progress)
- **Goal:** Fix five verified wiring gaps in Phase 6 code that prevent the system from running end-to-end: proxy loop exits after one client, netstack never spawned, speck_home not created, port bindings never applied, VirtioFS unconditionally skipped.
- **Requirements:** NET-01, NET-02, NET-03, RUN-04, DOCKER-02, DOCKER-04
- **Depends on:** Phase 6
- **Plans:** 5 plans

Plans:
**Wave 1**

- [x] 06.1-01-PLAN.md — Fix proxy loop (Bug 1) + VmThread helpers for netstack wiring (Bug 2-vz) + remove VirtioFS gate (Bug 5): vm_thread.rs + guest.rs (Wave 1)
- [ ] 06.1-04-PLAN.md — CR-01 gap closure: wrap Guest control sequence in tokio::task::spawn_blocking to fix runtime panic in run_up (Wave 1)
- [ ] 06.1-05-PLAN.md — CR-02+CR-03 gap closure: delegate_rx drain thread (VM death → state=Stopped) + missing objc2-vz feature flags (Wave 1)

**Wave 2** *(blocked on Wave 1 completion)*

- [x] 06.1-02-PLAN.md — Wire SpeckNet in run_up: create_dir_all + GuestConfig network/dns/speck_home + SpeckNet::spawn + set_port_map_channel (Bugs 2-cli + 3): up.rs (Wave 1)

**Wave 3** *(blocked on Wave 2 completion)*

- [x] 06.1-03-PLAN.md — Apply port bindings in container start() handler (Bug 4): containers.rs (Wave 2)

---

## Milestone v1.1 — Production Runtime

### Phases

- [x] **Phase 07: Gap Closure** — Close remaining v1.0 gaps so `spk run` works end-to-end (completed 2026-07-02)
- [x] **Phase 08: Config File & VM Resource Controls** — Persistent config file with VM right-sizing and env/flag/file precedence (completed 2026-07-03)
- [ ] **Phase 09: Daemon Lifecycle** — Background daemon via launchd LaunchAgent with `spk down` and `spk restart`
- [ ] **Phase 10: Shell Environment Integration** — `spk env` for immediate use; idempotent `spk init` for persistent `DOCKER_HOST`
- [ ] **Phase 11: VPN-Proof DNS** — Live DNS reload via SCDynamicStore/vsock; no host `:53` binding; split-DNS + SERVFAIL translation
- [ ] **Phase 12: Corporate CA Injection** — Custom CA certs injected into guest trust bundle before containerd/buildkitd start
- [ ] **Phase 13: Diagnostics** — `spk doctor` health suite covering DNS, certs, VM state, and socket reachability
- [ ] **Phase 14: Homebrew Distribution** — Signed `.pkg` Cask artifact that preserves the virtualization entitlement through Homebrew re-signing

### Phase Details

### Phase 07: Gap Closure

**Goal**: Users can run containers end-to-end — network egress works, Docker client connects via Unix socket, and published ports are reachable on macOS localhost
**Depends on**: Phase 06.1
**Requirements**: GAP-01, GAP-02, GAP-03, GAP-04
**Success Criteria** (what must be TRUE):

  1. `spk run alpine ping -c 1 8.8.8.8` exits 0 with round-trip output — containers have working network egress via the wired-up SpeckNet
  2. `spk ps` lists running containers by connecting over Unix socket — not TCP — confirming DockerClient connects correctly
  3. `spk run -p 8080:80 nginx` followed by `curl http://localhost:8080` from the macOS terminal returns an HTTP response — port bindings activate at container start
  4. `spk up` completes without a runtime panic — `spawn_blocking` fix is wired, VM delegate drain thread is active, and all required objc2-vz feature flags are present

**Plans**: 5 plans
Plans:
**Wave 1**

- [x] 07-01-PLAN.md — GAP-01 VM lifecycle: spawn_blocking, delegate drain, objc2-vz features (Wave 1)
- [x] 07-02-PLAN.md — GAP-03 DockerClient Unix socket transport for JSON/raw/streaming paths (Wave 1)
- [x] 07-03-PLAN.md — D-05 VirtioFS bind mounts at container lifecycle while preserving GAP-04 port maps (Wave 1)
- [x] 07-04-PLAN.md — D-06 BuildKit build context tar forwarding (Wave 1)

**Wave 2** *(blocked on Wave 1 completion)*

- [x] 07-05-PLAN.md — D-03/D-04 ignored integration_07 E2E tests and Nyquist validation closeout (Wave 2)

### Phase 08: Config File & VM Resource Controls

**Goal**: Users can set VM resource defaults once in a config file and override them with CLI flags or env vars without repeating arguments every invocation
**Depends on**: Phase 07
**Requirements**: CFG-01, CFG-02, CFG-03, VMCFG-01, VMCFG-02, VMCFG-03, VMCFG-04
**Success Criteria** (what must be TRUE):

  1. A user sets `vm.cpus: 4` in `$SPECK_HOME/config.yaml` and `spk up` applies that CPU count without any CLI flag — config file is read on startup
  2. `spk up --cpus 2 --memory 2048` overrides config file values for that invocation; `SPECK_VM_CPUS=8` overrides the CLI flag — env > CLI > file precedence holds
  3. Attempting `--disk 10` when the current disk image is 20 GB prints a clear "shrink not supported" error and exits non-zero — disk can grow, shrink is an explicit error
  4. Adding an unknown key to `config.yaml` produces a startup warning; the process does not panic or refuse to start
  5. Changing `--cpus` or `--memory` on a running VM causes `spk up` to print a restart-required message before proceeding

**Plans**: 3 plans

Plans:
**Wave 1**

- [x] 08-01-PLAN.md — Config schema, CLI flags, env/CLI/file/default resolver, and CPU/memory validation (Wave 1)
- [x] 08-03-PLAN.md — Guest-side ext4 growth for configured data disk size (Wave 1)

**Wave 2** *(blocked on Wave 1 config resolver completion)*

- [x] 08-02-PLAN.md — Wire resolved config into `spk up`, grow-only host disk reconciliation, and restart-required detection (Wave 2)

### Phase 09: Daemon Lifecycle

**Goal**: `spk` persists as a launchd background service across terminal sessions and can be started, stopped, and restarted without touching system internals
**Depends on**: Phase 08
**Requirements**: DAEMON-01, DAEMON-02, DAEMON-03, DAEMON-04, DAEMON-05
**Success Criteria** (what must be TRUE):

  1. Running `spk up`, closing the terminal, then running `docker ps` in a new terminal still lists running containers — daemon survives terminal close
  2. Running `spk down` stops the VM and the background process cleanly; subsequent `docker ps` fails to connect
  3. Running `spk restart` brings the runtime back up without manual intervention — graceful stop followed by fresh start
  4. Running `spk up --foreground` streams structured logs to stdout until interrupted — suitable for CI environments
  5. `$SPECK_HOME/speck.log` grows with log entries while the daemon is running; daemon liveness is detected via `$SPECK_HOME/run/control.sock`, not a PID file

**Plans**: TBD

### Phase 10: Shell Environment Integration

**Goal**: Docker-compatible tools connect to Speck automatically after a one-time `spk init` setup with no manual `DOCKER_HOST` export required in future sessions
**Depends on**: Phase 09
**Requirements**: SHELL-01, SHELL-02, SHELL-03
**Success Criteria** (what must be TRUE):

  1. Running `eval $(spk env)` in any terminal makes `docker ps` connect to Speck's socket — the command emits correct shell-ready export statements
  2. Running `spk init` once and opening a new shell session makes `DOCKER_HOST` point to Speck automatically via the written dotfile block
  3. Running `spk init` a second time produces no duplicate block in `~/.zshrc`, `~/.bashrc`, or fish config — idempotent across any number of invocations
  4. Running `spk init` when `DOCKER_HOST` is already set to another runtime prints a warning before writing the block — opt-in behavior, not a silent hijack

**Plans**: TBD

### Phase 11: VPN-Proof DNS

**Goal**: Container DNS resolves correctly under Cloudflare WARP and corporate VPN configurations — live, without a VM restart — and the host port 53 is never bound
**Depends on**: Phase 10
**Requirements**: DNS-01, DNS-02, DNS-03, DNS-04, DNS-05, DNS-06
**Success Criteria** (what must be TRUE):

  1. `nslookup google.com` inside a running container succeeds while Cloudflare WARP is active — WARP resolvers at `127.0.2.2/3` are reachable from the host DNS handler via vsock
  2. Toggling WARP off then on and running `nslookup` inside a container within 5 seconds succeeds without a VM restart — SCDynamicStore event triggers live resolver table update
  3. A VPN-scoped internal hostname (e.g., `internal.corp.example`) resolves correctly from inside a container via split-DNS routing to the VPN's nameservers
  4. `lsof -i :53` on the macOS host while Speck is running shows no Speck process bound to port 53 — the DNS proxy runs inside the VM network namespace only
  5. Immediately after VPN reconnect, `nslookup internal.corp` from a container returns SERVFAIL rather than a cached NXDOMAIN — clients retry after reconnect instead of caching a permanent failure

**Plans**: TBD

### Phase 12: Corporate CA Injection

**Goal**: Containers pull images from private registries and make HTTPS calls to internal services without TLS errors when custom CA certificates are configured
**Depends on**: Phase 11
**Requirements**: CERT-01, CERT-02, CERT-03, CERT-04
**Success Criteria** (what must be TRUE):

  1. Adding a corporate CA cert path to `config.yaml` under `ca.extra_certs` and running `spk up` makes `docker pull internal.registry.corp/image:tag` succeed without TLS errors
  2. `curl https://internal.api.corp` from inside a running container exits 0 without certificate warnings — cert is present in guest OS trust bundle and in containerd `hosts.toml`
  3. Specifying a non-existent cert path in `config.yaml` causes `spk up` to exit before the VM starts with a clear file-not-found error message
  4. Specifying a file that is not valid PEM causes `spk up` to exit before the VM starts with a "not valid PEM" error — no silent partial injection

**Plans**: TBD

### Phase 13: Diagnostics

**Goal**: Users and support engineers can diagnose the full Speck runtime health — DNS, networking, certs, VM state, and socket reachability — with a single command
**Depends on**: Phase 12
**Requirements**: DOCTOR-01, DOCTOR-02, DOCTOR-03
**Success Criteria** (what must be TRUE):

  1. `spk doctor` when everything is healthy exits 0 and prints a structured pass/fail summary covering: codesign entitlement, VM running state, Docker socket reachability, `DOCKER_HOST` accuracy, public DNS resolution, VPN-scoped DNS resolution, cert injection status, and VM resource utilization
  2. `spk doctor` when the Docker socket is unreachable exits 1 and names the failing check with an actionable hint (e.g., "run `spk up`")
  3. `spk doctor dns google.com` reports which resolver answered, the IP(s) returned, and whether the result matches the macOS system resolver — full path tracing through guest DNS
  4. `spk doctor` when `DOCKER_HOST` points to a different runtime's socket prints a WARN line identifying the conflict

**Plans**: TBD

### Phase 14: Homebrew Distribution

**Goal**: macOS developers install and upgrade Speck via Homebrew with zero manual signing steps and full entitlement preservation through the tap's re-signing step
**Depends on**: Phase 13
**Requirements**: BREW-01, BREW-02, BREW-03
**Success Criteria** (what must be TRUE):

  1. `brew install speck-runtime/speck/spk` on a fresh machine installs a working `spk` binary — `spk doctor` codesign check passes
  2. Running `spk up` after a Homebrew install succeeds without a "virtualization entitlement missing" or codesigning error
  3. `brew upgrade spk` produces a new binary where `spk up` still works — the `com.apple.security.virtualization` entitlement survives Homebrew's re-signing step

**Plans**: TBD

### Progress

| Phase | Plans Complete | Status | Completed |
|-------|----------------|--------|-----------|
| 07. Gap Closure | 5/5 | Complete   | 2026-07-02 |
| 08. Config File & VM Resources | 3/3 | Complete   | 2026-07-03 |
| 09. Daemon Lifecycle | 0/TBD | Not started | - |
| 10. Shell Integration | 0/TBD | Not started | - |
| 11. VPN-Proof DNS | 0/TBD | Not started | - |
| 12. Corporate CA Injection | 0/TBD | Not started | - |
| 13. Diagnostics | 0/TBD | Not started | - |
| 14. Homebrew Distribution | 0/TBD | Not started | - |
