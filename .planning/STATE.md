# State

**Current phase:** 04 — Guest Networking (✅ Complete)
**Status:** Complete (6/6 plans)
**Previous phase:** 03 — Vsock Echo (✅ Complete)

## Phase 04 Summary

Phase 04 (Guest Networking) is complete across 3 waves and 6 plans:

- **04-01** — `speck-net` crate scaffold + shared `NetworkConfig` type
- **04-02** — `FdDevice` (smoltcp Device trait) + `SmoltcpInterface` + `SpeckNet::spawn()` poll loop
- **04-03** — VM network device wiring (`VZFileHandleNetworkDeviceAttachment`) + vsock DNS port config
- **04-04** — TCP re-origination bridge, DHCP server, vsock DNS proxy, host MTU detection
- **04-05** — Integration tests for netstack (ARP, DNS proxy, FD lifecycle) — `#[ignore]`d
- **04-06** — Guest-side DNS forwarder in vminitd (AF_VSOCK → UDP:53)

All crates compile cleanly: `speck-net`, `speck-core`, `speck-vz` (with tests), `speck-cli`, and `speck-guest` (cross-compiled).

## Plan 03-04 Complete

- Integration test `test_vsock_echo` + `test_vsock_connect_refused` written and compiles cleanly
- `#[derive(Debug)]` added to `VzSocket` for test assertions
- Guest binary `vminitd` built for `aarch64-unknown-linux-musl` (static ELF)
- Initrd built at `/tmp/speck-initrd.cpio.gz` (186KB, vminitd as `/init`)
- Cross-compilation fixed: `build.rustc` in `.cargo/config.toml` to resolve Homebrew/Rustup toolchain conflict
- Test execution blocked on `com.apple.security.virtualization` entitlement — deferred to later development
- `cargo check -p speck-vz --tests` passes ✅

## Plan 03-03 Complete

- `speck-guest` crate with vminitd binary (static musl, `aarch64-unknown-linux-musl`)
- `vsock_echo::serve()` with AF_VSOCK socket, bind to VMADDR_CID_ANY, listen, accept, echo loop
- Kernel cmdline parser for `vsock_port=PORT` (default 1234)
- `libc` dependency for raw socket syscalls
- `#[cfg(target_os = "linux")]` gated module for cross-compile safety
- `cargo check --target aarch64-unknown-linux-musl --lib` passes ✅
- `cargo check --target aarch64-unknown-linux-musl --bin vminitd` passes ✅
- Commits: `a2d8823`, `27d0ba9`

## Plan 03-02 Complete

- `VZVirtioSocketDeviceConfiguration` wired into VM config in `do_start`
- `VZSocketDevice` extracted from `vm.socketDevices()` after VM start, stored via `VmSocketDevice` Send wrapper
- `VmCommand::VsockConnect{port, reply}` IPC variant + `do_vsock_connect` handler
- `do_vsock_connect` calls `connectToPort_completionHandler` with `StackBlock`, `dup()`s fd, wraps in `VzSocket`
- `Guest::vsock_connect(port)` public API delegating to `VmThread::vsock_connect(port)`
- `cargo check -p speck-vz` passes ✅
- Commit: `2f0cd5e`

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

## Phase 4 Context Gathered

- Phase 4 Guest Networking context captured via discuss-phase
- Decisions: smoltcp Rust-only netstack, 172.16.0.0/24 DHCP, vsock-based DNS proxy, MSS clamping, new speck-net crate
- Context: `.planning/phases/04-guest-networking/04-CONTEXT.md`
