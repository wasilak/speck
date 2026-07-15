# ROADMAP

---

## Milestones

- ✅ **v1.0 Foundation** — Phases 1–6 (shipped prior)
- ✅ **v1.1 Production Runtime** — Phases 07–14 (shipped 2026-07-06)
- ✅ **v1.2 Hardened Runtime** — Phases 15–19 (shipped 2026-07-09)
- 🚧 **v1.3 Transparent Proxy Restoration** — Phases 20–24 (in progress)

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

<details>
<summary>✅ v1.2 Hardened Runtime (Phases 15–19) — SHIPPED 2026-07-09</summary>

- [x] **Phase 15: Stability Foundation** (8/8 plans) — completed 2026-07-07
- [x] **Phase 16: Daemon Polish** (5/5 plans) — completed 2026-07-08
- [x] **Phase 17: testcontainers Conformance** (10/10 plans) — completed 2026-07-08
- [x] **Phase 17.1: Close testcontainers conformance gaps (INSERTED)** (4/4 plans) — completed 2026-07-09
- [x] **Phase 18: Ad-hoc Development Distribution** (3/3 plans) — completed 2026-07-08
- [x] **Phase 18.1: Close gap: daemon reliability (INSERTED)** (1/1 plan) — completed 2026-07-08
- [x] **Phase 19: Close testcontainers conformance gaps (INSERTED)** (1/1 plan) — completed 2026-07-09

Full details: `.planning/milestones/v1.2-ROADMAP.md`

</details>

### 🚧 v1.3 Transparent Proxy Restoration (In Progress)

**Milestone Goal:** Implement locked decision D-21 — `speck.sock` becomes a transparent byte proxy to the real dockerd running inside the guest; SpeckDockerd shrinks from a Docker API reimplementation to a thin allowlisted middleware. Exit gate: `scripts/conformance-smoke.sh` 18/18 (baseline 2026-07-11: 8/18).

**Sequencing rationale:** Pure proxy first (cutover), middleware endpoints incrementally (create interception, then response rewriting + restart gate), retirement only after all SpeckDockerd prior art (503 VmState middleware, port-map channel, VirtioFS share config) is adapted into the proxy path, executable gate last.

- [x] **Phase 20: Transparent Proxy Cutover** - `speck.sock` served by the byte proxy to guest dockerd; core Docker lifecycle + hijacked streams answered by real dockerd (completed 2026-07-13)
- [x] **Phase 21: Create Interception Middleware** - HTTP-aware allowlist layer intercepts container create: `-v` binds translated to VirtioFS, `-p` ports registered with the host netstack (completed 2026-07-14)
- [x] **Phase 22: Response Rewriting & Restart Gate** - `HostIp` rewritten to host-reachable values in inspect/port responses; 503/Retry-After restart gate rides in front of the proxy (2/2 plans complete, live verified)
- [x] **Phase 23: SpeckDockerd Retirement** - containerd-based endpoint handlers deleted; guest dockerd is the single source of container/image/volume/network truth
- [x] **Phase 24: Conformance Exit Gate** - conformance-smoke 20/20 + build/push/pull round-trip, intact `spk down/up` launchd cycle

## Phase Details

Phase details for completed milestones are archived to `.planning/milestones/`.

### Phase 20: Transparent Proxy Cutover

**Goal**: `speck.sock` is served by the transparent byte proxy to guest dockerd (`/run/speck/dockerd.sock`) over the hardened vsock bridge — every Docker API request is answered by real dockerd, not reimplemented handlers
**Depends on**: Nothing (v1.2 complete; rides the hardened vsock bridge and the existing `guest::docker_api_unix_proxy()` dead code)
**Requirements**: PROXY-01, PROXY-02, PROXY-03
**Success Criteria** (what must be TRUE):

  1. Passthrough lifecycle behaves per Docker API against real dockerd — conformance-smoke checks `ping`, `pull`, `images list`, `run detached, short name`, `ps shows container`, `ps -a`, `inspect`, `logs`, `stop`, `rm`, `run image CMD fallback`, `wait`, `events`, `volume create/ls/rm`, `network ls` pass
  2. Hijacked/upgraded streams pass through bidirectionally byte-for-byte — conformance-smoke `run foreground w/ output` and `exec` checks pass; `docker exec -it`, `docker logs -f`, and `docker attach` stream live
  3. A source-inspection regression test fails if `speck.sock` is ever wired back to SpeckDockerd endpoint handlers instead of the transparent proxy (D-21 guard, lands with the wiring)
  4. Proxy reads drain eagerly per Architecture Invariant #4 — existing vsock bridge regression tests (incl. `bridge_drains_vsock_with_stalled_downstream`) still pass

**Plans:** 3/3 plans complete

Plans:

- [x] 20-01-PLAN.md — Fix vsock bridge half-close (SHUT_WR) + connector retry in unix_vsock_proxy (Wave 1)
- [x] 20-02-PLAN.md — HTTP-aware passthrough proxy module + upgrade join, tested against in-process fake dockerd (Wave 1)
- [x] 20-03-PLAN.md — Cut speck.sock over to the proxy, drop wire-path version strip, land D-21 guard test (Wave 2)

### Phase 21: Create Interception Middleware

