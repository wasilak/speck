# Roadmap: Speck (`spk`)

## Overview

Speck v1 is the "Full CLI vision" — a product-grade, Apple-Silicon-only container runtime. The roadmap is risk-first and horizontally layered: it front-loads the two least-reversible seams (the Rust⇄native hypervisor FFI/threading model, and the VPN/WARP-resilient host-inheriting network path) before building any feature breadth. The journey runs from an empty repo and an entitlement skeleton, through a bare VM boot, vsock plumbing, the networking thesis (the go/no-go core value), in-guest containerd, a first VPN-resilient `spk run` loop, then VirtioFS volumes, BuildKit, the Docker-compatible socket, CLI/TUI polish, and finally notarized Homebrew distribution. The one ordering invariant: **networking (Phase 4) precedes containerd (Phase 5)** because containerd's registry pulls depend on the network path existing.

## Phases

**Phase Numbering:**
- Integer phases (1, 2, 3): Planned milestone work
- Decimal phases (2.1, 2.2): Urgent insertions (marked with INSERTED)

Decimal phases appear between their surrounding integers in numeric order.

- [x] **Phase 1: Foundation** - Workspace, Core/Frontend seam, entitlement & signing skeleton, AGPLv3/CLA scaffolding
- [ ] **Phase 2: FFI/Bridge + Bare VM Boot** - Boot a signed micro-VM to Running; resolve the binding decision
- [ ] **Phase 3: vsock Plumbing** - Reliable host⇄guest vsock channel with reconnect/health-check
- [ ] **Phase 4: Host-Inheriting Netstack + Live DNS** - The thesis: VPN/WARP-resilient networking (go/no-go)
- [ ] **Phase 5: In-Guest containerd + vminitd** - Control plane over vsock; the CRI/K3s seam
- [ ] **Phase 6: First VPN-Resilient Run Loop** - `spk run` end-to-end surviving a live VPN/WARP toggle (go/no-go gate)
- [ ] **Phase 7: VirtioFS Volumes + Named Volumes** - Correct bind mounts and named volumes
- [ ] **Phase 8: Image Build (BuildKit)** - `spk build` via in-guest BuildKit with crash recovery
- [ ] **Phase 9: Docker-Compatible API Socket** - Mountable docker.sock passing testcontainers + Compose
- [ ] **Phase 10: CLI/TUI Polish** - clap surface, cyberpunk theme, progress, tracing, ratatui dashboard, completions
- [ ] **Phase 11: Distribution** - Developer ID notarization + stapling and Homebrew install

## Phase Details

### Phase 1: Foundation
**Goal**: A Cargo workspace exists where business logic is structurally isolated from presentation, every build is codesigned with the virtualization entitlement, and the project's licensing/contribution scaffolding is in place.
**Depends on**: Nothing (first phase)
**Requirements**: ENGINE-05, ARCH-01, ARCH-02, ARCH-03, DIST-01, DIST-04
**Success Criteria** (what must be TRUE):
  1. `speck-core` compiles independently as a `.a`/`.dylib` with no dependency on any frontend crate, and the workspace builds (`speck-core`, `speck-cli`, `speck-vm-*`, `speck-dockerd`, `speck-guest`, `xtask`).
  2. A CI lint fails the build if `print!`/`println!`/`eprintln!` appears anywhere in `speck-core`; Core emits only through an injected `EventSink`.
  3. Every build (even dev) is codesigned, and CI asserts the `com.apple.security.virtualization` key is present via `codesign -d --entitlements -`.
  4. The build refuses/declines to target anything other than `aarch64-apple-darwin`.
  5. LICENSE (AGPLv3), a CLA process, and a documented "Speck Enterprise" exception path are present in the repo.
**Plans**: 3 plans

