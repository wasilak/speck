---
phase: 04
plan: 03
subsystem: "speck-vz"
tags:
  - networking
  - virtualization
  - vsock-dns
  - file-handle-attachment
requires:
  - "04-01: speck-net crate + NetworkConfig type"
  - "04-02: FdDevice + SmoltcpInterface + SpeckNet::spawn()"
provides:
  - "04-04: Higher-level Guest network integration"
affects:
  - "crates/speck-vz/src/vm_thread.rs"
  - "crates/speck-vz/src/config.rs"
  - "crates/speck-vz/src/error.rs"
  - "crates/speck-vz/Cargo.toml"
tech-stack:
  added:
    - "socket2 0.5 (with all feature for socketpair on macOS)"
  patterns:
    - "VZFileHandleNetworkDeviceAttachment wired via NSFileHandle from dup'd socketpair fd"
    - "Virtio network device configuration with configurable MTU and MAC"
    - "Vsock DNS connection initiated inside VM start completion handler"
key-files:
  created: []
  modified:
    - "crates/speck-vz/Cargo.toml"
    - "crates/speck-vz/src/error.rs"
    - "crates/speck-vz/src/config.rs"
    - "crates/speck-vz/src/vm_thread.rs"
decisions:
  - "Socket pair with Domain::UNIX + Type::DGRAM for datagram-based VZFileHandleNetworkDeviceAttachment"
  - "Host socket set non-blocking for async readiness monitoring"
  - "Vsock DNS connection is best-effort (inside start completion callback, not blocking start)"
  - "Public APIs dup the internal fd on each call so caller owns their copy"
metrics:
  duration: 0h 10min
  completed_date: "2026-06-26"
---

# Phase 04 Plan 03: Wire network device + vsock DNS connection

Wired a `VZVirtioNetworkDeviceConfiguration` with `VZFileHandleNetworkDeviceAttachment` into the VM configuration, and added a best-effort vsock DNS connection initiated when the VM enters the Running state.

## Tasks

### Task 1 — Feature flags, deps, error variants (c05b986)

- Enabled VZFileHandleNetworkDeviceAttachment, VZVirtioNetworkDeviceConfiguration, VZNetworkDeviceAttachment, VZNetworkDeviceConfiguration, VZMACAddress features
- Added NSFileHandle to objc2-foundation features
- Added socket2 (0.5) and speck-net path dependencies
- Added `Network(String)` and `NetworkIo(io::Error)` error variants

### Task 2 — Network fields in GuestConfig (716ec89)

- Added `network: Option<NetworkConfig>` and `dns_vsock_port: Option<u32>` fields
- Added builder methods `.network()` and `.dns_vsock_port()`
- Added MTU >= 1500 validation in `GuestConfig::validate()`

### Task 3 — Wire VZFileHandleNetworkDeviceAttachment in do_start (647af44)

- Created `VmControl.netstack_fd` field and `VmCommand::NetstackFd` variant
- In `do_start()`: created a UNIX DGRAM socketpair, dup'd the vm-facing fd into an `NSFileHandle`, constructed `VZFileHandleNetworkDeviceAttachment`, set MTU when > 1500, configured `VZVirtioNetworkDeviceConfiguration` with MAC address from config, added to `setNetworkDevices`
- Stored host-side dup'd fd in `VmControl.netstack_fd`
- Added `VmThread::netstack_fd()` public API with dup-on-retrieve semantics

### Task 4 — Vsock DNS connection (D-07) (647af44, same commit)

- Added `VmControl.dns_vsock_fd` field and `VmCommand::DnsVsockFd` variant
- Inside the VM start completion handler (after state = Running), when `dns_vsock_port` is `Some`, connects to the vsock port using `connectToPort_completionHandler`, dups the connection fd, and stores it in `dns_vsock_fd`
- Added `VmThread::dns_vsock_fd()` public API with dup-on-retrieve semantics

## Deviations

None — plan executed as written.

## Verification

```
cargo check -p speck-vz  ✓ (0 errors, 2 dead_code warnings for new public APIs)
```

## Known Stubs

None.

## Threat Flags

None.

## Commits

| Commit | Message |
|--------|---------|
| `c05b986` | feat(04-03): add network feature flags, socket2 dep, and network error variants |
| `716ec89` | feat(04-03): add network and dns_vsock_port fields to GuestConfig |
| `647af44` | feat(04-03): wire VZFileHandleNetworkDeviceAttachment in do_start |

## Duration

~10 minutes.

## Self-Check: PASSED

All created files exist, all 3 commits verified in git log, `cargo check -p speck-vz` passes with 0 errors.
