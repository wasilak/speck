---
gsd_state_version: 1.0
milestone: v1.0
milestone_name: milestone
current_phase: 06
status: completed
last_updated: "2026-06-28T18:00:00.000Z"
progress:
  total_phases: 6
  completed_phases: 6
  total_plans: 32
  completed_plans: 32
  percent: 100
---

# State — 🎉 Phase 6 Complete — Milestone v1.0 Complete

**Status:** All 32 plans across 6 phases are complete.

## Phase 06 Complete — Docker API Compat Layer

Phase 06 (Docker API Compat Layer) is complete across 5 waves and 11 plans:

- **06-01** — speck-core types: Container, Image, Volume, Network domain types
- **06-02** — speck-dockerd scaffold: axum server, hyper_util Unix socket + upgrades, stream.rs frame encode/decode, router + handler stubs
- **06-03** — Docker API: system (/_ping, /version, /info) + container lifecycle + exec
- **06-04** — Docker API: attach hijack, logs streaming, image pull/push with registry auth, events SSE, networks + volumes
- **06-05** — Port publishing: PortPublishBridge in speck-net + smoltcp active-connect to guest IP
- **06-06** — VirtioFS volumes: multi-device VZVirtioFileSystemDeviceConfiguration + vminitd auto-mount + Ryuk docker.sock symlink
- **06-07** — BuildKit: vendored proto + tonic codegen + POST /build handler + Guest::buildkitd_unix_proxy()
- **06-08** — CLI: full speck-cli with clap v4, indicatif, anstream theme, DockerClient, all subcommands
- **06-09** — spk dashboard: ratatui TUI with container list + log tail + keyboard navigation
- **06-10** — Codesigning + CI: xtask codesign-dev, release.yml Developer ID + notarytool, Homebrew Formula
- **06-11** — testcontainers conformance: bollard api_conformance.rs + integration_06.rs end-to-end

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

## Plan 05-02 Complete

- New `sock_forwarder.rs` module: generic vsock→Unix socket bidirectional forwarder
  - `pub fn serve(vsock_port, unix_path)` loops accepting vsock connections and proxying each to a Unix socket
  - Private `fn unix_connect(path)` for AF_UNIX SOCK_STREAM connections
  - Private `fn proxy_copy(read_fd, write_fd)` for bidirectional byte copy
  - Per-connection: `dup()` all fds, spawn two threads (one per direction)
- vminitd extended from 51→453 lines with full boot lifecycle:
  - `mount_early_filesystems()` — /proc (procfs), /sys (sysfs), /dev (devtmpfs)
  - `mount_disks()` — /dev/vda→/rootfs (ext4), /dev/vdb→/rootfs/var/lib/containerd (ext4, formats if needed)
  - `spawn_service_with_restart()` for containerd + buildkitd with 1s backoff loop
  - `wait_for_containerd_socket()` — polls /rootfs/run/containerd/containerd.sock (50x200ms)
  - `send_ready_signal()` — vsock listen/accept on ready_port, writes b"READY\n"
  - Two `sock_forwarder::serve()` threads for containerd (port 9001) and buildkitd (port 9002)
  - 3 new cmdline parsers: `containerd_vsock_port`, `buildkitd_vsock_port`, `ready_vsock_port`
  - Old `vsock_echo::serve(port)` call removed; PID 1 now loops on infinite sleep
- `cargo check --target aarch64-unknown-linux-musl -p speck-guest` passes ✅
- `cargo check --target aarch64-unknown-linux-musl --bin vminitd` passes ✅
- Commits: `3864bca`, `6ccbc24`

## Plan 05-03 Complete

- `scripts/fetch-rootfs.sh` — download + SHA256 verify + data.img stub creation
- `xtask/src/main.rs` — `task_init()` extended to call `fetch-rootfs.sh` alongside `fetch-kernel.sh`
- `.github/workflows/build-rootfs.yml` — CI arm64 ext4 image builder with containerd 2.3.2 + runc 1.5.0 + buildkitd 0.31.1
- All verifications pass: bash -n, cargo build, YAML validation
- Commits: `e434612`, `9d510e3`, `7710689`

## Phase 5 Complete

Phase 05 (containerd + BuildKit Integration) is complete across 3 waves and 5 plans:

- **05-01** — GuestConfig: 5 new optional fields + DiskAttachment error variant + objc2-vz features
- **05-02** — vminitd: sock_forwarder, disk mounting, containerd supervision, READY signal
- **05-03** — scripts/fetch-rootfs.sh + xtask init extension + .github/workflows/build-rootfs.yml
- **05-04** — VmThread: disk attachment (VZVirtioBlockDeviceConfiguration) + WaitForGuestReady command + containerd-client dev-dep
- **05-05** — Guest::wait_for_ready() + Guest::containerd_unix_proxy() + bridge_vsock_unix + integration_05.rs (#[ignore]'d RUN-06 tests)

## Plan 05-05 Complete

- Guest::wait_for_ready() delegates to VmThread::wait_for_ready(ready_vsock_port)
- Guest::containerd_unix_proxy() bridges vsock port 9001 to temp Unix socket (PID+port unique path)
- bridge_vsock_unix() copies bytes bidirectionally with dup'd fds for independent ownership
- integration_05.rs: 3 #[ignore]'d tests — test_vm_boots_with_disks, test_containerd_ready, test_image_pull_alpine
- cargo test -p speck-vz --test integration_05 passes with 0 passed, 3 ignored
- Commits: a71d3e7, 65db917

## Decisions

- [Phase ?]: do_wait_for_ready: VsockConnect and VsockTimeout are retriable (ECONNREFUSED expected until vminitd binds port); all other errors propagate immediately
- [Phase ?]: Disk attachment requires both paths set together (rootfs_disk_path + data_disk_path); if either is None, no storage devices are attached

## Performance Metrics

| Phase | Plan | Duration | Notes |
|-------|------|----------|-------|
| Phase 06 P04 | 26 min | 2 tasks | 12 files |
| Phase 06 P05 | 17min | 2 tasks | 7 files |