Plans:
- [x] 01-01-PLAN.md — Workspace scaffold + EventSink + no-print denials + hard-target cfg gate
- [x] 01-02-PLAN.md — xtask + CI + entitlements plist
- [x] 01-03-PLAN.md — AGPLv3 LICENSE + CLA.md + CONTRIBUTING.md + CLA workflow

### Phase 2: FFI/Bridge + Bare VM Boot
**Goal**: A signed, installed binary boots a bare micro-VM to a Running state in milliseconds and shuts it down cleanly under stress, with the Rust⇄native binding decision settled empirically.
**Depends on**: Phase 1
**Requirements**: ENGINE-01, ENGINE-02, ENGINE-03
**Success Criteria** (what must be TRUE):
  1. A bare micro-VM (Kata arm64 kernel + initrd) boots to `[Running]` via `Virtualization.framework` and reaches Running in milliseconds, not seconds.
  2. The VM boots from a *signed, installed* binary carrying the entitlement — not only under `cargo run`.
  3. A boot/shutdown stress loop runs clean with no libdispatch/Obj-C threading crashes (the single-serial-queue ownership contract holds).
  4. The contested binding decision (`objc2-virtualization` native vs. `swift-bridge`) is resolved with a documented rationale and a working boot spike on the chosen path.
**Plans**: 3 plans
**Research**: yes (binding decision must be settled empirically; runtime-init-in-Rust-main and dual-async-runtime bridging are genuinely hard)

Plans:
- [x] 02-01-PLAN.md — objc2 ecosystem deps, Error types, VmThread with dispatch queue ✦ DONE

### Phase 3: vsock Plumbing
**Goal**: The Rust core can reliably reach in-guest services over a `VZVirtioSocketDevice` channel that survives drops, treated as a fallible Unix-socket-backed transport from the start.
**Depends on**: Phase 2
**Requirements**: ENGINE-04
**Success Criteria** (what must be TRUE):
  1. The host dials a guest vsock echo server and exchanges data; the FD is handed to Tokio off the VM's serial queue (no blocking the lifecycle queue).
  2. The channel auto-reconnects and reports health after an induced drop/stall.
  3. The vsock seam is exercised under concurrent connections without deadlocking the VM state machine.
**Plans**: TBD

Plans:
- [ ] 03-01: TBD

### Phase 4: Host-Inheriting Netstack + Live DNS
**Goal**: A guest process reaches the internet and resolves internal corporate hostnames using whatever route and DNS macOS currently has — and keeps working across a live VPN/WARP toggle. This is the product thesis and the go/no-go gate.
**Depends on**: Phase 3
**Requirements**: NET-01, NET-02, NET-03, NET-04, NET-05, NET-06
**Success Criteria** (what must be TRUE):
  1. Guest egress goes through a user-space host netstack (`VZFileHandleNetworkDeviceAttachment`, never `VZNATNetworkDeviceAttachment`) that re-originates connections from the host network namespace; no synthetic router/subnet is created.
  2. The micro-VM inherits the host's live routing table — a guest process reaches the internet via the host's current route.
  3. DNS is resolved host-side from SystemConfiguration (`SCDynamicStore`) honoring scoped/split resolvers — a guest process resolves an internal corporate hostname.
  4. An in-flight `ping`/`curl` from inside the guest keeps working across a **live Cloudflare WARP / corporate VPN toggle** with no restart.
  5. Egress MTU (~1420 under WARP) is detected on every network change and propagated to the guest NIC with MSS clamping, so large transfers complete.
**Plans**: TBD
**Research**: yes (MANDATORY deep research — the defining risk; no off-the-shelf Apple primitive; embed-vs-fork `gvisor-tap-vsock` vs. native Rust netstack, `SCDynamicStore` privilege boundary, scoped-resolver handling, MTU/MSS under WARP all need spiking)

Plans:
- [ ] 04-01: TBD