**Goal**: An HTTP-aware interception layer sits in front of the byte proxy for allowlisted requests only — container create is rewritten for host awareness (VirtioFS binds, netstack port registration) while everything else, including hijacked streams, stays raw
**Depends on**: Phase 20
**Requirements**: MW-01, MW-02
**Success Criteria** (what must be TRUE):

  1. `docker run -v <hostdir>:/data alpine cat /data/file` reads live host file content; edits made on the host are visible inside the running container (VirtioFS-backed bind translation)
  2. `docker run -d -p 18099:80 nginx:alpine` answers `curl http://localhost:18099/` — conformance-smoke `port publish` check passes (port-map channel registration with the host netstack)
  3. Published ports are unregistered when the container stops or is removed — no stale host listeners after `docker rm -f`
  4. Interception is allowlist-only: non-create requests and hijacked streams still pass byte-for-byte (`run foreground w/ output` and `exec` checks still pass after the interception layer lands)

**Plans**: 3 plans

Plans:

- [x] 21-01-PLAN.md — Define the runtime bind-root contract and mount the `virtiofs-binds` device for guest-visible bind sources (Wave 1)
- [x] 21-02-PLAN.md — Rewrite `POST /containers/create` bind sources through the proxy and add backend/live bind regressions (Wave 2)
- [x] 21-03-PLAN.md — Harden published-port activation/cleanup and prove stale-listener removal on localhost (Wave 3)

### Phase 22: Response Rewriting & Restart Gate

**Goal**: Inspect/port responses are host-truthful and the daemon restart window degrades gracefully — the remaining two allowlisted middleware behaviors work in front of the proxy
**Depends on**: Phase 21
**Requirements**: MW-03, MW-04
**Success Criteria** (what must be TRUE):

  1. `docker inspect` on a published-port container reports host-reachable `HostIp` values in `NetworkSettings.Ports` — testcontainers-style port discovery works
  2. `docker port <container>` shows host-reachable bindings, not guest-internal addresses
  3. During `spk restart`, Docker API requests receive `503` with `Retry-After` (no connection resets, no hangs on a dead vsock); the same requests succeed once restart completes
  4. The VmState gate rides in front of the proxy: while the VM is restarting, `docker ps` fails fast with 503 rather than stalling

**Plans**: 2 plans

Plans:

- [x] 22-01-PLAN.md — Implement HostIp response rewriting for inspect and list containers endpoints (Wave 1)
- [x] 22-02-PLAN.md — Live verification of HostIp rewriting and restart gate endurance (Wave 2, live verified)

### Phase 23: SpeckDockerd Retirement

**Goal**: The containerd-based Docker endpoint reimplementation is deleted — guest dockerd is the single source of container/image/volume/network truth; the host keeps only the four allowlisted middleware concerns
**Depends on**: Phase 22 (all middleware prior art must be adapted out of SpeckDockerd before deletion)
**Requirements**: PROXY-04
**Success Criteria** (what must be TRUE):

  1. `crates/speck-dockerd` contains no containerd-backed Docker endpoint handlers — remaining code is only bind translation, port tracking, `HostIp` rewriting, and the 503 gate
  2. No duplicate host-side container/exec/network/volume state stores remain; after a daemon restart, `docker ps -a` reflects dockerd truth with no host-side shadow state
  3. Workspace builds and tests pass clean after removal (`cargo build`, `cargo clippy`, `cargo test`) with no orphaned dead code left behind
  4. Migration note is documented: images previously pulled into containerd namespace "speck" are not visible to dockerd's store (documented for users, not migrated)

**Plans**: TBD

### Phase 24: Conformance Exit Gate

**Goal**: The milestone's executable definition of done passes end to end against a live daemon — full smoke gate, intact daemon lifecycle, and the build/push workflow verified
**Depends on**: Phase 23
**Requirements**: GATE-01, GATE-02, GATE-03
**Success Criteria** (what must be TRUE):

  1. `scripts/conformance-smoke.sh` passes 18/18 against a live daemon started with `spk up` (baseline 2026-07-11: 8/18)
  2. `spk down` → `spk up` launchd cycle completes cleanly; `spk status` reports running and `spk doctor` passes its checks after the refactor
  3. `docker build` of a test Dockerfile succeeds and `docker push` + `docker pull` round-trips against a local registry container (de-risks the terraform/ECR primary use case)
  4. `spk restart` still works end-to-end with the 503 gate — full down/up/restart cycle intact

**Plans**: TBD

---

## Progress

**Execution Order:** Phases execute in numeric order: 20 → 21 → 22 → 23 → 24

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
| 15. Stability Foundation | v1.2 | 8/8 | Complete | 2026-07-07 |
| 16. Daemon Polish | v1.2 | 5/5 | Complete | 2026-07-08 |
| 17. testcontainers Conformance | v1.2 | 10/10 | Complete | 2026-07-08 |
| 17.1 Close testcontainers gaps | v1.2 | 4/4 | Complete | 2026-07-09 |
| 18. Ad-hoc Development Distribution | v1.2 | 3/3 | Complete | 2026-07-08 |
| 18.1 Close Gap: Daemon Reliability | v1.2 | 1/1 | Complete | 2026-07-08 |
| 19. Close testcontainers conformance gaps | v1.2 | 1/1 | Complete | 2026-07-09 |
| 20. Transparent Proxy Cutover | v1.3 | 3/3 | Complete   | 2026-07-13 |
| 21. Create Interception Middleware | v1.3 | 3/3 | Complete | 2026-07-14 |
| 22. Response Rewriting & Restart Gate | v1.3 | 2/2 | Complete | 2026-07-14 |
| 23. SpeckDockerd Retirement | v1.3 | 1/1 | Complete | 2026-07-15 |
| 24. Conformance Exit Gate | v1.3 | 1/1 | Complete | 2026-07-15 |
