# Handover: Next Agent — Fix All Remaining Issues

**Written:** 2026-07-06
**Context:** Post v1.1 release, working on the "B-list" backlog

---

## Current State

**Branch:** `main` (3 files uncommitted, see below)
**Tests:** `cargo test --workspace` — 246 passed, 34 ignored
**Cargo:** `cargo check` — clean across workspace

### Uncommitted changes (working tree)

| File | What changed | 
|------|-------------|
| `crates/speck-cli/src/commands/up.rs` | B3: fetch_initrd/fetch_rootfs .tmp download pattern + cleanup guards on all error paths |
| `crates/speck-vz/src/error.rs` | B6: GuestReadyTimeout carries last vsock error (u32 → (u32, String)) |
| `crates/speck-vz/src/vm_thread.rs` | B6: per-attempt vsock logging with tracing::warn!, last error surfaced in timeout |

**These are safe to commit as-is.** All tests pass.

---

## What Was Done This Session

### ✅ B2: `speck up --dry-run` system validation

- `--dry-run` flag added to `SpkUp` (newtype wrapping `UpCommand`)
- `validate_system()` — checks `com.apple.security.virtualization` entitlement via `codesign -d --entitlements -` + `plutil -extract`
- `validate_binary()` — checks Developer ID + notarization via `codesign -v --strict` + `spctl --assess`
- Validation failures = warnings (stderr), not hard blocks — dev workflow stays unbroken

**Key files:** `up.rs` lines ~62-115, ~310-340

### ✅ B3: Fetch-with-rename + cleanup guards

- `fetch_initrd()` now downloads to `.tmp`, renames on success; stale `.tmp` removed at start
- `fetch_rootfs()`: cleanup on every error path — curl fail removes `.gz.tmp`, gunzip fail removes both `.tmp` and `.gz.tmp`, checksum mismatch removes `.tmp`

### ✅ B6: Per-attempt vsock diagnostics

- `do_wait_for_ready()` logs each attempt with `tracing::warn!` (attempt number, port, error detail)
- `GuestReadyTimeout(u32)` → `GuestReadyTimeout(u32, String)` carrying last error or "all attempts timed out"
- Error message: `"guest ready signal timed out on vsock port {0} (last error: {1})"`

---

## Remaining Issues (for Next Agent)

### 🔴 Currently Broken / Untested

| # | Issue | Location | Impact | Details |
|---|-------|----------|--------|---------|
| B1 | `spk down` unreliable outside launchd | `commands/down.rs` | Medium — dev friction | Daemon doesn't stop cleanly when run outside launchd. Mentioned in RETROSPECTIVE.md. Must kill PIDs manually. |
| B4 | Image store not implemented | TBD | Medium — missing capability | containerd already stores layers in guest; host-side image store (caching, listing `docker images`-style, pruning) not done |
| B5 | Guest version check absent | TBD | Low | No mechanism to verify guest rootfs/initrd versions match host expectations |
| B7 | Serial/console.log not surfaced in GuestReadyTimeout | `vm_thread.rs`/`error.rs` | Low — UX gap | Error says "check console.log" but doesn't actually extract/log its content because serial capture is not implemented yet |
| B11 | No download progress bars for initrd | `up.rs` | Low — UX gap | `fetch_initrd()` uses bare curl without `--progress-bar`; `fetch_rootfs()` has `--progress-bar` |
| B12 | SHA-256 uses external `shasum`, not Rust | `up.rs` | Low — portability | `fetch_rootfs()` shells out to `shasum -a 256`; should use `sha2` or `ring` crate |

### 🟡 Minor / Cosmetic

| # | Issue | Location | Details |
|---|-------|----------|---------|
| — | `integration_05.rs`/`_06.rs`/`_07.rs` are `#[ignore]`'d | `crates/speck-vz/tests/` | Require codesigned binary to run; no CI runner can exercise them. Consider smoke tests that skip vsock. |
| — | `README.md` outdated (install uses `cargo install` now) | `README.md` | Says "clone and build"; Formula exists but no `brew install` path published |
| — | Project still marked as v1.1 complete in STATE.md | `.planning/STATE.md` | Need to start v1.2 milestone planning |

