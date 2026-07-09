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

## Milestone: v1.2 — Hardened Runtime

**Shipped:** 2026-07-09
**Phases:** 7 | **Plans:** 32 | **Timeline:** 3 days (2026-07-07 → 2026-07-09)

### What Was Built

- Unit tests for DNS proxy (NXDOMAIN, SERVFAIL, VPN-scoped paths) and vminitd mount/sysctl orchestration — all pass without network or root
- Serial console capture and guest version mismatch detection at startup
- Unsafe `set_var`/`remove_var` sweep replaced with temp-env crate
- Docker API conformance: 9/9 tests passing (pull → create → start → exit → logs)
- SQLite-backed state persistence for containers, execs, networks, volumes
- Port publishing end-to-end through smoltcp stack
- `spk exec` through vsock to containerd
- Reliable `spk down` outside launchd (PID fallback + ESRCH detection)
- First-class `spk restart` (stop+start subprocess with 503 Retry-After middleware)
- Ad-hoc signed development release pipeline (pkg, tar, metadata)
- Control socket reliability fixes (accept loop resilience, VmState lease timeout)
- Container stdout/stderr log relay over vsock with live follow-mode streaming

### What Worked

- **Inserted phases (decimal numbering):** 17.1, 18.1, 19 were added mid-milestone without renumbering. The decimal convention made it clear they were gap closures
- **Gap-closure pattern:** Phase 17's verification revealed 3 blockers. Rather than reopen Phase 17, inserted phases (17.1, 19) handled them in order — clean separation, clear ownership
- **Conformance test suite:** bollard-based Docker API conformance tests provided real feedback. Passing them was a genuine product milestone
- **Goal-backward verification:** The verification pass on Phase 17 found gaps the original plans missed (HostIp default, network existence validation) — caught before they shipped

### What Was Inefficient

- **Multi-plan wave dependencies:** Phase 17's Wave 2/3 blocking on Wave 1 created idle time. The plans were large enough that splitting into smaller vertical slices would have reduced wait
- **Mid-milestone ROADMAP.md drift:** Phase 19 was marked "in progress" when the ROADMAP was updated but Phase 17.1 wasn't flagged as complete — required reconciliation at milestone close
- **Verification docs not re-run:** Phase 17 gaps were closed by 17.1 and 19, but VERIFICATION.md was never updated — led to open-artifact audit warnings at milestone close

### Patterns Established

- **Log relay pattern:** vsock-based FIFO relay with offset-aware reads. Host pins two FIFOs per container (stdout/stderr), guest relay thread reads and serves via simple READ:stdout/READ:stderr protocol
- **Initrd rebuild loop:** `cargo xtask build-in-guest` → rebuild initrd → scp into VM → daemon restart. Fast iteration without full VM boot cycle
- **Decimal phase insertion:** `N.N` numbering for urgent gap closures without reindexing existing phases

### Key Lessons

- Transfer service image pull (containerd v1.7) doesn't set `containerd.io/snapshot/overlayfs.key` label — committed snapshots must be discovered by listing + parent-chain analysis instead
- Snapshot key discovery by parent-chain (find the committed snapshot no other committed snapshot references as parent) is more reliable than filtering by label
- HostIp normalization needs to handle both `null` and `""` — different Docker client versions produce different representations
- Docker's `follow=true` log streaming cannot be implemented as a finite snapshot — requires mpsc channel + offset-based relay reads that stay open until container exit or client disconnect

### Cost Observations

- Sessions: ~6 sessions across 3 days
- Model mix: balanced profile
- Notable: Gap-closure phases were the most efficient (tightly scoped, clear goal)
- Phase 17 was the largest (10 plans) — could benefit from being split into smaller vertical slices next time

## Cross-Milestone Trends

### Phase/Plan Velocity

| Milestone | Phases | Plans | Duration | Plans/Day |
|-----------|--------|-------|----------|-----------|
| v1.1 | 8 | 26 | 5 days | 5.2 |
| v1.2 | 7 | 32 | 3 days | 10.7 |

### Efficiency Gain

v1.2 achieved double the plans/day (10.7 vs 5.2) compared to v1.1. Likely factors:
- Less platform/architecture work (networking, daemon, CA injection in v1.1)
- More focused work on dockerd layer (single crate)
- Gap-closure phases were tightly scoped and efficient
- Pattern reuse from v1.1 (wave-based execution, source-assertion tests)
