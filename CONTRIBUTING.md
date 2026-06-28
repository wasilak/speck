# Contributing to Speck

## Welcome

Speck is an ultra-fast container runtime for Apple Silicon that never loses the network under corporate VPNs and Cloudflare WARP. Your contributions help make this vision a reality.

## Development Setup

**Prerequisites:**
- Rust stable (edition 2024) — install via `rustup`
- Xcode Command Line Tools on macOS

The `.cargo/config.toml` sets the default build target to `aarch64-apple-darwin`. On Linux or non-Apple-Silicon machines, run `rustup target add aarch64-apple-darwin` first.

**Local CI replication:** Run `cargo xtask ci` to run all CI checks locally (fmt, clippy, no-print lint, test).

Note: `tracing` events in `speck-core` tests are silent by default (no subscriber installed in the library). This is correct, not a bug — the library only emits events; the frontend installs the subscriber.

## Contribution Workflow

1. Fork the repository and create a feature branch.
2. Make your changes.
3. Ensure your PR passes CI (fmt, clippy, aarch64 check, no-print lint, tests).
4. Open a pull request.

Before merge, you must sign the Contributor License Agreement (CLA). The CLA Assistant bot will prompt you on your first PR.

## CLA Section

We require a Contributor License Agreement to enable the Speck Enterprise commercial license path while keeping the community edition AGPLv3 forever. Without copyright assignment from all contributors, dual-licensing would be legally impossible.

See `.github/CLA.md` for the full text.

## Speck Enterprise

Speck is AGPLv3 open source. We plan to offer Speck Enterprise as a commercial product. AGPLv3 guarantees the community edition will always be open. The CLA allows us to fund development through commercial licenses.

## Code Style

- `cargo fmt` for formatting.
- `cargo clippy` for linting.

**Architectural rule:** Never use `print!`/`println!`/`eprintln!`/`dbg!` in `speck-core`. Use the `EventSink` trait instead.

Why? `speck-core` is a library consumed by the CLI, the Docker-socket daemon, and a future SwiftUI GUI. If Core writes to stdout directly, it breaks the presentation separation. CI enforces this via clippy `#![deny(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)]` plus a ripgrep-based hard gate.

## License

By contributing, you agree to the CLA in `.github/CLA.md`. Speck is licensed under AGPLv3 (see `LICENSE`).
