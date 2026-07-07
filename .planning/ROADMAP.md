# ROADMAP

---

## Milestones

- ✅ **v1.0 Foundation** — Phases 1–6 (shipped prior)
- ✅ **v1.1 Production Runtime** — Phases 07–14 (shipped 2026-07-06)
- 📋 **v1.2 Hardened Runtime** — Phases 15–18 (in progress)

---

## Phases

<details>
<summary>✅ v1.1 Production Runtime (Phases 07–14) — SHIPPED 2026-07-06</summary>

- [x] Phase 07: Gap Closure (5/5 plans) — completed 2026-07-02
- [x] Phase 08: Config File & VM Resource Controls (3/3 plans) — completed 2026-07-03
- [x] Phase 09: Daemon Lifecycle (3/3 plans) — completed 2026-07-03
- [x] Phase 10: Shell Environment Integration (2/2 plans) — completed 2026-07-04
- [x] Phase 11: VPN-Proof DNS (5/5 plans) — completed 2026-07-04
- [x] Phase 12: Corporate CA Injection (3/3 plans) — completed 2026-07-05
- [x] Phase 13: Diagnostics (2/2 plans) — completed 2026-07-05
- [x] Phase 14: Homebrew Distribution (3/3 plans) — completed 2026-07-06

Full details: `.planning/milestones/v1.1-ROADMAP.md`

</details>

### 📋 v1.2 Hardened Runtime

- [x] **Phase 15: Stability Foundation** — Unit-test seams for DNS proxy and vminitd; console log, version check, unsafe sweep, secret wrapping (completed 2026-07-07)
- [ ] **Phase 16: Daemon Polish** — Reliable `spk down` via PID fallback; first-class `spk restart`; port and exec verified end-to-end
- [ ] **Phase 17: testcontainers Conformance** — SpeckDockerd wired as intercepting layer; SQLite state persistence; Docker API conformance tests pass
- [ ] **Phase 18: Developer ID Distribution** — Developer ID signing + notarytool + `.pkg` + Homebrew Cask

---

## Phase Details

### Phase 15: Stability Foundation

**Goal**: Developer can run isolated unit tests for Speck's DNS proxy and vminitd without network access or root privileges, and `spk up` detects mismatched assets before starting.
**Depends on**: Phase 14 (v1.1 complete)
**Requirements**: TEST-01, TEST-02, TEST-03, TEST-04, TEST-05, TEST-06, TEST-07
**Success Criteria** (what must be TRUE):

  1. `cargo test` in `speck-net` covers NXDOMAIN, SERVFAIL, VPN-scoped resolver, and default path — all pass on a machine with no network access
  2. `cargo test` in `speck-guest` covers mount ordering, chroot/pivot_root, and sysctl sequences — all pass without root or a Linux environment
  3. `spk up` fails with an actionable error message (including a `spk up --pull` hint) when rootfs/initrd version files on disk do not match the host binary constants
  4. `$SPECK_HOME/console.log` is non-empty after `spk up`; the path appears in boot failure error messages
  5. `cargo build` produces zero Rust 2024 `unsafe_op_in_unsafe_fn` warnings; registry auth passwords do not appear in `Debug` output

**Plans**: 8 plans
Plans:

- [x] 15-01-PLAN.md — Verify Cargo package legitimacy before dependency changes
- [x] 15-02-PLAN.md — Add DNS Resolver seam for offline DNS tests
- [x] 15-03-PLAN.md — Add asset version guard and console log path verification
- [x] 15-04-PLAN.md — Add vminitd Syscalls seam and mount extraction
- [x] 15-05-PLAN.md — Add offline DNS proxy path tests
- [x] 15-06-PLAN.md — Add mock-based vminitd mount/sysctl tests
- [x] 15-07-PLAN.md — Wrap registry auth password in SecretString
- [x] 15-08-PLAN.md — Replace unsafe test env mutation with temp-env

### Phase 16: Daemon Polish

**Goal**: Developer can reliably stop, restart, verify published ports, and exec into containers regardless of how the daemon was started.
**Depends on**: Phase 15
**Requirements**: DAEMON-01, DAEMON-02, DAEMON-03, DAEMON-04
**Success Criteria** (what must be TRUE):

  1. `spk down` terminates the daemon via PID file fallback when the daemon was started with `cargo run` (not via launchd)
  2. `spk restart` completes the stop + start sequence and returns the daemon to ready state without manual intervention; Docker API returns `503 Retry-After` during the restart window
  3. `curl localhost:<host_port>` returns HTTP 200 from a published container port on the macOS host
  4. `spk exec <container> echo hello` prints `hello` to stdout; `inspect_exec` returns exit code 0

**Plans**: 5 plans
Plans:

