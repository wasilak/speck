# Phase 09 Context: Daemon Lifecycle

**Phase:** 09-daemon-lifecycle
**Status:** Planning
**Date:** 2026-07-03

---

## Domain

Implement `spk up` daemon mode so the VM runs as a background process that survives terminal close, surfaces `spk down` for clean shutdown, and emits structured rotating logs to disk. The daemon must integrate cleanly with macOS conventions (launchd), remain debuggable via `--foreground`, and gracefully stop running containers before tearing down the VM.

---

## Decisions

### D1 — Re-exec / Daemonization Mechanism

**Decision:** LaunchAgent re-exec pattern. `spk up` daemonizes by default.

- `spk up` (default): writes a temporary LaunchAgent plist, calls `launchctl bootstrap`, then exits 0. launchd re-launches `spk up --foreground` as a managed service.
- `spk up --foreground`: runs inline, logs to stdout, no launchd involvement.
- An internal env var `SPECK_DAEMONIZED=1` identifies launchd-managed instances (prevents re-exec loops).
- **Rationale:** Apple's canonical daemon pattern on macOS; no `fork()` (banned); `daemonize` crate is banned per PROJECT.md constraints; survives terminal close natively; crash-restart handled by launchd `KeepAlive`.

### D2 — Control Socket

**Decision:** Minimal Unix socket at `$SPECK_HOME/run/control.sock` — liveness/ping-pong only.

- Socket serves one purpose: `spk down` pings it to confirm the daemon is alive before issuing `launchctl bootout`.
- SIGTERM handler performs graceful VM shutdown (stop containers, then VM).
- No structured command channel; `spk down` does not send shutdown commands over the socket — it uses `launchctl bootout` to trigger SIGTERM.
- **Rationale:** Keeps the control plane simple; avoids building a full IPC protocol in Phase 09. Full command channel deferred to a later phase if needed.

### D3 — Logging

**Decision:** `tracing-appender` non-blocking writer to `$SPECK_HOME/speck.log`, size-based rotation.

- Size-based rotation: 10 MB per file, 5 files retained (oldest rolled off).
- `--foreground` mode uses the stdout subscriber instead (no file logging).
- `tracing-appender` is a new dependency to add to `speck-cli/Cargo.toml`.
- **Rationale:** `tracing-appender` integrates naturally with the existing `tracing` ecosystem already in use; non-blocking writer avoids log I/O on the hot path; size-based rotation is simpler to reason about than time-based for a long-running daemon.

### D4 — Down / Restart Semantics

**Decision:** Graceful shutdown: stop containers first, then VM. Restart failure: log WARN and exit non-zero.

- `spk down`: stop all running containers (via containerd gRPC over vsock), then send VM stop signal, then `launchctl bootout`.
- On restart failure (e.g., config mismatch detected in Phase 08): log a WARN, restore LaunchAgent state (re-register plist), exit non-zero. User retries with `spk up`.
- No automatic retry loop for restart failures.
- **Rationale:** Graceful stop prevents data corruption in running containers; keeping restart simple avoids complex state machine in Phase 09; user is always in control of retry.

---

## Canonical References

- **ROADMAP.md §Phase 09** — goal, 5 success criteria, DAEMON-01..05 req IDs
- **REQUIREMENTS.md §Daemon Lifecycle (DAEMON)** — DAEMON-01..05, all Pending
- **STATE.md** — key decisions section records D1–D4 from this discussion

---

## Code Context

**Existing relevant code:**
- `crates/speck-cli/src/commands/down.rs` — 7-line stub, prints "spk down: send SIGTERM..." message; Phase 09 will implement this fully.
- `crates/speck-cli/src/commands/up.rs` — `run_up()` is the entry point; daemon mode branches from here.
- `crates/speck-cli/src/main.rs` — `UpArgs` struct; add `--foreground` flag here.
- No `restart.rs` exists; restart logic lives inside `run_up()` or a dedicated module.

**New dependencies needed:**
- `tracing-appender` — add to `crates/speck-cli/Cargo.toml`

**File locations for Phase 09 deliverables:**
- `$SPECK_HOME/run/control.sock` — runtime control socket
- `$SPECK_HOME/run/vm-config.json` — already written by Phase 08 (used for resource snapshot)
- `$SPECK_HOME/speck.log` (+ rotated variants) — daemon log output
- LaunchAgent plist: `~/Library/LaunchAgents/io.speck.vm.plist` (temporary, written at `spk up` time)

---

## Deferred

- Full structured IPC command channel over `control.sock` (beyond ping-pong) — not Phase 09
- `spk restart` as a first-class command — not Phase 09; user runs `spk down && spk up`
- Time-based log rotation — deferred; size-based is sufficient for Phase 09
- K3s / multi-VM daemon management — out of scope until a dedicated phase