### 🟢 Already Fixed (this session, not committed)

| Item | Status | Note |
|------|--------|------|
| `fetch_initrd` → `.tmp` + rename on success | ✅ Uncommitted | curl target changed from `dest` to `tmp` |
| `fetch_rootfs` cleanup guards | ✅ Uncommitted | Removes temp files on each error path |
| vsock per-attempt tracing::warn! | ✅ Uncommitted | Logs attempt + port + error |
| `GuestReadyTimeout` carries error detail | ✅ Uncommitted | `(u32, String)` instead of `(u32)` |

### 🟢 Already Fixed (previous commit cc38823)

| Item | Status | Note |
|------|--------|------|
| `SPECK_HOME` default → `~/.speck` | ✅ Committed | Both `main.rs` and `speck-vz` |
| Daemon `RunAtLoad=true` | ✅ Committed | launchd actually starts the VM now |
| Asset override: skip fetch if all `--kernel/--initrd/--rootfs/--data-disk` given | ✅ Committed | Faster dev iteration |
| `spk status` command | ✅ Committed | Shows daemon PID + resources |
| `spk logs` with `--tail`/`--follow` | ✅ Committed | Streams daemon stdout/stderr |
| `spk doctor` SKIP → INFO label | ✅ Committed | Cert check returns PassWithDetail for no-certs |
| GuestReadyTimeout console.log hint message | ✅ Committed | Part of B7 — message surfaces path to log |

---

## Architecture Map for Debugging

### Asset download flow

```
main() → run_up()
  → fetch_kata_assets(speck_home)
    → fetch_initrd()     [downloads initrd/initrd.cpio.gz] — CRATES.io release
    → fetch_rootfs()     [downloads rootfs.img.gz]          — GitHub release
    → ensure_data_disk() [creates blank data.img]
```

### VM startup sequence

```
run_up()
  → Guest::new(config)                        [build VZVirtualMachineConfiguration]
  → spawn_blocking { guest.start() }          [VZVirtualMachine::start on dispatch queue]
  → spawn_blocking { guest.wait_for_ready() } [vsock connect loop on port 9000]
  → guest.set_port_map_channel()
  → guest.netstack_fd()
  → guest.connect_dns_vsock(53)
  → SpeckNet::new(...).spawn()
  → dockerd client connect
```

### Vsock port map

| Port | Purpose | Owner |
|------|---------|-------|
| 9000 | READY signal | vminitd → host |
| 9001 | containerd gRPC | containerd → host (via vsock proxy) |
| 9002 | buildkitd gRPC | buildkitd → host (via vsock proxy) |

### `do_wait_for_ready` current params

```
READY_MAX_ATTEMPTS = 300     (was 30 — fixed in debug session)
READY_ATTEMPT_TIMEOUT = 500ms
Total max wait: 300 × 500ms = 150 seconds
Sleep between connect attempts: 200ms
```

---

## Debug Context: GuestReadyTimeout Root Cause

The **previous known blocker** (resolved 2026-06-29) was two bugs:
1. **vminitd/mount_disks** — checked `ENODEV` only for format-retry; unformatted ext4 disks return `EINVAL`. Fix: added `libc::EINVAL` check.
2. **do_wait_for_ready** — 30 attempts × 200ms = 6s max; guest needed >10s. Fix: increased to 300 attempts (150s with 500ms attempt timeout).

Full debug session: `.planning/debug/guest-ready-signal-timed-out.md`

---

## Quick Start for Next Agent

```bash
# Build + test
cargo check --target aarch64-apple-darwin
cargo test --workspace

# Codesign for local run
cargo build -p speck-cli --target aarch64-apple-darwin
cargo xtask codesign-dev
./target/aarch64-apple-darwin/debug/spk --help

# Run in foreground (no daemon)
./target/aarch64-apple-darwin/debug/spk up --foreground

# Check current state
./target/aarch64-apple-darwin/debug/spk doctor
./target/aarch64-apple-darwin/debug/spk status
```
