---
gsd_state_version: 1.0
milestone: v1.0
milestone_name: milestone
current_phase: 05
status: executing
last_updated: "2026-06-26T15:45:00.000Z"
progress:
  total_phases: 6
  completed_phases: 4
  total_plans: 21
  completed_plans: 17
  percent: 81
---

# State

**Current phase:** 05
**Status:** Executing Phase 05
**Previous phase:** 04 — Guest Networking (✅ Complete)

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

## Plan 05-01 Complete

- GuestConfig extended with 5 new optional fields: rootfs_disk_path, data_disk_path, containerd_vsock_port, buildkitd_vsock_port, ready_vsock_port
- GuestConfigBuilder has setter methods for all 5 new fields
- GuestConfig::validate() rejects set-but-missing rootfs_disk_path and data_disk_path
- Error enum has DiskAttachment(String) and GuestReadyTimeout variants
- objc2-virtualization features include VZStorageDeviceConfiguration, VZVirtioBlockDeviceConfiguration, VZStorageDeviceAttachment, VZDiskImageStorageDeviceAttachment
- Commits: `6a55189`, `a074ed9`, `b1b3200`

## Phase 5 Planned

- Phase 5 containerd + BuildKit Integration context captured via discuss-phase
- 5 plans across 3 waves planned:
  - **Wave 1** (parallel): ~~05-01 GuestConfig fields~~ ✅, 05-02 vminitd supervision, 05-03 rootfs CI + fetch
  - **Wave 2**: 05-04 Host-side disk attach + WaitForGuestReady
  - **Wave 3**: 05-05 Guest API + integration tests
- Success criteria: containerd reachable via gRPC/vsock + alpine image pull succeeds
- Plans: `.planning/phases/05-containerd-buildkit-integration/05-0[1-5]-PLAN.md`
- Context: `.planning/phases/05-containerd-buildkit-integration/05-CONTEXT.md`
