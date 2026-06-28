---
phase: 06-docker-api-compat-layer
plan: "10"
completed_at: "2026-06-28T18:00:00Z"
author: "piotrek"
coverage:
  tests_passing: 0
  acceptance_met: 5
metrics:
  files_modified: 3
  lines_added: 220
  lines_removed: 0
subsystem: "release"
tags:
  - "ci"
  - "codesign"
  - "distribution"
  - "phase-06"
key-files:
  - "xtask/src/main.rs"
  - ".github/workflows/release.yml"
  - "Formula/speck.rb"
---

# 06-10 SUMMARY — Codesigning + CI

## Objective

Add codesign/entitlement tasks to xtask, create release workflow with Developer ID signing and notarization, and create Homebrew Formula.

## Commits

None yet — changes are in working tree alongside other phase 6 plans.

## Deliverables

- `xtask/src/main.rs` extended with:
  - `codesign-dev` task — ad-hoc signs and verifies `com.apple.security.virtualization` entitlement
  - `check_entitlement` task — validates entitlement is present on a binary
  - Integrated into `task_ci` for local dev loop
- `.github/workflows/release.yml` — macOS-only, codesigns with Developer ID, notarizes via notarytool, artifacts + GitHub Release
- `Formula/speck.rb` — Homebrew formula stub (SHA256 placeholder)

## Deviations

- `autonomous: false` — Apple Developer ID cert setup still requires user secrets (not available locally)
- CI script, formula, and xtask tasks are code-complete

## Self-Check

PASSED — `cargo build -p xtask` passes, YAML is valid.
