# Research Summary — Speck v1.2 Hardened Runtime

**Project:** Speck (`spk`)
**Domain:** macOS-native Apple Silicon container runtime — stability hardening milestone
**Researched:** 2026-07-06
**Confidence:** HIGH

---

## Executive Summary

Speck v1.2 "Hardened Runtime" is not a feature milestone — it is a correctness and distribution milestone. The four research tracks converge on a single theme: the v1.1 runtime works but is not yet safe to ship widely, test reliably, or distribute without triggering Gatekeeper. The most impactful work, in order, is: (1) adding a test seam around the DNS proxy (Speck's core value proposition has zero test coverage today), (2) fixing daemon lifecycle reliability (`spk down` can fail when the daemon was started outside launchd), (3) filling the Docker API conformance gaps that prevent testcontainers from working end-to-end, and (4) wiring Developer ID signing and notarization into `xtask` so the binary can be distributed without the "unidentified developer" Gatekeeper dialog.

The architecture research produced a critical finding that changes the scope of STATE-01/02: all Docker API traffic currently bypasses the axum server (`SpeckDockerd`) entirely and goes through a raw vsock byte-bridge to moby/dockerd running inside the guest. This means moby already persists container, image, and volume state to `data.img` across VM restarts. What needs persistence in `speck-dockerd` is only the network and volume metadata that `SpeckDockerd` itself owns (names, IDs, IPAM config) — a small JSON file, not a full SQLite schema. This narrows STATE-01/02 significantly. Also notable: CONSOLE-01 (serial console capture) is already implemented in `vm_thread.rs` — it needs verification and ticket closure, not implementation.

The primary risks for v1.2 are: rusqlite blocking tokio worker threads if called from async handlers (use `tokio-rusqlite` or `deadpool-sqlite`); `notarytool submit` returning exit code 0 on rejection (must parse JSON output); and `spk restart` racing the VM stop completion handler (must await stop reply before issuing start). None of these are blockers if caught early — they are all well-understood failure patterns with clear mitigations.

---

## Key Facts

- **CONSOLE-01 is already done.** `vm_thread.rs` already wires `VZVirtioConsoleDeviceConfiguration` + `VZFileSerialPortAttachment` to `$SPECK_HOME/console.log`. PROJECT.md still shows it unchecked — update and close.
- **`SpeckDockerd` axum server is never called in production.** `run_up()` calls `guest.docker_api_unix_proxy()` (raw byte bridge), not `SpeckDockerd::start()`. The `AppState` stores exist but receive no requests.
- **Moby persists state to `data.img`.** Container, image, and volume state survives VM restarts via moby's own BoltDB. STATE-01/02 scope reduces to persisting `SpeckDockerd`'s network/volume name→ID metadata only.
- **`sled` is a dead end.** Last stable release September 2021; on-disk format changes before 1.0 with no migration path; maintainer explicitly recommends SQLite for reliability.
- **`notarytool submit --wait` exits 0 on rejection.** This is a documented gotcha — CI scripts must parse the JSON `status` field, not check `$?`.
- **Hardened Runtime flag is mandatory for notarization.** `--options runtime` must be on every `codesign` call. Ad-hoc signing today omits it.
- **Virtualization entitlement must be boolean `<true/>`, not string `"true"`.** Wrong type causes silent SIGKILL at runtime after notarization succeeds.
- **`ditto`, not `zip`, for notarizable archives.** `zip` strips extended attributes; Gatekeeper cannot verify the notarization ticket from a `zip`-extracted binary.
- **The existing `down.rs` test contains `assert!(!src.contains("Commands::Restart"))` that will break** when `Commands::Restart` is added. Remove that assertion.
- **`mockall` attribute order matters:** `#[automock]` must appear **before** `#[async_trait]`, not after.
- **Pipe buffer fills during VM boot burst.** The serial console reader thread must be started before `VZVirtualMachine.start()`, not after — otherwise the 10-50 KB boot burst fills the 64 KB pipe buffer and stalls the VM.
- **`kqueue` does not reliably wake on pipe reads on older macOS.** Console log reader must be a `std::thread` with blocking `read()`, not a tokio async task.

---

## Stack Additions

New crates for v1.2 only. Existing v1.1 stack is unchanged.

| Crate | Version | Purpose | Crate | Type |
|-------|---------|---------|-------|------|
| `rusqlite` | 0.40.1 | SQLite state store, bundled SQLite 3.53.2 | `speck-dockerd` | dep |
| `rusqlite_migration` | 2.6.0 | Schema migration via `user_version` pragma | `speck-dockerd` | dep |
| `deadpool-sqlite` | 0.13.0 | Async connection pool over rusqlite for tokio | `speck-dockerd` | dep |
| `secrecy` | 0.10.3 | Zeroize-on-drop wrapper for `RegistryAuth.password` | `speck-core` | dep |
| `mockall` | 0.15.0 | `#[automock]` proc macro for trait-level mocking | `speck-net`, `speck-guest` | dev-dep |
| `async-trait` | 0.1.89 | Async fn in traits, required for `mockall` async mock path | `speck-net` | dep |

**Note on STATE-01/02 stack choice:** The architecture research recommends plain JSON files (`serde_json`, already a dep) with atomic write (write-to-tmp, rename) over SQLite for the network/volume metadata store. The store is small, single-writer, and needs zero query capability. SQLite is overkill; if the scope expands later, adding `rusqlite` at that point is low-cost. `secrecy` is the only new dep that touches `speck-core`.

---

## Feature Table Stakes

### testcontainers Conformance (CONFORM-01)

| Endpoint / Behavior | Minimum Viable |
|---------------------|----------------|
| `GET /containers/{id}/json` port mapping | `NetworkSettings.Ports` with string `HostPort`, `"0.0.0.0"` `HostIp` |
| `GET /containers/{id}/json` state | `State.Running: true` immediately after `start`; `State.ExitCode: 0` |
| Log stream format | Docker multiplexed 8-byte frame header (stream_type, 3 zero bytes, 4-byte BE length) |
| `inspect_network` IPAM | `IPAM.Config[0].Gateway` and `IPAM.Config[0].Subnet` populated |
| `PUT /containers/{id}/archive` | Accept tar body upload, return HTTP 200 |
| `GET /containers/{id}/archive` | Return tar stream |
| `GET /_ping` + `GET /version` | HTTP 200 with `Api-Version` header; `ApiVersion >= "1.40"` |
| Exec hijack | `start_exec` upgrades to TCP stream; `inspect_exec` returns `Running: false` + exit code after completion |

### State Persistence (STATE-01/02)

| State item | Minimum Viable |
|------------|----------------|
| Network metadata | `$SPECK_HOME/state/networks.json` written on create/delete, loaded on daemon start |
| Volume metadata | `$SPECK_HOME/state/volumes.json` written on create/delete, loaded on daemon start |
| Container inspect cache | Rebuilt from containerd gRPC on daemon start (not persisted to disk) |
| Exec state | NOT persisted; all `running=true` entries reset to `exit_code=-1` on startup |

### Developer ID Distribution (BREW-DEVID-01/02/03)

| Step | Minimum Viable |
|------|----------------|
| BREW-DEVID-01 | Sign with Developer ID Application + `--options runtime` + `com.apple.security.virtualization`; notarize; verify `status: Accepted` in JSON output |
| BREW-DEVID-02 | `.pkg` via `pkgbuild` + Developer ID Installer cert; notarize + staple; enables offline Gatekeeper |
| BREW-DEVID-03 | Homebrew Cask pointing at signed `.pkg`; SHA256 from signed artifact; verify entitlement survives Homebrew re-sign |

### Daemon Polish (DAEMON-*)

| Feature | Minimum Viable |
|---------|----------------|
| `spk down` | Works via PID file fallback regardless of how daemon was started |
| `spk restart` | `run_down()` + `daemonize()` sequence; waits for control.sock disappearance before starting |
| PORT-E2E | `curl localhost:<host_port>` returns 200 from macOS host for a published container port |
| EXEC-E2E | `spk exec <container> echo hello` returns output to stdout; `inspect_exec` returns exit code 0 |

### Stability & Testing (Track 1)

| Feature | Minimum Viable |
|---------|----------------|
| DNS-TEST-01 | `DnsBackend` or `Resolver` trait extracted; mockall-powered unit tests cover NXDOMAIN->SERVFAIL, TTL capping, VPN-scoped branch, round-robin selection |
| VMINIT-TEST-01 | `Syscalls` trait extracted; unit tests cover mount ordering, error propagation, sysctl encoding, pivot_root choreography |
| VERSION-01 | `$SPECK_HOME/.rootfs-version` and `$SPECK_HOME/initrd/.version` checked in `ensure_assets()`; mismatch emits actionable error with `spk up --pull` hint |
| CONSOLE-01 | Verify `$SPECK_HOME/console.log` is non-empty after `spk up`; mark ticket closed |
| UNSAFE-01 | `init.rs` and `shell.rs` unsafe blocks audited and documented |

---

## Architecture Integration

### Build Order Recommendation

```
Phase A — Stability Foundation (Track 1)
  UNSAFE-01           speck-cli       no new files, no new API
  DNS-TEST-01         speck-net       new test file, no prod change
  VMINIT-TEST-01      speck-guest     new test file, no prod change
  VERSION-01          speck-cli       add to up.rs::ensure_assets()
  CONSOLE-01          speck-vz        verify + close (already done)

Phase B — Daemon Polish (Track 4)
  DAEMON-DOWN         speck-cli       fix kill_via_pid_file() in down.rs
  PORT-E2E            speck-net       manual verify + regression test
  EXEC-E2E            speck-dockerd   manual verify + regression test
  DAEMON-RESTART      speck-cli       new restart.rs; needs DAEMON-DOWN first

Phase C — testcontainers Conformance (Track 2)
  STATE-01/02         speck-dockerd   new persistence.rs (JSON, not SQLite)
  CONFORM-01          speck-dockerd   fill TODOs in api_conformance.rs

Phase D — Developer ID Distribution (Track 3)
  BREW-DEVID-01       xtask           sign + notarize in xtask dist
  BREW-DEVID-02       xtask           pkgbuild wrapper
  BREW-DEVID-03       Formula/        Homebrew Cask file
```

**Rationale for this order:**

1. **Phase A first** — creates regression protection before any structural changes. DNS-TEST-01 and VMINIT-TEST-01 are pure additions; they cannot break existing functionality. UNSAFE-01 must be audited before new unsafe-adjacent code is written in later phases.

2. **Phase B before C** — daemon must be reliably stoppable (DAEMON-DOWN) before testcontainers conformance tests can be run end-to-end. PORT-E2E and EXEC-E2E failures would cause CONFORM-01 tests to fail non-obviously without a working daemon as a baseline.

3. **Phase C needs Phase B** — STATE-01/02 persistence only matters if the daemon can be restarted reliably. CONFORM-01 needs stable port/exec paths (PORT-E2E, EXEC-E2E) to be meaningful.

4. **Phase D is parallel-capable** — no code dependencies on Phases A/B/C. The only practical gate is having a stable binary worth signing. Apple Developer enrollment can begin immediately. Practically, BREW-DEVID-01 should be done after Phase A stabilizes the binary.

### Crate Change Concentration

```
speck-core      NO CHANGES except secrecy dep for RegistryAuth
speck-vz        CONSOLE-01 verify only (already implemented)
speck-net       DNS-TEST-01: new test file only, no prod code change
speck-dockerd   STATE-01/02 persistence.rs + CONFORM-01 handler stubs
speck-guest     VMINIT-TEST-01: new test file only, no prod code change
speck-cli       VERSION-01, UNSAFE-01, DAEMON-*, restart.rs
xtask           BREW-DEVID-01/02/03 signing pipeline
```

### Scope Ambiguities Needing Product Clarification

1. **SpeckDockerd wiring decision (STATE-01/02 blocker).** The architecture research reveals a fork in the road: (A) wire SpeckDockerd axum server into `run_up()` as the intercepting layer, replacing the raw vsock proxy — larger change, enables proper API conformance control; (B) add persistence to `SpeckDockerd` for when it eventually gets wired, but leave the raw proxy in place for v1.2. The decision changes the STATE-01/02 implementation significantly. **Clarify before Phase C planning.**

2. **`spk restart` scope for v1.2.** The minimum viable implementation is external process sequencing (`down + up`). The architecture supports in-process VM restart (reuse VmThread, send Stop then Start) but requires careful task handle cleanup (pitfall 4-B). Which level of sophistication is required for v1.2?

3. **`spk doctor` integration for console log.** Should `spk doctor` show the last N lines of `console.log` on boot failure? Trivial to add, but worth confirming scope so it does not slip into v1.3.

---

## Watch Out For

Top 5 pitfalls most likely to cause pain if ignored:

**1. `notarytool submit` exits 0 on rejection — CI will ship broken binaries.**
Parse the JSON output. Check `.status == "Accepted"`. A single unchecked `$?` in the release script will ship an unnotarized binary silently. This breaks Homebrew installs for every user after September 2026.
Prevention: `xcrun notarytool submit ... --output-format json | jq -r '.status'`; fail the build if not `"Accepted"`.

**2. rusqlite called directly from async axum handlers blocks the tokio worker pool.**
This looks identical to correct async code at a glance. The failure is not a crash — it is a gradual timeout storm under any non-trivial load. `testcontainers` running parallel container tests will hit this immediately.
Prevention: Use `deadpool-sqlite` (already in the stack recommendation) so all DB calls go through `spawn_blocking` internally. Never hold a rusqlite connection in an async context.

**3. `spk restart` issuing Start before Stop completes races the GCD completion handler.**
`do_stop` is async at the Apple VZ level — `stopWithCompletionHandler` returns before the VM is stopped. Any restart code that fires `VmCommand::Start` without awaiting the `done_rx` reply from the stop path will hit `Error::AlreadyRunning`.
Prevention: Await the stop reply channel explicitly. The current `do_stop` implementation blocks on `done_rx.recv_timeout`; the restart command layer must not bypass this.

**4. `com.apple.security.virtualization` entitlement as string type causes silent SIGKILL.**
`<string>true</string>` instead of `<true/>` passes `codesign -v`, passes notarization, installs successfully, then silently kills the process when it tries to create a `VZVirtualMachine`. There is no error message.
Prevention: Add `codesign -d --entitlements :- ./spk | grep virtualization` as a mandatory `xtask dist` verification step that fails the build if the entitlement is absent or malformed.

**5. DNS proxy test isolation — `getaddrinfo` is not mockable without a trait seam.**
Without the `Resolver` trait extraction, DNS unit tests make live DNS calls. CI in a restricted network environment will produce flaky NXDOMAIN/SERVFAIL results and eventually disable the tests. The DNS proxy is Speck's primary differentiator — it must have deterministic unit tests.
Prevention: Extract `trait DnsBackend` / `trait Resolver` before writing any tests. Use `mockall` on the trait, not on getaddrinfo.

---

## Open Questions

Deduplicated across all 4 research files. Items marked BLOCKING must be resolved before the relevant phase starts.

| # | Question | Relevant Phase | Source |
|---|----------|----------------|--------|
| 1 | **BLOCKING:** Wire SpeckDockerd into `run_up()` (Interpretation A) or add persistence only for future use (Interpretation B)? | Phase C (STATE-01/02) | ARCHITECTURE.md |
| 2 | Does `spk restart` need in-process VM restart (reuse VmThread) or is external process sequencing (`down + up`) sufficient for v1.2 testcontainers workflows? | Phase B (DAEMON-RESTART) | ARCHITECTURE.md, FEATURES.md |
| 3 | Is `spk doctor` integration for `console.log` tail in scope for v1.2? | Phase A (CONSOLE-01) | FEATURES.md |
| 4 | Should CONFORM-01 cover `POST /containers/{id}/wait?condition=not-running` or is that v1.3? testcontainers-rs uses polling (`inspect_container`), not event-based wait by default. | Phase C (CONFORM-01) | FEATURES.md |
| 5 | App Store Connect API key for CI notarization — is it available, or should BREW-DEVID-01 use Apple ID + app-specific password for v1.2? | Phase D (BREW-DEVID-01) | FEATURES.md |
| 6 | Does `spk down` need to wait for containers to stop gracefully before sending SIGTERM to the daemon, or is immediate VM poweroff acceptable for v1.2? | Phase B (DAEMON-DOWN) | FEATURES.md |
| 7 | Should the `Syscalls` trait in `vminitd.rs` be `pub(crate)` only, or does it need to be `pub` for integration test access from a separate test binary? | Phase A (VMINIT-TEST-01) | STACK.md |
| 8 | `deadpool-sqlite` vs plain JSON files for STATE-01/02 — given narrow scope (small network/volume metadata), is adding the SQLite dep justified, or does JSON-with-atomic-rename suffice? | Phase C (STATE-01/02) | ARCHITECTURE.md vs STACK.md |

---

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack | HIGH | All versions verified against crates.io on 2026-07-06; version compatibility matrix confirmed |
| Features | HIGH | testcontainers-rs API surface verified against bollard source; notarytool workflow from Apple official docs |
| Architecture | HIGH | Derived from direct source code inspection of current codebase; critical raw-proxy finding verified in `run_up()` |
| Pitfalls | HIGH | Apple Developer Forums confirmed VZ console pitfalls; tokio-rs/axum discussions confirmed SQLite async pitfalls; vm_thread.rs source confirmed restart race patterns |

**Overall confidence: HIGH**

### Gaps to Address

- **CONFORM-01 test harness scope:** The existing `api_conformance.rs` test structure was referenced but not fully read. Before Phase C, audit what stubs exist vs what needs implementation. May reveal additional work items.
- **Homebrew entitlement preservation:** Confirmed via Homebrew docs that `--preserve-metadata=entitlements` is used, but the specific Cask DSL to trigger it was MEDIUM confidence. Verify after creating the first Cask draft.
- **`VZVirtioConsolePortConfiguration` isConsole flag:** Verify `isConsole = true` is set in the existing `vm_thread.rs` CONSOLE-01 implementation — omitting it causes silent log drop (pitfall 5-B).

---

## Sources

### Primary (HIGH confidence)
- crates.io API — stable versions 2026-07-06: rusqlite 0.40.1, rusqlite_migration 2.6.0, deadpool-sqlite 0.13.0, secrecy 0.10.3, mockall 0.15.0, async-trait 0.1.89
- `speck` repository source inspection — AppState, vm_thread.rs, up.rs, down.rs, api_conformance.rs, dns.rs, vminitd.rs
- Apple Developer Documentation — notarizing-macos-software-before-distribution, hardened-runtime, com.apple.security.virtualization entitlement
- testcontainers-rs / bollard source — client.rs API surface, confirmed no Ryuk usage
- Docker Engine API docs — log multiplexing frame format, container inspect fields
- tokio-rs/axum discussions #964, #2629 — rusqlite async pitfalls, mutex-across-await deadlock

### Secondary (MEDIUM confidence)
- github.com/Code-Hex/vz Go mirror — VZVirtioConsoleDeviceConfiguration isConsole semantics
- Apple Developer Forums thread 758354 — console port configuration pitfalls
- rsms/macOS-distribution-gist — ditto vs zip, staple workflow, signing order
- cargo-nextest docs — process-per-test isolation for DNS tests
- DeepWiki apple/containerization VM lifecycle — stop ordering patterns

---
*Research completed: 2026-07-06*
*Ready for roadmap: yes*
