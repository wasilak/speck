# ROADMAP

---

## Milestones

- ✅ **v1.0 Foundation** — Phases 1–6 (shipped prior)
- ✅ **v1.1 Production Runtime** — Phases 07–14 (shipped 2026-07-06)
- 📋 **v1.2 Hardened Runtime** — Phases 15–18.1 (in progress, Phase 17.1 inserted)

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
- [x] **Phase 16: Daemon Polish** — Reliable `spk down` via PID fallback; first-class `spk restart`; port and exec verified end-to-end (completed 2026-07-08)
- [x] **Phase 17: testcontainers Conformance** — SpeckDockerd wired as intercepting layer; SQLite state persistence; Docker API conformance tests pass; gap closure closed (completed 2026-07-08)
- [x] **Phase 17.1: Close testcontainers conformance gaps (INSERTED)** — follow-stream log relay, HostIp default, network existence validation (2/4 plans executed; 2 gap-closure plans pending after verification gaps — see 17.1-VERIFICATION.md) (completed 2026-07-09)
- [x] **Phase 18: Developer ID Distribution** — Developer ID signing + notarytool + `.pkg` + Homebrew Cask (completed 2026-07-08)
- [x] **Phase 18.1: Close gap: daemon reliability (INSERTED)** — Control socket recovery, VmState lease timeout, control.sock cleanup (completed 2026-07-08)
- [ ] **Phase 19: Close testcontainers conformance gaps (INSERTED)** — Fix `follow=true` live log stream, default HostIp to `"0.0.0.0"`, add network existence validation (in progress)

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
- [x] 16-04-PLAN.md — Port publish + exec E2E test expansions
- [x] 16-05-PLAN.md — 503 Retry-After middleware + PREPARE_RESTART daemon handler

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

**Plans**: 10 plans
Plans:
**Wave 1**

- [x] 17-01-PLAN.md — SQLite-backed storage module with schema, CRUD, exec reconciliation
- [x] 17-02-PLAN.md — Persist volume/network/exec handlers through storage

**Wave 2** *(blocked on Wave 1 completion)*

- [x] 17-03-PLAN.md — Persist port bindings; inject into container inspect response
- [x] 17-04-PLAN.md — Real container log output through encode_frame multiplexing
- [x] 17-05-PLAN.md — Wire SpeckDockerd as production layer; fix image_push + network validation

**Wave 3** *(blocked on Wave 2 completion)*

- [x] 17-06-PLAN.md — Enable three bollard conformance tests
- [x] 17-07-PLAN.md — Gap closure: real stdout/stderr log relay for Docker logs

**Gap closure (re-verification)** *(Wave 1 plans parallel; Wave 2 blocked on both)*

- [x] 17-08-PLAN.md — Gap closure: live follow=true log stream + relay lifecycle cleanup (CONF-05)
- [x] 17-09-PLAN.md — Gap closure: network existence validation in connect/disconnect (CONF-03)
- [x] 17-10-PLAN.md — Gap closure: HostIp "0.0.0.0" default + conformance test coverage (CONF-06, CONF-07)

### Phase 17.1: Close testcontainers conformance gaps (follow-stream, HostIp default, network validation) (INSERTED)

**Goal:** `logs --follow` yields a live streaming response that delivers bytes appended after the client subscribed; omitted `HostIp` in port bindings defaults to `"0.0.0.0"` instead of `null`; all remaining conformance integration tests pass and verify the three gaps.
**Requirements**: CONF-05, CONF-06, CONF-03, CONF-07
**Depends on:** Phase 17
**Plans:** 4/4 plans complete

**Wave 1**

- [x] 17.1-01-PLAN.md — Follow-stream live log relay + HostIp default (cherry-pick from orphaned branch + working tree)

**Wave 2** *(blocked on Wave 1 — same file conflict on containers.rs)*

- [x] 17.1-02-PLAN.md — Conformance integration tests + SPECK_SOCK fix + daemon/socket smoke verification