- [x] 16-01-PLAN.md — Add VmState::Restarting model variant
- [x] 16-02-PLAN.md — Harden PID file ESRCH handling + status path fix
- [x] 16-03-PLAN.md — spk restart CLI command (stop+start subprocess + PREPARE_RESTART signal)
- [ ] 16-04-PLAN.md — Port publish + exec E2E test expansions
- [ ] 16-05-PLAN.md — 503 Retry-After middleware + PREPARE_RESTART daemon handler

### Phase 17: testcontainers Conformance

**Goal**: testcontainers-rs can start containers, map ports, run log-wait strategies, and survive daemon restarts against the Speck Docker API.
**Depends on**: Phase 16
**Requirements**: CONF-01, CONF-02, CONF-03, CONF-04, CONF-05, CONF-06, CONF-07
**Success Criteria** (what must be TRUE):

   1. All three bollard-based conformance tests in `api_conformance.rs` (lines 63, 93, 119) pass — no TODO stubs remain
   2. Named volumes and networks survive a `spk down && spk up` cycle — `docker-compose up/down` completes without 404 errors
   3. `WaitFor::message_on_stdout` log-wait strategies work — `GET /containers/{id}/logs` returns Docker 8-byte frame multiplexed format
   4. testcontainers-rs port mapping resolves correctly — `inspect_container` returns `NetworkSettings.Ports` with string `HostPort` and `"0.0.0.0"` `HostIp`
   5. On daemon startup, no exec entries remain with `running=true` — all are reset to `exit_code=-1`

**Plans**: 6 plans
Plans:

- [x] 17-01-PLAN.md — SQLite-backed storage module with schema, CRUD, exec reconciliation
- [x] 17-02-PLAN.md — Persist volume/network/exec handlers through storage
- [x] 17-03-PLAN.md — Persist port bindings; inject into container inspect response
- [x] 17-04-PLAN.md — Real container log output through encode_frame multiplexing
- [x] 17-05-PLAN.md — Wire SpeckDockerd as production layer; fix image_push + network validation
- [ ] 17-06-PLAN.md — Enable three bollard conformance tests

### Phase 18: Developer ID Distribution

**Goal**: Users can install `spk` via Homebrew Cask and pass Gatekeeper on a clean macOS system with no prior developer setup.
**Depends on**: Phase 15 (stable binary); DIST track is otherwise independent of Phases 16–17
**Requirements**: DIST-01, DIST-02, DIST-03, DIST-04
**Success Criteria** (what must be TRUE):

  1. `cargo xtask dist` produces a Developer ID signed binary — `codesign -d --entitlements` confirms `com.apple.security.virtualization` is present as `<true/>` (boolean, not string)
  2. `cargo xtask dist` notarization step parses JSON output and fails the build if `.status != "Accepted"`; the stapled binary passes `stapler validate`
  3. `.pkg` installer installs `spk` to `/usr/local/bin` and `spk --version` succeeds after install
  4. `brew install --cask speck` on a clean macOS machine completes without a Gatekeeper quarantine dialog; the entitlement survives Homebrew re-signing

**Plans**: TBD

---

## Progress

| Phase | Milestone | Plans Complete | Status | Completed |
|-------|-----------|----------------|--------|-----------|
| 01. Foundation | v1.0 | — | Complete | — |
| 02. FFI/Bridge + VM Boot | v1.0 | — | Complete | — |
| 03. Vsock Echo | v1.0 | 4/4 | Complete | — |
| 04. Guest Networking | v1.0 | 6/6 | Complete | — |
| 05. containerd + BuildKit | v1.0 | 5/5 | Complete | — |
| 06. Docker API Compat | v1.0 | 11/11 | Complete | — |
| 06.1. Fix 5 Integration Blockers | v1.0 | 5/5 | Complete | — |
| 07. Gap Closure | v1.1 | 5/5 | Complete | 2026-07-02 |
| 08. Config File & VM Resources | v1.1 | 3/3 | Complete | 2026-07-03 |
| 09. Daemon Lifecycle | v1.1 | 3/3 | Complete | 2026-07-03 |
| 10. Shell Integration | v1.1 | 2/2 | Complete | 2026-07-04 |
| 11. VPN-Proof DNS | v1.1 | 5/5 | Complete | 2026-07-04 |
| 12. Corporate CA Injection | v1.1 | 3/3 | Complete | 2026-07-05 |
| 13. Diagnostics | v1.1 | 2/2 | Complete | 2026-07-05 |
| 14. Homebrew Distribution | v1.1 | 3/3 | Complete | 2026-07-06 |
| 15. Stability Foundation | v1.2 | 8/8 | Complete   | 2026-07-07 |
| 16. Daemon Polish | v1.2 | 3/5 | In Progress|  |
| 17. testcontainers Conformance | v1.2 | 5/6 | In Progress|  |
| 18. Developer ID Distribution | v1.2 | 0/TBD | Not started | — |
