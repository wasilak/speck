# Phase 09 Discussion Log

**Date:** 2026-07-03
**Phase:** 09-daemon-lifecycle

---

## Summary

Pre-planning discussion for Phase 09 (Daemon Lifecycle). Resolved 4 gray areas before plan generation.

---

## Discussion Areas

### Area 1 — Re-exec / Daemonization Mechanism

**Question asked:**
> How should `spk up` daemonize? Options: (A) LaunchAgent re-exec, (B) `daemonize` crate / double-fork, (C) `--foreground` only (no daemon mode yet).

**Context provided:**
- `daemonize` crate and `fork()` are banned per PROJECT.md constraints.
- launchd is macOS-canonical for persistent background processes.
- Apple's `containerization` CLI uses the re-exec pattern.

**Decision:** Option A — LaunchAgent re-exec. `spk up` daemonizes by default; `--foreground` keeps it inline. Internal `SPECK_DAEMONIZED=1` env var prevents re-exec loops.

---

### Area 2 — Control Socket

**Question asked:**
> What should the control socket support? Options: (A) Liveness only (ping-pong), (B) Full command channel (stop/restart/status), (C) No socket (signal-only).

**Context provided:**
- `spk down` needs to confirm daemon liveness before `launchctl bootout`.
- Full command channel would require an IPC protocol not yet designed.
- SIGTERM is sufficient for graceful shutdown if the handler is implemented correctly.

**Decision:** Option A — Liveness only. Socket at `$SPECK_HOME/run/control.sock`. `spk down` uses `launchctl bootout`; SIGTERM triggers graceful shutdown. Full command channel deferred.

---

### Area 3 — Logging

**Question asked:**
> How should the daemon log? Options: (A) `tracing-appender` size-based rotation, (B) `tracing-appender` time-based (daily) rotation, (C) stdout only (OSLog/unified logging).

**Context provided:**
- `tracing-appender` integrates with existing `tracing` ecosystem.
- Size-based rotation is simpler to reason about for a long-running daemon.
- macOS unified logging (OSLog) would require additional FFI.

**Decision:** Option A — `tracing-appender` with size-based rotation (10 MB / 5 files). `--foreground` uses stdout subscriber.

---

### Area 4 — Down / Restart Semantics

**Question asked:**
> How should `spk down` behave? And what happens on restart failure? Options: (A) Graceful (stop containers first), (B) Force (SIGTERM immediately), (C) Two-phase (try graceful, fallback to force).

**Context provided:**
- Phase 08 already detects resource mismatches that require restart.
- Graceful stop prevents corruption in running containers.
- Restart failure is an edge case; keeping it simple avoids a complex state machine.

**Decision:** Option A — Graceful shutdown (stop containers, then VM). Restart failure: log WARN, restore LaunchAgent plist, exit non-zero. User retries with `spk up`.

---

## Outcome

All 4 gray areas resolved. Context written to `09-CONTEXT.md`. Ready for `/gsd:plan-phase 09`.
