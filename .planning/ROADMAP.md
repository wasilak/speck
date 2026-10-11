# ROADMAP

---

## Milestones

- ✅ **v1.0 Foundation** — Phases 1–6 (shipped prior)
- ✅ **v1.1 Production Runtime** — Phases 07–14 (shipped 2026-07-06)
- ✅ **v1.2 Hardened Runtime** — Phases 15–19 (shipped 2026-07-09)
- ✅ **v1.3 Transparent Proxy Restoration** — Phases 20–25 (shipped 2026-07-17)
- 🚧 **v1.4 Daily Driver** — Phases 26–30 (in progress)

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

<details>
<summary>✅ v1.3 Transparent Proxy Restoration (Phases 20–25) — SHIPPED 2026-07-17</summary>

- [x] Phase 20: Transparent Proxy Cutover (3/3 plans) — completed 2026-07-13
- [x] Phase 21: Create Interception Middleware (3/3 plans) — completed 2026-07-14
- [x] Phase 22: Response Rewriting & Restart Gate (2/2 plans) — completed 2026-07-14
- [x] Phase 23: SpeckDockerd Retirement (1/1 plan) — completed 2026-07-15
- [x] Phase 24: Conformance Exit Gate (1/1 plan) — completed 2026-07-15
- [x] Phase 25: Correct Guest Boot — MS_MOVE switch_root (3/3 plans) — completed 2026-07-17

Full details: `.planning/milestones/v1.3-ROADMAP.md`

</details>

### 🚧 v1.4 Daily Driver (In Progress)

**Milestone Goal:** Speck replaces Colima on the owner's corporate Mac (Cloudflare WARP + TLS-inspecting proxy + split-DNS). Exit gate: 0 manual DNS/proxy resets over 5 working days.

**Sequencing rationale:** Phase 0 hygiene + live baseline first (executable `scripts/egress-smoke.sh`, corporate-Mac M0 checklist). Egress before TLS (TLS tests need all-port egress), resilience after the data path works, dogfood last.

- [ ] **Phase 26: General Egress** - all-port TCP + UDP re-origination, socket sweep, RST on connect failure, event-driven poll
- [ ] **Phase 27: Corporate TLS & Proxy** - CA injection fix, Keychain export, CA into containers, explicit proxy/PAC if M0 requires it
- [ ] **Phase 28: Resilience** - daemon exits on VM death, netstack supervision, sleep/wake clock resync, full DNS record types
- [ ] **Phase 29: Operational Polish** - lock-poison safety, bounded forwards, `::1` publish, dockerd backoff, sysctl, doctor checks
- [ ] **Phase 30: Dogfood Gate** - one week on the corporate Mac as the only runtime

## Phase Details

Phase details for completed milestones are archived to `.planning/milestones/`.

### Phase 26: General Egress

**Goal**: Any TCP/UDP flow a container opens is re-originated from the host network stack, so ssh, databases, search clusters and custom ports work exactly as from macOS itself
**Depends on**: v1.3 (Phase 0 hygiene + egress baseline done 2026-10-11)
**Requirements**: EGRESS-01..06, OPS-04
**Success Criteria** (what must be TRUE):

  1. `scripts/egress-smoke.sh` TCP section passes for ports 80, 443, 22, 5432, 9200, 8443 and an arbitrary high port
  2. `scripts/egress-smoke.sh` UDP section passes (NTP query to a public server)
  3. SocketSet size stays flat after N connect/close cycles (unit test) — no smoltcp socket leak
  4. Connecting to a closed host port fails fast in the container with "connection refused"
  5. `ping 192.0.2.1` from a container no longer gets a fake reply
  6. `scripts/conformance-smoke.sh` 26/26 and Invariant #4/#5 regression tests still pass
  7. A published initrd release matches HEAD; a clean `spk up` on another Mac boots it (OPS-04)

**Plans**: TBD

### Phase 27: Corporate TLS & Proxy

**Goal**: Containers and builds work behind the corporate TLS-inspecting proxy without image changes or manual cert copying
**Depends on**: Phase 26, M0 baseline from the corporate Mac
**Requirements**: TLS-01..04
**Success Criteria** (what must be TRUE):

  1. A CA listed in `config.yaml` is present in the guest bundle after boot (no `--fresh` wipe)
  2. Keychain-trusted corporate root is exported to the guest automatically
  3. `docker run alpine apk add curl` and `curl https://<internal-host>` succeed behind TLS inspection
  4. If M0 shows an explicit proxy/PAC: `docker pull`, `docker build` and `spk up --pull` succeed through it

**Plans**: TBD

### Phase 28: Resilience

**Goal**: Network changes, sleep/wake and VM failure heal themselves — no manual reset ever needed
**Depends on**: Phase 26
**Requirements**: RES-01..04
**Success Criteria** (what must be TRUE):

  1. Killing the VM makes the daemon exit and launchd brings Speck back
  2. An unexpected netstack exit is restarted automatically
  3. After 10 min sleep → wake, the guest clock is within 2 s of the host and container HTTPS works
  4. DNS SRV/TXT/MX lookups from a container return real answers; concurrent slow lookups do not block each other

**Plans**: TBD

### Phase 29: Operational Polish

**Goal**: Close the remaining review findings that can bite during long-running daily use
**Depends on**: Phase 28
**Requirements**: OPS-01..03
**Success Criteria** (what must be TRUE):

  1. No `.expect("lock poisoned")` remains in `speck-dockerd`; forward concurrency is bounded
  2. `curl http://[::1]:<published>` works
  3. `spk doctor` flags a stale host binary vs guest assets, a missing guest CA and guest clock skew

**Plans**: TBD

### Phase 30: Dogfood Gate

**Goal**: Prove the Core Value on the real target network
**Depends on**: Phases 26–29
**Requirements**: DOG-01
**Success Criteria** (what must be TRUE):

  1. Corporate Mac runs Speck as the only runtime (`docker context use speck`, Colima stopped) for one working week
  2. Incident log shows 0 manual DNS/proxy resets over 5 working days
  3. Manual checklists M1 (workloads) and M2 (WARP flap, Wi-Fi switch, sleep/wake, reboot) pass

**Plans**: TBD

---

## Progress

**Execution Order:** Phases execute in numeric order: 26 → 27 → 28 → 29 → 30

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
| 25. Correct Guest Boot | v1.3 | 3/3 | Complete | 2026-07-17 |
| 26. General Egress | v1.4 | 0/? | Not started | — |
| 27. Corporate TLS & Proxy | v1.4 | 0/? | Not started | — |
| 28. Resilience | v1.4 | 0/? | Not started | — |
| 29. Operational Polish | v1.4 | 0/? | Not started | — |
| 30. Dogfood Gate | v1.4 | 0/? | Not started | — |
