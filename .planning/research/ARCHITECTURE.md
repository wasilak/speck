# Architecture Integration Analysis: Speck v1.2

**Domain:** macOS-native container runtime — hardened runtime milestone integration points
**Researched:** 2026-07-06
**Confidence:** HIGH (derived from direct source inspection of current codebase, not from training data)

---

## Critical Finding: Docker API Serving Path

Before addressing any v1.2 questions, a prerequisite finding must be established because it changes the scope of STATE-01/02.

The **current serving path** for Docker API requests is a raw-bytes bridge, not the axum server:

```
docker client
  → speck.sock (Unix socket, $SPECK_HOME)
  → guest.docker_api_unix_proxy()  ← raw byte bridge OS thread
  → vsock port 9003 in guest
  → vminitd sock_forwarder
  → /var/run/docker.sock (moby/dockerd inside guest)
```

`SpeckDockerd::start()` (the axum server in `crates/speck-dockerd/src/lib.rs`) is **never called** from `run_up()`. Despite `speck-cli` depending on `speck-dockerd` in `Cargo.toml`, no `speck_dockerd::` item is imported in `up.rs` or any other CLI command file.

The `AppState` with its `network_store`, `volume_store`, and `exec_store` HashMap fields is **live infrastructure that is not currently receiving requests**. All Docker API traffic goes through moby, which persists its own state to `data.img` (the virtio-blk data disk).

This finding has direct implications for STATE-01/02 scoping (see Question 1 below).

---

## Question 1: Dockerd State Persistence — Where Should It Live?

### Current State

`crates/speck-dockerd/src/state.rs` defines `AppState`:

```rust
pub struct AppState {
    pub guest: Arc<speck_vz::Guest>,
    pub exec_store: Arc<tokio::sync::Mutex<ExecStore>>,       // LRU-capped HashMap
    pub network_store: Arc<tokio::sync::Mutex<HashMap<String, NetworkSummary>>>,
    pub volume_store: Arc<tokio::sync::Mutex<HashMap<String, VolumeSummary>>>,
    ...
}
```

These are in-memory only, initialized fresh on every `SpeckDockerd::start()`. However, `SpeckDockerd::start()` is not currently called from `run_up()`.

### Two Interpretations of STATE-01/02

**Interpretation A — Wire SpeckDockerd as the interception layer:**
STATE-01/02 means wiring `SpeckDockerd` into `run_up()` as the serving layer (replacing or wrapping `guest.docker_api_unix_proxy()`), then adding persistence so its `AppState` survives restarts. This is the larger change.

**Interpretation B — Moby already persists; conformance gaps are the real issue:**
Since moby persists containers/networks/volumes to `data.img`, container and volume state already survives VM restarts. STATE-01/02 is about ensuring the `SpeckDockerd` axum handlers (when they are eventually wired) have persistence, and that the conformance tests don't fail because of stale in-memory state. The testcontainers conformance failures are more likely about stub endpoints than missing persistence.

### Recommendation

**Keep persistence inside `speck-dockerd` as a new `persistence.rs` file. Do not put it in `speck-core`.**

Rationale:
- `speck-core` is a zero-deps, zero-I/O types crate. Adding file I/O violates its only invariant.
- The stores (`ExecStore`, `NetworkSummary`, `VolumeSummary`) are `serde`-derived types and `serde_json` is already a `speck-dockerd` dependency.
- Persistence is a Docker API shim concern — it belongs with the shim.

### Proposed Abstraction

```
crates/speck-dockerd/src/persistence.rs   ← NEW FILE

pub struct StateStore {
    path: PathBuf,   // $SPECK_HOME/dockerd-state.json
}

impl StateStore {
    pub fn load(path: PathBuf) -> Result<StateSnapshot>
    pub fn save(&self, snapshot: &StateSnapshot) -> Result<()>
}

pub struct StateSnapshot {
    pub networks: HashMap<String, NetworkSummary>,
    pub volumes: HashMap<String, VolumeSummary>,
    // exec_store: ephemeral, NOT persisted (exec instances are short-lived)
}
```

`SpeckDockerd::start()` should accept a `speck_home: PathBuf` parameter. On construction, `AppState::new()` loads from disk via `StateStore::load()`. On each write mutation (create/delete network or volume), the handler calls `state.persistence.save()`. Exec state is explicitly not persisted.

**Container state** should be re-synced from containerd gRPC on startup via `AppState::containerd_client()`, not stored in a local file. The ground truth for container metadata is in-guest containerd.

### Integration Points

