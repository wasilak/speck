# Plan 04-02 Summary: FdDevice + SmoltcpInterface + SpeckNet::spawn()

**Phase:** 04 — Guest Networking
**Status:** ✅ Complete
**Date:** 2026-06-26

## What was built

- **device.rs:** `FdDevice` wrapping a `RawFd` implements `smoltcp::phy::Device` — reads/writes raw L2 Ethernet frames from a socketpair fd. Sets `O_NONBLOCK` via `fcntl` in constructor, closes fd on `Drop`. Uses `FdRxToken`/`FdTxToken` implementing `RxToken`/`TxToken` traits. `receive()` returns `None` on `EAGAIN` (non-blocking). DeviceCapabilities: Medium::Ethernet, configurable MTU, max_burst_size=1.

- **interface.rs:** `SmoltcpInterface` owns `FdDevice` + smoltcp `Interface` + `SocketSet`. Constructor creates Interface with MAC from `NetworkConfig.mac`, IP from `NetworkConfig.guest_ip` with `NetworkConfig.subnet_prefix`. `poll()` returns `true` if socket state changed. Four accessor methods (`iface()`, `iface_mut()`, `sockets()`, `sockets_mut()`) for future re-origination/DNS modules.

- **lib.rs (speck-net):** `SpeckNet` now holds `Option<RawFd> + NetworkConfig`. `spawn()` accepts a socketpair fd, dups it (one copy for `AsyncFd` readiness notification, one for owned `FdDevice`), and spawns a tokio task running a `clear_ready()` → `poll()` loop.

## Files

### Created
- `crates/speck-net/src/device.rs` — 106 lines, FdDevice with smoltcp Device trait
- `crates/speck-net/src/interface.rs` — 82 lines, SmoltcpInterface with Interface + SocketSet

### Modified
- `crates/speck-net/src/lib.rs` — SpeckNet placeholder → full struct with spawn()
- `crates/speck-net/Cargo.toml` — Added tokio features `rt`, `net`

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] smoltcp 0.13.1 API differences from plan**
- **Found during:** Task 2 (interface.rs compilation)
- **Issue:** Plan assumed smoltcp `Interface` was generic over device type. In 0.13.1, `Interface` is not generic — `new()` takes `&mut (impl Device + ?Sized)` (borrows for initialization), `poll()` takes device + sockets as separate params. `PollResult` is an enum (`None`/`SocketStateChanged`), not a struct.
- **Fix:** Restructured `SmoltcpInterface` to own `device` as a separate field; adjusted `new()` to pass `&mut device`; `poll()` uses `matches!` to check for `SocketStateChanged`.
- **Files modified:** crates/speck-net/src/interface.rs
- **Verified:** cargo check passes

**2. [Rule 3 - Blocking] tokio::io::unix::AsyncFd needs "net" feature**
- **Found during:** Task 2 (lib.rs compilation)
- **Issue:** `tokio::io::unix` module requires `cfg(all(unix, feature = "net"))`. Missing from Cargo.toml.
- **Fix:** Added `"net"` to tokio features. Also added `"rt"` for `tokio::spawn`.
- **Files modified:** crates/speck-net/Cargo.toml
- **Verified:** cargo check passes

**3. [Rule 3 - Blocking] AsyncFdReadyGuard uses `clear_ready()` not `clear()`**
- **Found during:** Task 2 (lib.rs compilation)
- **Issue:** Tokio's `AsyncFdReadyGuard` method is `clear_ready()`, not `clear()`.
- **Fix:** Changed `guard.clear()` → `guard.clear_ready()`.
- **Files modified:** crates/speck-net/src/lib.rs
- **Verified:** cargo check passes

**4. [Rule 2 - Missing Critical] FdRxToken/FdTxToken need pub(crate) visibility**
- **Found during:** Task 2 (device.rs compilation after module wiring)
- **Issue:** Private types leaked through pub(crate) `Device` trait impl on `pub(crate)` struct.
- **Fix:** Changed `struct FdRxToken`/`struct FdTxToken` → `pub(crate) struct`.
- **Files modified:** crates/speck-net/src/device.rs
- **Verified:** cargo check passes

**5. [Rule 2 - Missing Critical] AsyncFdReadyGuard must-use warning**
- **Found during:** Task 2 (clippy check)
- **Issue:** `async_fd.readable().await?` returns a `#[must_use] AsyncFdReadyGuard` that must be consumed.
- **Fix:** Bind guard with `let mut guard` and call `guard.clear_ready()`.
- **Files modified:** crates/speck-net/src/lib.rs
- **Verified:** cargo check passes with zero warnings

---

**Total deviations:** 5 auto-fixed (2 Rule 2, 3 Rule 3)
**Impact on plan:** All auto-fixes necessary for correctness. No scope creep.

## Verification

- `cargo check -p speck-net` ✅ — zero errors, zero warnings
- `cargo test -p speck-net` ✅ — unit tests pass (no test failures)
- `cargo clippy --no-deps -p speck-net` — pre-existing tokio crate rustc-version errors only (unrelated environment issue)

## Next Phase Readiness

- Core netstack device and interface layer complete
- Ready for TCP re-origination bridge (Plan 04-03/04-04)
- `SmoltcpInterface` accessors available for socket management
