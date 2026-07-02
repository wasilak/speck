---
phase: 07
slug: gap-closure
status: complete
nyquist_compliant: true
wave_0_complete: true
created: 2026-07-02
updated: 2026-07-02
---

# Phase 07 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | Rust built-in test harness with `tokio::test` for async integration tests |
| **Config file** | Workspace `Cargo.toml`; no separate test config |
| **Quick run command** | `cargo test -p speck-cli -p speck-vz -p speck-dockerd --lib` |
| **Full suite command** | `cargo test --workspace` |
| **Ignored VM E2E command** | `cargo test -p speck-vz --test integration_07 -- --ignored --nocapture` |
| **Estimated runtime** | Quick library suite target < 60s; ignored VM E2E is manual/codesigned and may exceed 60s |

---

## Sampling Rate

- **After every task commit:** Run the task's `<automated>` command, then `cargo test -p speck-cli -p speck-vz -p speck-dockerd --lib` if the task touched Rust production code.
- **After every plan wave:** Run `cargo test --workspace`.
- **Before `/gsd-verify-work`:** Full suite must be green
- **Before phase sign-off:** Run `cargo test -p speck-vz --test integration_07 -- --ignored --nocapture` on a codesigned Apple Silicon host with required `$SPECK_HOME` artifacts.
- **Max feedback latency:** < 60 seconds for normal task checks; ignored E2E intentionally manual due VM/codesign prerequisites.

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| 07-01-01 | 01 | 1 | GAP-01 | T-07-01 | Blocking VM startup is isolated from Tokio runtime; errors propagate | lib/unit | `cargo test -p speck-cli --lib` | ✅ existing | ✅ green |
| 07-01-02 | 01 | 1 | GAP-01 | T-07-02 | Delegate stop/error events update state instead of being dropped | lib/unit | `cargo test -p speck-vz --lib` | ✅ existing | ✅ green |
| 07-01-03 | 01 | 1 | GAP-01 | T-07-SC | No new package install; explicit objc2-vz feature coverage | compile | `cargo test -p speck-vz --lib` | ✅ existing | ✅ green |
| 07-02-01 | 02 | 1 | GAP-03 | T-07-03 / T-07-04 | DockerClient dials Unix socket, not TCP | lib/unit | `cargo test -p speck-cli docker_client -- --nocapture` | ✅ existing | ✅ green |
| 07-02-02 | 02 | 1 | GAP-03 | T-07-05 | Streaming/upgrade paths use same Unix transport | lib/unit | `cargo test -p speck-cli --lib` | ✅ existing | ✅ green |
| 07-03-01 | 03 | 1 | D-05 / STORAGE-01 | T-07-06 / T-07-07 | Bind paths validated before host exposure | lib/unit | `cargo test -p speck-dockerd containers -- --nocapture` | ✅ existing | ✅ green |
| 07-03-02 | 03 | 1 | D-05 / STORAGE-01 | T-07-06 / T-07-07 | Blocking checkpoint confirms exact D-05 VirtioFS create/start implementation path before lifecycle code changes | source gate + manual decision | `python3 -c 'from pathlib import Path; p=Path(".planning/phases/07-gap-closure/07-03-PLAN.md").read_text(); assert "option id=\"d05-supported\"" in p and "option id=\"d05-replan-required\"" in p and "Do not proceed to Task 3" in p and "execution stops before Task 3" in p'` | ✅ plan task | ✅ green |
| 07-03-03 | 03 | 1 | D-05 / STORAGE-01 + GAP-04 | T-07-08 | Bind-mount wiring implements D-05 exactly and does not regress localhost port maps | lib/unit | `cargo test -p speck-vz -p speck-dockerd --lib` | ✅ existing | ✅ green |
| 07-04-01 | 04 | 1 | D-06 / BUILD-01 | T-07-09 / T-07-10 | Build body is bounded or streamed safely | lib/unit | `cargo test -p speck-dockerd build -- --nocapture` | ✅ existing | ✅ green |
| 07-04-02 | 04 | 1 | D-06 / BUILD-01 | T-07-11 | BuildKit request carries local context/session input | lib/unit | `cargo test -p speck-dockerd --lib` | ✅ existing | ✅ green |
| 07-05-01 | 05 | 2 | GAP-01..04 + D-05/STORAGE-01 + D-06/BUILD-01 | T-07-12 / T-07-13 / T-07-14 | Ignored E2E tests document runtime, Unix socket, egress, localhost port, bind mount, and build context behavior | compile/source gate + manual ignored E2E | `cargo test -p speck-vz --test integration_07 --no-run` | ✅ created (3d31a892) | ✅ green |
| 07-05-02 | 05 | 2 | GAP-01..04 | T-07-12 | Validation map stays concrete and complete | source gate | `grep -v '^#' 07-VALIDATION.md \| grep -c template_token \| xargs test 0 -eq` (no template placeholder braces) | ✅ this file | ✅ green |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [x] `crates/speck-vz/tests/integration_07.rs` — ignored E2E tests for GAP-01, GAP-02, GAP-03, GAP-04, D-05/STORAGE-01, and D-06/BUILD-01. Created by Plan 07-05 (commit 3d31a892).
- [x] DockerClient source gates — Plan 07-02 removed `HttpConnector`, `build_http()`, and `let _ = &self.sock_path`; `test_spk_ps_uses_unix_socket` enforces this via `include_str!` compile-time check.
- [x] BuildKit context-copy fixture — `test_build_context_copy` creates a tar with `Dockerfile` (`FROM alpine\nCOPY app.txt /app/app.txt`) and `app.txt`; empty-context build would fail the COPY instruction.