| File | Change | Type |
|------|--------|------|
| `crates/speck-dockerd/src/persistence.rs` | New file — `StateStore`, `StateSnapshot` | New |
| `crates/speck-dockerd/src/state.rs` | Add `persistence: StateStore` field; load in `AppState::new()` | Modified |
| `crates/speck-dockerd/src/lib.rs` | `SpeckDockerd::start()` gains `speck_home: PathBuf` parameter | Modified |
| `crates/speck-dockerd/src/handlers/networks.rs` | Call `state.persistence.save()` on create/delete | Modified |
| `crates/speck-dockerd/src/handlers/volumes.rs` | Call `state.persistence.save()` on create/delete | Modified |
| `crates/speck-cli/src/commands/up.rs` | Wire `SpeckDockerd::start()` or keep raw proxy | Modified if Interpretation A |

---

## Question 2: Serial Console Capture Integration

### Finding: CONSOLE-01 Is Already Implemented

The `vm_thread.rs::do_start()` function (lines ~479-513) already wires the console device:

```rust
// ── Console device (serial log for kernel + vminitd debug) ────
let console_log_path = config.speck_home.join("console.log");
let virtio_console = VZVirtioConsoleDeviceConfiguration::init(...);
let port0 = VZVirtioConsolePortConfiguration::init(...);
port0.setIsConsole(true);
let attachment = VZFileSerialPortAttachment::initWithURL_append_error(
    ..., &console_log_url, false, // truncate on each VM start
)?;
port0.setAttachment(Some(attachment_ref));
vm_config.setConsoleDevices(&console_array);
```

The `VZVirtioConsoleDeviceConfiguration`, `VZVirtioConsolePortConfiguration`, and `VZFileSerialPortAttachment` types are imported at the top of `vm_thread.rs`.

The CONCERNS.md document states "vm_thread.rs and config.rs contain no references to VZVirtioConsoleDeviceConfiguration" — this was accurate when the document was generated but the implementation has been added since. PROJECT.md still shows `[ ] CONSOLE-01` which needs updating.

### Correct Integration Location

The console device is correctly placed in `vm_thread.rs::do_start()`, inside the `unsafe { ... }` block that assembles the `VZVirtualMachineConfiguration`. This is the right location because:

1. All VZ framework object creation happens here — console is a VM device alongside network, storage, and vsock
2. `console_log_path` derives from `config.speck_home` — no new GuestConfig field needed
3. `config.rs` (GuestConfig) correctly carries `speck_home: PathBuf` already

**No changes to `config.rs` are needed** — the console path is derived at boot time from `speck_home`.

### Remaining Work for CONSOLE-01