### Phase 5: In-Guest containerd + vminitd over vsock
**Goal**: Stock containerd runs in-guest supervised by a `vminitd` PID 1, reachable from Core as a gRPC/CRI client over vsock, preserving the K3s/CRI seam.
**Depends on**: Phase 4
**Requirements**: RUN-06
**Success Criteria** (what must be TRUE):
  1. `vminitd` (static PID 1) supervises containerd in-guest; Core connects as a `tonic` gRPC/CRI client over vsock.
  2. The supervised daemon survives an induced crash and reconnects without rebooting the VM.
  3. The containerd version is pinned to an exact, mutually-tested release (no `latest`).
  4. The control path is CRI-shaped (containerd in-guest, not runc-on-host translation), keeping the K3s drop-in seam intact.
**Plans**: TBD
**Research**: yes (vsock reliability, content-store persistence, and daemon supervision warrant a defect-catalog review of apple/container issues)

Plans:
- [ ] 05-01: TBD

### Phase 6: First VPN-Resilient Run Loop
**Goal**: A user can `spk run` a container end-to-end, with image pulls traversing the host-inherited network path, and the run survives a live VPN/WARP toggle. This is the explicit go/no-go gate.
**Depends on**: Phase 5
**Requirements**: RUN-01, RUN-02, RUN-03, RUN-04, RUN-05, RUN-07, RUN-08
**Success Criteria** (what must be TRUE):
  1. `spk run <image>` pulls and runs a container with logs streamed back; `spk ps`/`spk down`/`spk stop`/`spk rm` manage lifecycle; `spk exec` enters a running container interactively.
  2. `spk run alpine ping ...` keeps working across a **live WARP/VPN toggle** without restart, and image pull from a private registry succeeds while on a corporate VPN.
  3. Pulled images persist across `spk down` + restart (content store on a re-attached virtio-blk data disk).
  4. Ports publish to macOS localhost (`-p host:container`) without reintroducing a synthetic NAT layer.
  5. Env vars, restart policies, and resource limits can be set on a container and take effect.
**Plans**: TBD

Plans:
- [ ] 06-01: TBD

### Phase 7: VirtioFS Volumes + Named Volumes
**Goal**: Host directories bind-mount into containers correctly (permissions, case, read-only) via VirtioFS, named volumes work, and large-tree performance is measured against OrbStack.
**Depends on**: Phase 6
**Requirements**: STORAGE-01, STORAGE-02, STORAGE-03, STORAGE-04
**Success Criteria** (what must be TRUE):
  1. A user can bind-mount a host directory into a container (`-v host:container`) via `VZVirtioFileSystemDeviceConfiguration`.
  2. A correctness test corpus passes: explicit uid/gid mapping, `chown`, read-only files, and case-conflict detection/warning all behave correctly.
  3. A user can create and use named volumes backed by guest storage.
  4. Large-tree bind-mount performance is benchmarked against OrbStack with documented expectations.
**Plans**: TBD

Plans:
- [ ] 07-01: TBD

### Phase 8: Image Build (BuildKit)
**Goal**: A user can `spk build` an image from a Dockerfile via in-guest BuildKit over vsock, with cache and recovery from interrupted builds.
**Depends on**: Phase 7
**Requirements**: BUILD-01, BUILD-02, BUILD-03
**Success Criteria** (what must be TRUE):
  1. `spk build` produces an image from a Dockerfile via in-guest buildkitd over vsock.
  2. Builds honor `.dockerignore`, multi-stage stages, and layer cache.
  3. A killed/interrupted build is recoverable without restarting the VM, against a pinned BuildKit version.