*If none: "Existing infrastructure covers all phase requirements."*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| `spk up` starts a real VM without runtime panic | GAP-01 | Requires Apple Silicon host, codesigned binary, Virtualization.framework entitlement, and `$SPECK_HOME` kernel/initrd/rootfs/data artifacts | Run `cargo test -p speck-vz --test integration_07 test_spk_up_no_runtime_panic -- --ignored --nocapture` |
| `spk run alpine ping -c 1 8.8.8.8` returns RTT output | GAP-02 | Requires real guest networking and external egress | Run `cargo test -p speck-vz --test integration_07 test_spk_run_ping_egress -- --ignored --nocapture` |
| `spk ps` uses Unix socket | GAP-03 | Requires running Speck dockerd socket for E2E proof; source gate is automated | Run `cargo test -p speck-vz --test integration_07 test_spk_ps_uses_unix_socket -- --ignored --nocapture` |
| `spk run -p 8080:80 nginx` responds from macOS localhost | GAP-04 | Requires real container image, port bridge, and host curl/local HTTP check | Run `cargo test -p speck-vz --test integration_07 test_port_publish_localhost_nginx -- --ignored --nocapture` |
| D-05 implementation path decision is approved or execution stops | D-05 / STORAGE-01 | Human must approve the executor's feasibility finding because exact create/start-time VirtioFS support is a locked user decision and unapproved substitutes are forbidden | In Plan 07-03 Task 2, review the cited source/API path. Select `d05-supported` only if exact D-05 is confirmed; select `d05-replan-required` to stop before Task 3. This manual sign-off is outside automated sampling; the automated source gate only verifies the checkpoint contract remains present. |
| Bind mount visible inside container | D-05 / STORAGE-01 | Requires real container execution and guest-visible VirtioFS mapping | Run `cargo test -p speck-vz --test integration_07 test_bind_mount_visible_in_container -- --ignored --nocapture` |
| Build context COPY reaches BuildKit | D-06 / BUILD-01 | Requires running BuildKit in guest and local build context upload | Run `cargo test -p speck-vz --test integration_07 test_build_context_copy -- --ignored --nocapture` |

*If none: "All phase behaviors have automated verification."*

---

## Validation Sign-Off

- [x] All planned tasks have `<automated>` verify or explicit ignored E2E command.
- [x] Blocking D-05 decision checkpoint is modeled as manual sign-off outside automated sampling; its automated command only checks the source checkpoint contract is present.
- [x] Sampling continuity: no 3 consecutive tasks without automated verify.
- [x] Wave 0/Wave 2 requirements identify `integration_07.rs` creation before phase sign-off.
- [x] No watch-mode flags.
- [x] Feedback latency < 60s for normal task checks; manual ignored E2E documented separately.
- [x] `nyquist_compliant: true` set in frontmatter because this strategy is concrete.

**Approval:** ready for Phase 07 execution; ignored E2E sign-off pending implementation and codesigned runtime.