The implementation is functionally present. What may remain:
- Kernel cmdline uses `console=hvc0` — verify `VZVirtioConsoleDevice` maps to `hvc0` in the guest (it should — this is the standard VirtioConsole driver device name for Linux, used by Apple's own `container` framework)
- Integration test: after `spk up`, verify `$SPECK_HOME/console.log` is non-empty
- Update PROJECT.md to mark CONSOLE-01 as `[x]`

### Integration Points

| File | Change | Type |
|------|--------|------|
| `crates/speck-vz/src/vm_thread.rs` | Already implemented (lines 479-513) | Verify only |
| `crates/speck-vz/src/config.rs` | No change needed | None |
| `crates/speck-cli/src/commands/up.rs` | `console_log_hint` already references `console.log` (line ~590) | Already present |
| `.planning/PROJECT.md` | Mark CONSOLE-01 `[x]` | Documentation |

---

## Question 3: `spk restart` — What State Needs Graceful Stop vs. Reuse?

### State Inventory in `run_up()`

After `run_up()` has fully started, the following live objects exist in the daemon process:

**Must be stopped/cleaned on restart:**

| Object | Location in run_up() | How to stop |
|--------|---------------------|-------------|
| `VZVirtualMachine` (in VmThread) | `let guest = ...` | `guest.stop()` → `VmCommand::Stop` → GCD queue |
| `SpeckNet` tokio tasks | `netstack_handles` JoinHandles | Drop handles + let tasks detect channel close |
| `unix_vsock_proxy` OS threads | `guest.docker_api_unix_proxy()` | `speck.sock` removal triggers accept error → thread exits |
| `control.sock` UnixListener | `listener` in tokio spawn | `remove_file(ctrl_sock_path)` + listener drops |
| PID file | `run/speck.pid` | `remove_file(pid_path)` |

`shutdown_gracefully()` already handles all of these (stops containers, calls `guest.stop()`, removes `speck.sock`, removes `speck.pid`).

**Can be reused on restart:**

| Object | Why reusable |
|--------|-------------|
| tokio runtime | Lives outside `run_up()`, managed by `#[tokio::main]` |
| `$SPECK_HOME` directory structure | Persists on disk |
| `rootfs.img`, `data.img` | Disk images; no cleanup needed |
| `tracing-subscriber` | Installed once at process start in `main.rs` |

**VmThread specifically:** After `guest.stop()`, the VmThread OS thread is still alive and waiting for commands. Its state is `InternalState::Stopped`. A new `VmCommand::Start` can be sent to the same thread. This means the VM can be restarted within the same process without spawning a new VmThread.

### Recommended `spk restart` Implementation

`spk restart` should be implemented as a new CLI command that operates on the running daemon from the **outside** (like `spk down`), not as an internal signal handler:

```
spk restart
  1. Verify daemon is alive (connect to control.sock — same as run_down() step 1)
  2. Run spk down logic (launchctl bootout OR kill via PID file)
  3. Wait for control.sock to disappear (daemon fully stopped)
  4. Re-register and start: launchctl bootstrap with same plist + wait for ready
     OR: call run_up() directly if --foreground
```

This is safe because:
- Avoids in-process state mutation during restart (no signal races)
- Reuses `run_down()` and `daemonize()` which are already tested
- The launchd `KeepAlive: true` in the plist means after bootout + bootstrap the daemon auto-restarts — `spk restart` just orchestrates the timing

**New files:**
- `crates/speck-cli/src/commands/restart.rs` — `run_restart(speck_home: &Path, args: &UpArgs)` that calls `run_down()` then `daemonize()` or `run_up()`
- Register `Commands::Restart(RestartArgs)` in `main.rs`

### Integration Points

| File | Change | Type |
|------|--------|------|
| `crates/speck-cli/src/commands/restart.rs` | New — sequences run_down + daemonize | New |
| `crates/speck-cli/src/main.rs` | Add `Commands::Restart(RestartArgs)` variant | Modified |
| `crates/speck-cli/src/commands/mod.rs` | Add `pub mod restart;` | Modified |
| `crates/speck-cli/src/commands/down.rs` | No change — reuse `run_down()` | None |
| `crates/speck-cli/src/commands/up.rs` | No change — reuse `daemonize()` + `run_up()` | None |

The existing `down.rs` tests contain `assert!(!src.contains("Commands::Restart"))` — this will need to be removed when `Commands::Restart` is added.

---

## Question 4: Guest Version Check — Where Does Validation Belong?

### Current State

`crates/speck-cli/src/commands/up.rs` defines:

```rust
const ROOTFS_VERSION: &str = "0.2.0";
const INITRD_VERSION: &str = "0.1.0";
```

These constants are used only for URL construction in `fetch_rootfs()` and `fetch_initrd()`. There is no check that already-downloaded assets match these versions.

### Correct Location: `up.rs`, Not `GuestConfig`

The version validation belongs in `speck-cli/src/commands/up.rs`, specifically in `ensure_assets()` or immediately before the `GuestConfig::builder()` call.

**Why not in `GuestConfig::validate()`:**
- `GuestConfig::validate()` performs pure structural checks (files exist, CPU ≥ 1, memory ≥ 512 MiB) — no I/O side effects, no string comparisons against remote versions
- Adding version file I/O to a configuration struct violates the "zero side effects in Config" convention
- The version constants live in `up.rs` and are tightly coupled to the download logic there

**Why not in `speck-vz`:**
- `speck-vz` has no knowledge of asset versioning — it receives a `GuestConfig` with paths that already exist
- Version checking is a host bootstrapping concern, not a VM lifecycle concern

### Implementation Pattern

```
$SPECK_HOME/initrd/.version    ← written by fetch_initrd()
$SPECK_HOME/.rootfs-version    ← written by fetch_rootfs()
```

In `ensure_assets()`, before returning:
```rust
fn check_asset_version(path: &Path, expected: &str, label: &str) -> anyhow::Result<()> {
    let actual = std::fs::read_to_string(path).unwrap_or_default();
    if actual.trim() != expected {
        anyhow::bail!(
            "{label} version mismatch: disk has {:?}, host expects {expected:?}. \
             Run `spk up --pull` to update.",
            actual.trim()
        );
    }
    Ok(())
}
```

If the version file is absent (pre-VERSION-01 assets), treat as "unknown version" and warn, not fail — to avoid breaking existing users on first upgrade.

### Integration Points

| File | Change | Type |
|------|--------|------|
| `crates/speck-cli/src/commands/up.rs` | `check_asset_version()` helper + calls in `ensure_assets()` | Modified |
| `crates/speck-vz/src/config.rs` | No change | None |
| `crates/speck-vz/src/guest.rs` | No change | None |

---

## Question 5: Recommended Build Order for the 4 Tracks

### Dependency Graph Between Tracks

```
Track 1 (Stability & Testing)
  │
  ├── UNSAFE-01       no deps
  ├── DNS-TEST-01     no deps (tests existing code)
  ├── VMINIT-TEST-01  no deps (tests existing code)
  ├── VERSION-01      no deps (adds to ensure_assets)
  └── CONSOLE-01      already done — verify only
          │
          │ stable daemon + reliable down needed for:
          ▼
Track 4 (Daemon Polish)
  │
  ├── DAEMON-DOWN fix  blocks DAEMON-RESTART
  ├── PORT-E2E         unblocks testcontainers port tests
  ├── EXEC-E2E         unblocks testcontainers exec tests
  └── DAEMON-RESTART   needs reliable DAEMON-DOWN first
          │
          │ stable daemon + reliable exec/port needed for:
          ▼
Track 2 (testcontainers Conformance)
  │
  ├── STATE-01 + STATE-02  (parallel; independent of each other)
  └── CONFORM-01            needs STATE-01/02 done first
          
Track 3 (Developer ID Distribution)
  │  no code deps on Tracks 1/2/4
  │  practical dep: sign a stable binary (after Track 1 is done)
  └── BREW-DEVID-01 → BREW-DEVID-02 → BREW-DEVID-03  (sequential)
```

### Recommended Phase Order

**Phase A — Stability Foundation (Track 1)**

Rationale: These are pure code quality and test coverage items with no external dependencies. They should come first because:
- DNS-TEST-01 and VMINIT-TEST-01 create regression protection before any v1.2 changes are made
- UNSAFE-01 prevents subtle UB from affecting later work
- CONSOLE-01 is verify-only (already done)

Order within phase:
1. UNSAFE-01 — correctness fix; should be in place before any new code is written
2. DNS-TEST-01 — protects core VPN value proposition
3. VMINIT-TEST-01 — protects guest init; mount failures are the hardest bugs to debug
4. VERSION-01 — simple addition to `up.rs::ensure_assets()`
5. CONSOLE-01 — verify and close ticket

**Phase B — Daemon Polish (Track 4)**

Rationale: These fix functional gaps that testcontainers exercises. PORT-E2E and EXEC-E2E failures would cause CONFORM-01 tests to fail non-obviously. DAEMON-DOWN must come before DAEMON-RESTART.

Order within phase:
1. DAEMON-DOWN (fix PID file fallback) — prerequisite for all other daemon operations
2. PORT-E2E — run the port publishing end-to-end path manually + add a test
3. EXEC-E2E — run exec end-to-end + add a test
4. DAEMON-RESTART — new `restart.rs` command, thin wrapper on run_down + daemonize

**Phase C — testcontainers Conformance (Track 2)**

Rationale: Needs stable daemon (Phase B) and cannot be started until STATE-01/02 are resolved.

Order within phase:
1. STATE-01 + STATE-02 (can be done in parallel — both add to `speck-dockerd/src/persistence.rs`)
2. CONFORM-01 — fill Docker API conformance TODOs; depends on STATE to ensure networks/volumes survive between test steps

**Phase D — Developer ID Distribution (Track 3)**

Rationale: No blocking code dependencies on Tracks 1/2/4. The only practical dependency is having a stable binary worth signing. Can start external prerequisites (Apple Developer enrollment, CI runner setup) immediately in parallel with Phase A.

Order within phase:
1. BREW-DEVID-01 — Developer ID signing + notarytool workflow
2. BREW-DEVID-02 — `.pkg` installer
3. BREW-DEVID-03 — Homebrew Cask (requires notarized binary from BREW-DEVID-01)

---

## Integration Summary Table

| v1.2 Item | Primary Crate(s) | New File? | New API? | Gate |
|-----------|-----------------|-----------|----------|------|
| CONSOLE-01 | `speck-vz` | No | No | Already done — verify only |
| VERSION-01 | `speck-cli` | No | No | Add to `up.rs::ensure_assets()` |
| UNSAFE-01 | `speck-cli` | No | No | Sweep `init.rs`, `shell.rs` |
| DNS-TEST-01 | `speck-net` | Yes (test file) | No | Pure tests of `dns.rs` |
| VMINIT-TEST-01 | `speck-guest` | Yes (test file) | No | Pure tests of `vminitd.rs` functions |
| STATE-01/02 | `speck-dockerd` | Yes (`persistence.rs`) | `SpeckDockerd::start()` sig change | Depends on SpeckDockerd wiring decision |
| CONFORM-01 | `speck-dockerd` | No | No | Fill existing TODOs in `api_conformance.rs` |
| DAEMON-DOWN | `speck-cli` | No | No | Fix `kill_via_pid_file()` in `down.rs` |
| DAEMON-RESTART | `speck-cli` | Yes (`restart.rs`) | `Commands::Restart` | Needs reliable DAEMON-DOWN |
| PORT-E2E | `speck-net`, `speck-cli` | No | No | Manual verify + test |
| EXEC-E2E | `speck-dockerd`, `speck-cli` | No | No | Manual verify + test |
| BREW-DEVID-01/02/03 | `xtask`, `Formula/` | Yes (Cask file) | No | External: Apple Developer enrollment |

---

## Crate Boundaries for v1.2 Changes

Changes are concentrated in two crates. No changes to `speck-core` or `speck-vz` are required for any v1.2 feature except the CONSOLE-01 verification.

```
speck-core     ← NO CHANGES for v1.2
speck-vz       ← CONSOLE-01 verify only (already implemented)
speck-net      ← DNS-TEST-01 tests only (new test file, no production code change)
speck-dockerd  ← STATE-01/02 persistence.rs + CONFORM-01 handler stubs
speck-guest    ← VMINIT-TEST-01 tests only (new test file, no production code change)
speck-cli      ← VERSION-01, UNSAFE-01, DAEMON-*, restart.rs
```

---

## Anti-Patterns to Avoid in v1.2 Work

**State persistence in speck-core:**
`speck-core` has no I/O, no deps beyond std types. Adding a file writer there breaks its single invariant and makes it a cross-cutting concern rather than a types library.

**Rebuilding persistence from scratch with SQLite/sled:**
The network and volume stores are small HashMaps of simple structs. `serde_json` is already a `speck-dockerd` dependency. A single `dockerd-state.json` file written atomically (write-to-tmp, rename) is sufficient and adds zero new dependencies.

**Piggybacking version check on GuestConfig::validate():**
`validate()` is a pure structural check called from tests. Adding filesystem reads that compare version strings breaks the pure-function property and makes tests depend on asset state.

**In-process restart via signal (SIGUSR1):**
The current tokio `select!` loop in `run_up()` only handles SIGINT and SIGTERM. Adding SIGUSR1 handling requires careful channel management. The external process approach (spk restart = spk down + spk up sequencing) is safer and reuses all existing code paths.

**SpeckDockerd wiring without a plan for the raw proxy:**
If SpeckDockerd is wired in as the serving layer, `guest.docker_api_unix_proxy()` must either be replaced or the two paths must not conflict on `speck.sock`. The changeover requires a single clean decision point in `run_up()` — not a partial wiring.

---

## Sources

All findings are from direct source code inspection of the current `speck` repository at `/Users/piotrek/git/speck`, revision corresponding to v1.1 milestone (post-2026-07-06). Files read:

- `.planning/PROJECT.md` — milestone requirements
- `.planning/codebase/ARCHITECTURE.md` — system diagram and crate responsibilities
- `.planning/codebase/STRUCTURE.md` — file layout and entry points
- `.planning/codebase/CONCERNS.md` — known gaps and technical debt
- `crates/speck-dockerd/src/state.rs` — AppState structure
- `crates/speck-dockerd/src/lib.rs` — SpeckDockerd::start() signature
- `crates/speck-vz/src/config.rs` — GuestConfig builder
- `crates/speck-vz/src/vm_thread.rs` — VmThread, VmCommand, do_start() implementation
- `crates/speck-vz/src/guest.rs` — Guest public API
- `crates/speck-cli/src/commands/up.rs` — run_up(), ensure_assets(), shutdown_gracefully()
- `crates/speck-cli/src/commands/down.rs` — run_down(), kill_via_pid_file()
- `crates/speck-cli/src/main.rs` — Commands enum
- `crates/speck-cli/Cargo.toml` — dependency list
- `crates/speck-dockerd/Cargo.toml` — dependency list
- `crates/speck-guest/src/bin/vminitd.rs` — PID 1 guest init structure
- `crates/speck-dockerd/tests/api_conformance.rs` — conformance test structure