**Plans**: TBD
**Research**: yes (BuildKit crash recovery and version drift — apple/container #284 — warrant a defect-catalog review)

Plans:
- [ ] 08-01: TBD

### Phase 9: Docker-Compatible API Socket
**Goal**: `speck-dockerd` serves a real, mountable Docker Engine REST API socket so existing tooling — Compose, testcontainers, IDEs — works unchanged, validated by a conformance suite in CI.
**Depends on**: Phase 8
**Requirements**: DOCKER-01, DOCKER-02, DOCKER-03, DOCKER-04, DOCKER-05
**Success Criteria** (what must be TRUE):
  1. Speck serves the Docker Engine REST API (baseline v1.43–v1.44) over a real Unix socket with `/_ping` + `/version` negotiation.
  2. The socket implements the containers, images, events, networks, and volumes endpoints common tooling needs (create/start/attach/logs/wait/exec, pull/build/push).
  3. `docker compose up` works unchanged against the Speck socket.
  4. testcontainers (Java + Go) suites pass against Speck, including Ryuk's requirement that the socket be mountable into a container.
  5. A conformance suite gates this in CI — not just `docker ps`.
**Plans**: TBD
**Research**: yes (the Engine API surface is large and behaviorally subtle; needs a conformance-suite design covering which exact endpoints/shapes testcontainers + Compose hit)

Plans:
- [ ] 09-01: TBD

### Phase 10: CLI/TUI Polish
**Goal**: A complete, beautiful presentation layer sits over the proven headless engine — full command surface, cyberpunk theme, progress, structured logging, a live dashboard, and shell completions.
**Depends on**: Phase 9
**Requirements**: CLI-01, CLI-02, CLI-03, CLI-04, CLI-05, CLI-06
**Success Criteria** (what must be TRUE):
  1. The CLI exposes a coherent `clap` v4 command surface (`spk up`/`down`/`ps`/`run`/`build`/`exec`/...).
  2. Output uses a cyberpunk (neon cyan/violet) theme that honors `NO_COLOR` and `CLICOLOR`; long operations show `indicatif` progress bars with time estimates.
  3. Microsecond-resolution structured logging is available via `tracing`.
  4. `spk dashboard` opens an interactive `ratatui` TUI showing live container state.
  5. `spk completion <shell>` generates native completions for zsh, fish, and bash.
**Plans**: TBD
**UI hint**: yes

Plans:
- [ ] 10-01: TBD

### Phase 11: Distribution
**Goal**: Release binaries are Developer ID signed, notarized, and stapled — and still boot a VM under Gatekeeper — and Speck installs via Homebrew with the entitlement preserved.
**Depends on**: Phase 10
**Requirements**: DIST-02, DIST-03
**Success Criteria** (what must be TRUE):
  1. Release binaries are Developer ID signed with hardened runtime, notarized (`notarytool`), and stapled; `spctl -a -vv` accepts a quarantined copy.
  2. A quarantined, notarized copy still boots a VM (hardened-runtime/entitlement interaction verified).
  3. Speck is installable via a Homebrew Formula, and re-signing on install preserves the virtualization entitlement.
**Plans**: TBD
**Research**: no (the Developer ID → notarytool → stapler chain is a documented, mechanical pipeline)

Plans:
- [ ] 11-01: TBD

## Progress

**Execution Order:**
Phases execute in numeric order: 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9 → 10 → 11

| Phase | Plans Complete | Status | Completed |
|-------|----------------|--------|-----------|
| 1. Foundation | 3/3 | ✅ Complete | 2026-06-25 |
| 2. FFI/Bridge + Bare VM Boot | 1/3 | In progress | - |
| 3. vsock Plumbing | 0/TBD | Not started | - |
| 4. Host-Inheriting Netstack + Live DNS | 0/TBD | Not started | - |
| 5. In-Guest containerd + vminitd | 0/TBD | Not started | - |
| 6. First VPN-Resilient Run Loop | 0/TBD | Not started | - |
| 7. VirtioFS Volumes + Named Volumes | 0/TBD | Not started | - |
| 8. Image Build (BuildKit) | 0/TBD | Not started | - |
| 9. Docker-Compatible API Socket | 0/TBD | Not started | - |
| 10. CLI/TUI Polish | 0/TBD | Not started | - |
| 11. Distribution | 0/TBD | Not started | - |
