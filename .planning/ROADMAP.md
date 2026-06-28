# ROADMAP

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

- **Status:** 🔧 In Progress (0/3 plans)
- **Goal:** Fix five verified wiring gaps in Phase 6 code that prevent the system from running end-to-end: proxy loop exits after one client, netstack never spawned, speck_home not created, port bindings never applied, VirtioFS unconditionally skipped.
- **Requirements:** NET-01, NET-02, NET-03, RUN-04, DOCKER-02, DOCKER-04
- **Depends on:** Phase 6
- **Plans:** 3 plans

Plans:

- [ ] 06.1-01-PLAN.md — Fix proxy loop (Bug 1) + VmThread helpers for netstack wiring (Bug 2-vz) + remove VirtioFS gate (Bug 5): vm_thread.rs + guest.rs (Wave 1)
- [ ] 06.1-02-PLAN.md — Wire SpeckNet in run_up: create_dir_all + GuestConfig network/dns/speck_home + SpeckNet::spawn + set_port_map_channel (Bugs 2-cli + 3): up.rs (Wave 1)
- [ ] 06.1-03-PLAN.md — Apply port bindings in container start() handler (Bug 4): containers.rs (Wave 2)
