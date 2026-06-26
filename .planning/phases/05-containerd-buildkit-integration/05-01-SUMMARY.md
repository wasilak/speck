---
phase: 05-containerd-buildkit-integration
plan: 01
subsystem: speck-vz
tags: [config, error-types, cargo-features, virtio-blk, tdd]
requires: []
provides: [05-01-GuestConfig-extensions, 05-01-Error-variants, 05-01-objc2-feature-flags]
affects: [crates/speck-vz/src/config.rs, crates/speck-vz/src/error.rs, crates/speck-vz/Cargo.toml]
tech-stack:
  added: []
  patterns: [optional-field-with-builder-setter, validate-disk-path-exists]
key-files:
  created: []
  modified:
    - crates/speck-vz/src/config.rs (+103 lines)
    - crates/speck-vz/src/error.rs (+8 lines)
    - crates/speck-vz/Cargo.toml (+4 lines)
decisions: []
metrics:
  duration: ~15 min
  completed_date: 2026-06-26
  tasks: 2
---

# Phase 5 Plan 1: GuestConfig Extensions + Error Variants + Feature Flags

**One-liner:** Added five new optional `GuestConfig` fields (disk paths + vsock ports), two `Error` variants for disk/ready failures, and four `objc2-virtualization` feature flags for virtio-blk — providing the type foundation for Phase 5 container integration.

## Task Summary

| # | Name                          | Type | Status | Commit |
|---|-------------------------------|------|--------|--------|
| 1 | GuestConfig + builder fields  | auto (tdd) | ✅ Done | `6a55189` (RED), `a074ed9` (GREEN) |
| 2 | Error variants + feature flags| auto | ✅ Done | `b1b3200` |

## What Was Built

### 1. `GuestConfig` 5 new optional fields

- `rootfs_disk_path: Option<PathBuf>` — path to ext4 rootfs image (/dev/vda)
- `data_disk_path: Option<PathBuf>` — path to ext4 data disk (/dev/vdb, /var/lib/containerd)
- `containerd_vsock_port: Option<u32>` — vsock port for containerd gRPC forwarder (recommended 9001)
- `buildkitd_vsock_port: Option<u32>` — vsock port for BuildKit gRPC forwarder (recommended 9002)
- `ready_vsock_port: Option<u32>` — vsock port for vminitd READY signal (recommended 9000)

All with builder setter methods on `GuestConfigBuilder`, following the `dns_vsock_port` pattern.

### 2. `GuestConfig::validate()` disk path checks

- Returns `Err(...)` when `rootfs_disk_path` is `Some` but file doesn't exist
- Returns `Err(...)` when `data_disk_path` is `Some` but file doesn't exist
- Passes (returns `Ok`) when both are `None` (fields are optional)

### 3. Two new `Error` variants

- `DiskAttachment(String)` — virtio-blk attachment failure
- `GuestReadyTimeout` — vminitd READY signal not received in time

### 4. Four new `objc2-virtualization` feature flags

- `VZStorageDeviceConfiguration`
- `VZVirtioBlockDeviceConfiguration`
- `VZStorageDeviceAttachment`
- `VZDiskImageStorageDeviceAttachment`

## TDD Gate Compliance

| Gate | Commit | Hash |
|------|--------|------|
| RED (test) | `test(05-01): add failing test for Phase 5 GuestConfig fields` | `6a55189` |
| GREEN (feat) | `feat(05-01): implement Phase 5 GuestConfig fields + builder + validation` | `a074ed9` |
| REFACTOR | Skipped — implementation clean | — |

## Verification Results

| Check | Result |
|-------|--------|
| `cargo build -p speck-vz` | ✅ Passes (0 errors, 2 pre-existing dead_code warnings in speck-net) |
| `cargo test -p speck-vz --lib -- config` | ✅ Passes (1 passed, test_guest_config_phase5_fields) |
| `cargo clippy -p speck-vz -- -Dwarnings` | ⚠️ Pre-existing toolchain issue (rustc Homebrew vs binary version mismatch in third-party crates — `cargo check` clean) |
| GuestConfig has 5 new fields | ✅ |
| GuestConfig::validate() rejects missing disks | ✅ |
| Error enum has DiskAttachment | ✅ |
| Error enum has GuestReadyTimeout | ✅ |
| objc2-vz features: 4 virtio-blk classes | ✅ |

## Deviations from Plan

None — plan executed exactly as written.

## Known Stubs

None — this is a type-definition-only plan with no stubs.

## Threat Flags

None — no new runtime trust boundaries introduced.

## Self-Check: PASSED

- ✅ `crates/speck-vz/src/config.rs` — found, 5 new fields + builder methods + validation + tests
- ✅ `crates/speck-vz/src/error.rs` — found, DiskAttachment + GuestReadyTimeout variants
- ✅ `crates/speck-vz/Cargo.toml` — found, 4 new objc2-virtualization features
- ✅ `SUMMARY.md` — found at `.planning/phases/05-containerd-buildkit-integration/05-01-SUMMARY.md`
- ✅ Commit `6a55189` — RED test commit
- ✅ Commit `a074ed9` — GREEN implementation commit
- ✅ Commit `b1b3200` — error variants + feature flags commit
