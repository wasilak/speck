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
- **Status:** 🏗️ In Progress (3/5 plans)
- **Goal:** Boot containerd + BuildKit inside the micro-VM supervised by vminitd (PID 1), reachable from the host via gRPC over vsock. Success = host pulls alpine image via containerd ImageService.
- **Requirements:** RUN-06
- **Plans:** 5 plans
- Plans:
  - [x] 05-01-PLAN.md — GuestConfig extensions (5 new fields) + Error variants + objc2-vz feature flags (Wave 1)
  - [x] 05-02-PLAN.md — Guest-side sock_forwarder + vminitd disk mounting + containerd/buildkitd supervision + READY signal (Wave 1)
  - [x] 05-03-PLAN.md — scripts/fetch-rootfs.sh + xtask init extension + .github/workflows/build-rootfs.yml (Wave 1)
  - [ ] 05-04-PLAN.md — VmThread disk attachment (VZVirtioBlockDeviceConfiguration) + WaitForGuestReady command + containerd-client dev-dep (Wave 2, has checkpoint)
  - [ ] 05-05-PLAN.md — Guest::wait_for_ready() + Guest::containerd_unix_proxy() + #[ignore]'d integration tests (Wave 3)

### Phase 6: Docker API Compat Layer
- **Status:** 📋 Backlog
