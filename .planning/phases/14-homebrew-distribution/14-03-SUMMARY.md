---
plan: 14-03
phase: 14-homebrew-distribution
status: deferred
---

# Plan 14-03 Summary — External Infrastructure Checkpoint (Deferred)

## Status: Deferred

This plan requires:
1. Apple Developer Program membership ($99/year)
2. Developer ID Installer certificate
3. `speck-runtime/homebrew-speck` tap repository
4. Four GitHub Secrets: `BUILD_INSTALLER_CERTIFICATE_BASE64`, `P12_INSTALLER_PASSWORD`, `DEVELOPER_ID_INSTALLER_NAME`, `TAP_REPO_TOKEN`

All of these are deferred until the project reaches a stable release and the full Developer ID + `.pkg` + Cask distribution path (documented in `14-01-PLAN.md`, `14-02-PLAN.md`, `14-03-PLAN.md`) is activated.

## What unblocks this

- Apple Developer Program enrollment
- Decision to move from Formula/ad-hoc to Cask/notarized distribution
- Re-executing plans 14-01 and 14-02 using the original (non-simplified) plan content

## Current distribution state

Ad-hoc signed tarball via `Formula/speck.rb` — sufficient for battle-testing on developer machines and Homebrew installs where the user controls the machine.
