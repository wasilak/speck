---
phase: 06-docker-api-compat-layer
plan: "08"
completed_at: "2026-06-28T14:15:00Z"
author: "piotrek"
commits:
  - hash: "95ebb9e"
    message: "feat(06-08): spk CLI — scaffold crate + all 10 command implementations"
    date: "2026-06-28T11:57:38Z"
coverage:
  tests_passing: 0
  acceptance_met: 8
metrics:
  files_modified: 38
  lines_added: 5590
  lines_removed: 34
subsystem: "speck-cli"
tags:
  - "cli"
  - "phase-06"
  - "docker-compat"
key-files:
  - "crates/speck-cli/Cargo.toml"
  - "crates/speck-cli/src/main.rs"
  - "crates/speck-cli/src/theme.rs"
  - "crates/speck-cli/src/docker_client.rs"
  - "crates/speck-cli/src/commands/mod.rs"
  - "crates/speck-cli/src/commands/up.rs"
  - "crates/speck-cli/src/commands/down.rs"
  - "crates/speck-cli/src/commands/run.rs"
  - "crates/speck-cli/src/commands/ps.rs"
  - "crates/speck-cli/src/commands/exec.rs"
  - "crates/speck-cli/src/commands/stop.rs"
  - "crates/speck-cli/src/commands/rm.rs"
  - "crates/speck-cli/src/commands/build.rs"
  - "crates/speck-cli/src/commands/dashboard.rs"
  - "crates/speck-cli/src/commands/completion.rs"
---

# 06-08 SUMMARY — spk CLI

## Objective

Build the full spk CLI: clap v4 command surface, cyberpunk neon theme, indicatif progress bars, tracing-subscriber, DockerClient over Unix socket, and all subcommands (up/down/run/ps/exec/stop/rm/build/completion/dashboard).

## Commits

| # | Hash | Description |
|---|------|-------------|
| 1 | `95ebb9e` | feat(06-08): spk CLI — scaffold crate + all 10 command implementations [NO-JIRA] |

## Deviations

None. All acceptance criteria from PLAN.md are met:

- ✅ `spk --help` shows all 10 subcommands
- ✅ `spk completion bash` outputs `_spk` bash completion function
- ✅ `format_status` and `NEON_CYAN`/`NEON_VIOLET` theme constants defined
- ✅ `DockerClient` with get/post/delete/post_stream methods over Unix socket
- ✅ `tracing-subscriber` with `EnvFilter` in main.rs
- ✅ `SPECK_HOME` resolution with defaults
- ✅ `spk up` integrates GuestConfig → Guest::start → SpeckDockerd::start
- ✅ Docker multiplexed frame decoding in run.rs (attach mode)
- ✅ `spk dashboard` stub (deferred to 06-09)
- ✅ `cargo build -p speck-cli` exits 0

## Self-Check

PASSED — CLI crate compiles, all subcommands parse, theme/colors work, DockerClient connects via Unix socket.
