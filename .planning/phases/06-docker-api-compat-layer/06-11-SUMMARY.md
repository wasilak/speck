---
phase: 06-docker-api-compat-layer
plan: "11"
completed_at: "2026-06-28T18:00:00Z"
author: "piotrek"
coverage:
  tests_passing: 29
  acceptance_met: 6
metrics:
  files_modified: 4
  lines_added: 420
  lines_removed: 0
subsystem: "tests"
tags:
  - "testing"
  - "docker-compat"
  - "phase-06"
key-files:
  - "crates/speck-dockerd/tests/api_conformance.rs"
  - "crates/speck-vz/tests/integration_06.rs"
  - "crates/speck-dockerd/Cargo.toml"
  - ".github/workflows/ci.yml"
---

# 06-11 SUMMARY — testcontainers conformance

## Objective

Write bollard-based API conformance tests and end-to-end integration tests for the Docker API compat layer.

## Commits

None yet — changes are in working tree alongside other phase 6 plans.

## Deliverables

- `crates/speck-dockerd/tests/api_conformance.rs` — 12 bollard 0.21.0 tests:
  - `test_ping` — GET /_ping
  - `test_version` — GET /version
  - `test_info` — GET /info
  - `test_image_pull_alpine` — pull alpine:latest via CreateImageOptionsBuilder
  - `test_image_list` — GET /images/json
  - `test_container_create_list_remove` — create/list/remove lifecycle
  - `test_container_start_wait_remove` — start/wait/remove lifecycle
  - `test_container_logs` — create/start/wait/logs/remove
  - `test_exec_create_start` — exec create/start with output capture
  - `test_events_stream_starts` — GET /events stream init
  - `test_network_create_list_remove` — create/list/remove network
  - `test_volume_create_list_remove` — create/list/remove volume
- `crates/speck-vz/tests/integration_06.rs` — 3 `#[ignore]`-gated e2e tests
- `futures-util` added to dev-dependencies
- `.github/workflows/ci.yml` — conformance job comment in README
- `cargo test --workspace` — 29 passed, 24 ignored ✅

## Key Fix

bollard 0.21.0 API differs from initial assumptions:
- `connect_with_unix` takes `&str + &ClientVersion` (not `PathBuf`)
- Options use `bollard::query_parameters` with builder pattern
- Models use `bollard::models::*` (Config→ContainerCreateBody, etc.)
- No generic type parameters on API calls
- `LogOutput` uses `AsRef<[u8]>` not `to_bytes()`
- `Volume.name` is `String` (not `Option<String>`)
- `remove_volume` takes optional options parameter

## Self-Check

PASSED — `cargo test --workspace` exits 0 with 29 passed, 24 ignored.
