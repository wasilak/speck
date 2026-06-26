---
phase: 05-containerd-buildkit-integration
plan: 03
subsystem: vm-boot, ci
tags: [rootfs, ext4, containerd, runc, buildkit, github-actions, bash, xtask]

requires:
  - phase: 05-containerd-buildkit-integration (02)
    provides: vminitd supervision + disk mounting lifecycle

provides:
  - scripts/fetch-rootfs.sh — download + SHA256 verify rootfs.img and create data.img stub
  - xtask task_init extended to call fetch-rootfs.sh
  - .github/workflows/build-rootfs.yml — CI arm64 ext4 image builder

affects:
  - 05-04 (disk attachment + wait-for-ready)
  - 05-05 (guest API + integration tests)

tech-stack:
  added: [bash, GitHub Actions ubuntu-24.04-arm runner, gunzip]
  patterns: [idempotent resource download (D-02), CI-built disk image, SHA256-verified binary artifacts]

key-files:
  created:
    - scripts/fetch-rootfs.sh
    - .github/workflows/build-rootfs.yml
  modified:
    - xtask/src/main.rs

key-decisions:
  - "fetch-rootfs.sh uses shasum -a 256 (not sha256sum) for macOS compatibility"
  - "data.img is zero-filled (not ext4-formatted); vminitd formats on first boot"
  - "task_init refactored to iterate over script list instead of duplicating code"
  - "CI workflow uses softprops/action-gh-release@v2 for GitHub Release artifact upload"
  - "containerd 2.3.2 config uses version = 3 format with overlayfs snapshotter"

patterns-established:
  - "Resource fetch scripts follow the fetch-kernel.sh pattern: SPECK_HOME default, version arg, existence skip, progress output"
  - "CI-built artifacts are gzip-compressed with companion .sha256 checksum files"
  - "xtask init orchestrates all one-time setup scripts in sequence"

requirements-completed: [RUN-06]

duration: 3min
completed: 2026-06-26
---

# Phase 05 Plan 03: Rootfs download script, xtask extension, and CI workflow for arm64 ext4 rootfs image

**Two disk resources (rootfs.img with containerd/runc/buildkitd binaries + zero-filled data.img stub), fetched via new script and orchestrated by xtask init, built by GitHub Actions arm64 runner publishing to releases**

## Performance

- **Duration:** 3 min
- **Started:** 2026-06-26T18:04:47Z
- **Completed:** 2026-06-26T18:07:30Z
- **Tasks:** 3
- **Files modified:** 3

## Accomplishments

- `scripts/fetch-rootfs.sh` downloads rootfs.img.gz from GitHub Releases, SHA256-verifies with `shasum -a 256`, and creates a 512 MB zero-filled `data.img` stub — idempotent per D-02 pattern
- `cargo xtask init` now calls both `fetch-kernel.sh` and `fetch-rootfs.sh` via a unified script iterator
- `.github/workflows/build-rootfs.yml` builds a 512 MB ext4 image on `ubuntu-24.04-arm` with containerd 2.3.2 + runc 1.5.0 + buildkitd 0.31.1, compressed with gzip + SHA256 checksum, uploaded as GitHub Release artifact
- Threat mitigations applied: T-05-03-01 (SHA256 verification before use), T-05-03-02 (version pins for all binaries)
- No new Cargo dependencies added (T-05-SC)

## Task Commits

Each task was committed atomically:

1. **Task 1: Create scripts/fetch-rootfs.sh** — `e434612` (feat)
2. **Task 2a: Extend xtask/src/main.rs** — `9d510e3` (feat)
3. **Task 2b: Create .github/workflows/build-rootfs.yml** — `7710689` (feat)

## Files Created/Modified

- `scripts/fetch-rootfs.sh` (NEW) — Download + SHA256 verify rootfs.img, create data.img stub
- `xtask/src/main.rs` (MODIFIED) — `task_init()` now iterates over both fetch scripts
- `.github/workflows/build-rootfs.yml` (NEW) — CI arm64 ext4 image builder workflow

## Decisions Made

- **shasum -a 256 over sha256sum:** macOS ships `shasum` not `sha256sum`; CI Ubuntu has both so `shasum` works everywhere
- **data.img as zero-filled raw file:** `VZDiskImageStorageDeviceAttachment` accepts RAW format; vminitd's `mke2fs -t ext4` on first boot is simpler than pre-formatting (which requires Linux)
- **task_init refactored to script list:** Instead of duplicating the check-run-check pattern per script, iterates over `&["scripts/fetch-kernel.sh", "scripts/fetch-rootfs.sh"]` — cleaner and extensible for future init steps
- **version = 3 containerd config:** containerd 2.x uses config version 3 format; overlayfs snapshotter is set for performance (native fallback if kernel lacks overlayfs, handled later)

## Deviations from Plan

None — plan executed exactly as written.

## Issues Encountered

None — all verifications passed on first attempt.

## User Setup Required

None — no external service configuration required.

## Next Phase Readiness

- rootfs fetch/download infrastructure ready for Plan 05-04 (disk attachment + WaitForGuestReady)
- CI workflow ready for manual/automated rootfs image builds
- Initial rootfs image needs to be built via GitHub Actions before `cargo xtask init` succeeds (fetch-rootfs.sh will fail gracefully with curl exit code until a release artifact is published)

## Self-Check: PASSED

- ✅ `scripts/fetch-rootfs.sh` exists and syntax-valid
- ✅ `xtask/src/main.rs` exists and builds
- ✅ `.github/workflows/build-rootfs.yml` exists and YAML-valid
- ✅ All 3 commits found in git history
- ✅ `bash -n scripts/fetch-rootfs.sh` passes
- ✅ `cargo build -p xtask` passes

---

*Phase: 05-containerd-buildkit-integration*
*Completed: 2026-06-26*