**Wave 3** *(17.1-03 blocked on 17.1-01 and 17.1-02 — it reuses the committed conformance tests/socket alignment and restores the guest/runtime path before any final suite claims)*

- [x] 17.1-03-PLAN.md — Restore guest containerd 9001 path, BusyBox blank-disk detection, and current-initrd rebuild/install workflow

**Wave 4** *(blocked on 17.1-03 — closes empty-string HostIp + bounded/chunked log-read defects and owns the only rebuilt-initrd full-suite gate)*

- [x] 17.1-04-PLAN.md — Fix empty-string HostIp + bounded/chunked log relay behavior and rerun full conformance suite

### Phase 18: Ad-hoc Development Distribution

**Goal**: Maintainers can produce honest ad-hoc signed, non-notarized development artifacts without Apple Developer Program credentials, while preserving exact virtualization entitlement validation for a future Developer ID lane.
**Depends on**: Phase 15 (stable binary); DIST track is otherwise independent of Phases 16–17
**Requirements**: DIST-01, DIST-02, DIST-03, DIST-04
**Success Criteria** (what must be TRUE):

  1. `cargo xtask dist-check` passes without Developer ID identities or notary credentials, and still validates `speck.entitlements` exactly.
  2. `cargo xtask dist --sign-only` ad-hoc signs the Apple Silicon release binary and confirms `com.apple.security.virtualization` is boolean true.
  3. `cargo xtask dist` produces clearly named non-notarized development package/archive artifacts and a Formula-compatible binary archive.
  4. Release/Homebrew metadata is explicit that this is a development-only route; Developer ID signing, notarization, stapling, and official Cask publication are deferred.

**Plans**: 3 plans

Plans:

- [x] 18-01-PLAN.md — Extract `xtask` dist/preflight helpers and remove the Developer ID/notary credential gate
- [x] 18-02-PLAN.md — Implement ad-hoc signed binary, development package, and non-notarized archives
- [x] 18-03-PLAN.md — Publish development release artifacts and preserve honest Homebrew development routes

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
| 16. Daemon Polish | v1.2 | 5/5 | Complete | 2026-07-08 |
| 17. testcontainers Conformance | v1.2 | 10/10 | Complete | 2026-07-08 |
| 17.1 Close testcontainers gaps | v1.2 | 4/4 | Complete   | 2026-07-09 |
| 18. Ad-hoc Development Distribution | v1.2 | 3/3 | Complete    | 2026-07-08 |
| 18.1 Close Gap: Daemon Reliability | v1.2 | 1/1 | Complete   | 2026-07-08 |
| 19. Close testcontainers conformance gaps | v1.2 | 0/TBD | In progress | - |

### Phase 18.1: Close gap: daemon stop/restart control socket reliability (INSERTED)

**Goal:** The daemon's control socket survives transient accept errors; VmState::Restarting auto-recovers after 60s; control.sock is cleaned up on graceful shutdown.
**Requirements**: RELIABILITY-01, RELIABILITY-02, RELIABILITY-03, RELIABILITY-04
**Depends on:** Phase 18
**Plans:** 1/1 plans complete
Plans:

- [x] 18.1-01-PLAN.md — Fix accept loop (continue+backoff), add VmState lease timeout, clean up control.sock, add source-inspection tests (completed 2026-07-08)

### Phase 19: Close testcontainers conformance gaps (INSERTED)

**Goal:** Fix the three remaining testcontainers conformance gaps: `logs --follow` delivers live streaming output (not snapshot+EOF), omitted `HostIp` defaults to `"0.0.0.0"`, and network connect/disconnect validates target network existence.
**Depends on:** Phase 17, Phase 17.1
**Requirements**: CONF-05, CONF-06, CONF-03
**Plans:** 1 plan

Plans:
- [ ] 19-01-PLAN.md — Fix Runtime.Name, rebuild initrd, run conformance suite end-to-end
