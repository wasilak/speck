# Speck Retrospective

---

## Milestone: v1.1 — Production Runtime

**Shipped:** 2026-07-06
**Phases:** 8 | **Plans:** 26 | **Timeline:** 5 days (2026-07-01 → 2026-07-06)

### What Was Built

- End-to-end container flow with working network egress, Unix socket, port bindings
- Background daemon via launchd LaunchAgent re-exec (no unsafe fork)
- Config file with env > CLI > file > default precedence; VM resource validation
- `spk env` + idempotent `spk init` for bash/zsh/fish shell integration
- VPN-proof DNS: VM-internal proxy, SCDynamicStore live reload, split-DNS, NXDOMAIN→SERVFAIL
- Corporate CA injection into guest OS trust bundle + containerd hosts.toml
- `spk doctor` 8-check health suite + `spk doctor dns` hostname trace
- Homebrew Formula (ad-hoc signed tarball) distribution

### What Worked

- **Wave-based execution:** Breaking each phase into explicit waves (Wave 1/2/3) with stated blockers kept parallelism clear and prevented ordering bugs from creeping in
- **TDD for correctness-critical code:** Phases 11 (DNS) and 12 (CA certs) started with RED baseline tests — caught issues before runtime testing was possible
- **Source-assertion tests (include_str! pattern):** Phases 09 and 13 used compile-time string assertions to verify structural invariants without mocking the actual system
- **SUMMARY.md per plan:** Compact, structured summaries made it trivial to reconstruct what each plan did and why during milestone close
- **Deviations documented inline:** Phase 14's ad-hoc signing pivot was captured in the SUMMARY immediately, so no context was lost when milestone close came

### What Was Inefficient

- **REQUIREMENTS.md traceability not maintained:** The traceability table "Pending" status on completed phases meant extra work at milestone close to audit and check off 21 requirements manually. A lightweight "mark complete when phase ships" protocol would save this
- **`spk down` reliability:** The stop command doesn't reliably halt processes started outside of launchd (direct `cargo run` sessions). This created friction during testing — had to kill PIDs manually. Should be fixed before v1.2
- **Artifact audit false positive:** The gsd-tools audit picked up CONTEXT.md resolved questions because it scans for `?` in text. Required a formatting workaround (removing question marks from section headers) rather than a semantic fix
- **Phase 14 ROADMAP.md not updated:** Plans were completed but ROADMAP.md still showed Phase 14 as 0/3. Manual audit caught it, but status tracking should update immediately on plan completion

### Patterns Established

- **launchd re-exec idiom:** `SPECK_DAEMONIZED=1` env var as re-exec loop guard; parent writes plist + bootstraps; child runs `--foreground`. Reusable for any macOS daemon written in Rust
- **SCDynamicStoreSetDispatchQueue pattern:** Always use GCD dispatch queue (not CFRunLoop) for network change notifications. CFRunLoop silently no-ops if not on the main thread
- **CA injection order:** `mount_ca_certs()` → `install_ca_certs()` → spawn containerd. Injection must happen before containerd reads TLS config at startup

### Key Lessons

- Architectural guarantees (DNS-01: never bind host :53) are worth documenting even when live verification isn't possible — the code review evidence was sufficient for milestone close
- `spk doctor` DNS trace chose `getaddrinfo` directly over a container-based nslookup pipeline. This is simpler, equally correct, and doesn't require a running VM. The original plan overspecified the implementation
- Phase 09 daemon lifecycle is the riskiest platform-specific phase: the `daemonize` crate prohibition (fork after Mach port init is UB) needed to be stated explicitly in STATE.md or it will be rediscovered in every session

### Cost Observations

- Sessions: ~8 sessions across 5 days
- Notable: TDD phases (11, 12) were slower per plan but caught bugs before runtime — net positive
- Deferred items (3) are genuine external blockers (Apple Developer Program, VPN machine) — not unfinished work

---

## Cross-Milestone Trends

*(Populated when v1.2 closes)*
