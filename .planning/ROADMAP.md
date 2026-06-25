# ROADMAP

### Phase 1: Foundation (crates, CI, xtask, codesign)
- **Status:** ✅ Complete
- **Goal:** Project scaffold, build system, CI pipeline, codesign entitlement

### Phase 2: FFI/Bridge + Bare VM Boot
- **Status:** ✅ Complete
- **Goal:** Boot a minimal Linux kernel to Running state via Virtualization.framework

### Phase 3: Vsock Echo — Host↔Guest Communication
- **Status:** 🔜 Planning Complete (4 plans, 3 waves)
- **Goal:** End-to-end host↔guest communication via virtio-vsock. Host connects to guest and performs a byte-level echo round-trip.
- **Design:** [`docs/superpowers/specs/2026-06-25-vsock-echo-design.md`](../docs/superpowers/specs/2026-06-25-vsock-echo-design.md)
- **Plans:**
  - [x] 03-01-PLAN.md — Foundation: feature flags, error types, VzSocket wrapper, vsock_port config
  - [ ] 03-02-PLAN.md — VmThread wiring: vsock device in VM config, socket device extraction, vsock_connect
  - [x] 03-03-PLAN.md — Guest side: vminitd with AF_VSOCK echo server (static musl binary)
  - [ ] 03-04-PLAN.md — Integration test: host→guest echo round-trip (checkpoint: human-verify)

### Phase 4: Guest Networking
- **Status:** 📋 Backlog
- **Goal:** Host-inheriting networking via `VZFileHandleNetworkDeviceAttachment` + user-space netstack

### Phase 5: containerd + BuildKit Integration
- **Status:** 📋 Backlog

### Phase 6: Docker API Compat Layer
- **Status:** 📋 Backlog
