# State

**Current phase:** 03 — Vsock Echo
**Status:** Executing (1/4 plans complete)
**Previous phase:** 02 — FFI/Bridge + Bare VM Boot (✅ Complete)

## Plan 03-01 Complete

- Feature flags: VZSocketDevice*, VZVirtioSocketDevice*, VZVirtioSocketConnection
- `libc` dependency added
- Error variants: `VsockConnect`, `VsockTimeout`, `VsockIo`
- New `vsock` module with `VzSocket` wrapper (read/write/drop via libc)
- `GuestConfig.vsock_port` field (default 1234) with builder method
- `cargo check -p speck-vz` passes ✅
- Commit: `1fe2a66`

## Phase 2 Summary

- Kernel boot via `VZLinuxBootLoader` with Kata 3.32.0 kernel
- VM boots to `Running` in ~120ms via `test_guest_boots_to_running`
- 10x stress test (`ten_start_stop_cycles`) passes in ~1.76s
- `VmThread` pattern with serial dispatch queue
- `VmDelegate` ObjC class for stop/error callbacks
- `do_stop` with 10s timeout (recv_timeout on mpsc channel)
- `Drop` impls for clean teardown
- Ad-hoc codesigning via `speck.entitlements`
- `scripts/fetch-kernel.sh` downloads Kata kernel + initrd to `$SPECK_HOME`
- `cargo xtask ci` passes (fmt, clippy, no-print, workspace tests, doctest)
