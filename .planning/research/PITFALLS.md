# Pitfalls Research — v1.2 Hardened Runtime

**Domain:** Apple Silicon container runtime — adding stability/testing, testcontainers conformance, Developer ID distribution, and daemon polish to the existing Speck Rust runtime.
**Researched:** 2026-07-06
**Confidence:** HIGH for notarization pitfalls (Apple official docs, Apple Developer Forums, rsms gist), HIGH for SQLite/async pitfalls (tokio-rs/axum discussions, tokio-rusqlite docs), MEDIUM for DNS test isolation (Rust community forums, nextest docs), HIGH for VZVirtioConsoleDeviceConfiguration (Apple Developer Forums thread 758354, code-hex/vz source), HIGH for restart race conditions (deepwiki apple/containerization, vm_thread.rs source analysis).

---

## Area 1: Unit-Testing the DNS Proxy (dns.rs + SCDynamicStore)

### Pitfall 1-A: `resolve_dns` calls getaddrinfo directly — no seam for test injection

**What goes wrong:**
`resolve_dns()` resolves via `addr_str.to_socket_addrs()`, which calls the macOS system resolver (getaddrinfo) inline. Tests that call `resolve_dns` against real domain names are network-dependent, non-deterministic, and will produce different NXDOMAIN/NODATA/SERVFAIL responses in different network environments (VPN on, VPN off, offline CI, split-DNS). You cannot test the NXDOMAIN-to-SERVFAIL translation logic, the QTYPE-filtering path, or the VPN-scoped vs. default resolver branch without controlling what getaddrinfo returns.

**Why it happens:**
`std::net::ToSocketAddrs` is a synchronous system call with no mockable layer. Rust doesn't allow monkey-patching (calls are resolved at compile time), and `dns.rs` exposes no trait boundary around resolution.

**Prevention:**
Introduce a `trait Resolver: Send + Sync { fn resolve(&self, domain: &str) -> Option<Vec<IpAddr>>; }`. The production implementation calls `getaddrinfo`. The test implementation returns a `HashMap<String, Vec<IpAddr>>` loaded at construction. Refactor `resolve_dns` to take `&dyn Resolver` (or a generic `R: Resolver`). Unit tests then exercise all code paths without any network access.

For the direct UDP path (VPN-scoped queries), similarly introduce a `trait DirectDns: Send + Sync { fn query(&self, server: SocketAddr, raw_query: &[u8]) -> Option<Vec<u8>>; }`. Test with a captured real DNS response or a hand-crafted wire-format response.

Example shape:
```rust
pub trait Resolver: Send + Sync {
    fn resolve_addrs(&self, domain: &str) -> Result<Vec<IpAddr>, ResolveError>;
}

pub struct SystemResolver;
impl Resolver for SystemResolver {
    fn resolve_addrs(&self, domain: &str) -> Result<Vec<IpAddr>, ResolveError> {
        // calls to_socket_addrs
    }
}

#[cfg(test)]
pub struct FakeResolver(pub HashMap<String, Vec<IpAddr>>);
```

### Pitfall 1-B: ResolverTable comes in via `watch::Receiver` — no seam needed, but test setup is non-obvious

**What goes wrong:**
`spawn_dns_proxy` takes a `tokio::sync::watch::Receiver<ResolverTable>`. This is actually already injectable — tests can create a `watch::channel` with a preloaded `ResolverTable`, send it to the function under test, and update it mid-test to simulate VPN connect/disconnect. The pitfall is that developers assume a full SCDynamicStore is required and skip testing the VPN-scoped branch entirely.

**Prevention:**
Tests for the VPN-scoped path only need `watch::channel(ResolverTable::with_entries(...))`. No SCDynamicStore required. The SCDynamicStore integration lives in the caller of `spawn_dns_proxy` (the `resolver_watch` module), which can be tested separately against a fake `DynamicStoreCallback`.

### Pitfall 1-C: Parallel tests fighting over shared getaddrinfo state

**What goes wrong:**
`cargo test` runs tests within the same process. If multiple tests call `resolve_dns` in parallel (which they do by default), they share the OS resolver cache and any `/etc/hosts` mutations. A test that calls `unsafe std::env::set_var("RES_OPTIONS", "...")` to change resolver behavior will corrupt other tests running concurrently. Even without env mutation, test results depend on the DNS state at the moment of execution — flaky on CI where DNS is restricted.

**Prevention:**
- Use `cargo nextest` as the test runner. Nextest spawns each test in its own process, providing full isolation.
- Any test that must call real `getaddrinfo` (integration tests, not unit tests) should be tagged `#[ignore]` and run only in a specific CI job with known network access.
- Never call `std::env::set_var` in a test that shares a process with other tests. With nextest's process-per-test model this becomes safe, but is still a footgun if someone runs `cargo test` directly.

### Pitfall 1-D: Testing the length-prefixed framing in `spawn_dns_proxy` requires fake vsock fds

**What goes wrong:**
`spawn_dns_proxy` takes a raw `RawFd` for vsock communication. Tests that try to exercise the framing protocol (2-byte BE length prefix, oversized-query discard) need a real fd pair. Using `socketpair(AF_UNIX, SOCK_STREAM, 0)` works fine, but tests that forget to close the write end will hang in `read_exact_fd` waiting for EOF that never comes.

**Prevention:**
Always close the write end of the socketpair from the test after writing all test data. Pattern:
```rust
let (read_fd, write_fd) = socketpair_blocking();
write_test_query(write_fd, &query_bytes);
drop_fd(write_fd);  // signals EOF to the proxy thread
let result = recv_response(read_fd);
```
Add `#[timeout]` or tokio `time::timeout` around the join handle to surface hangs as test failures rather than indefinite waits.

---

## Area 2: SQLite State Persistence in the Axum Server

### Pitfall 2-A: Calling rusqlite directly from async handlers blocks the tokio worker thread

**What goes wrong:**
`rusqlite` is synchronous. A handler that acquires a rusqlite `Connection` and calls `execute()` or `query_row()` blocks the tokio worker thread for the duration of the syscall. Under load this exhausts the worker pool, causing the Docker API server to stop accepting connections. The failure is not a crash — it manifests as timeouts that are hard to diagnose.

**Why it happens:**
The existing `AppState` handlers use `tokio::sync::Mutex` and `await` on them without blocking calls. Adding rusqlite inside an async handler looks syntactically identical but is semantically different: the lock is `await`-ed (non-blocking), but the DB call itself blocks a worker thread.

**Prevention:**
Use `tokio-rusqlite` (wraps the connection in a dedicated `std::thread` with a channel — all calls go through `conn.call(|c| { ... }).await`). This is the tokio community recommendation for rusqlite with axum. Do not use `Arc<Mutex<rusqlite::Connection>>` with `tokio::sync::Mutex` — it will work at low concurrency but will stall under load when threads pile up.

Alternative: `deadpool-sqlite` for connection pooling, but a single-connection setup with `tokio-rusqlite` is simpler for Speck's single-writer use case.

### Pitfall 2-B: Holding a `tokio::sync::Mutex` guard across a `.await` point deadlocks

**What goes wrong:**
`std::sync::MutexGuard` is `!Send` in most async contexts. If code accidentally holds a `tokio::sync::Mutex` guard across an `.await` (e.g., while waiting for a gRPC response), and another task tries to acquire the same mutex on the same tokio thread, the runtime deadlocks. The symptom is the axum server freezing with no error.

**Why it happens:**
The current `AppState` stores `network_store`, `volume_store`, `exec_store` as `Arc<tokio::sync::Mutex<...>>`. Handlers acquire the lock, do work, then release. If state persistence is added naively (e.g., lock → modify in-memory → call DB → unlock), the DB call inside the lock scope is an `.await` across a lock guard.

**Prevention:**
Pattern: clone the data you need out from under the lock, release the lock, then write to the DB outside the lock:
```rust
// Good: lock scope is minimal
let snapshot = {
    let mut store = state.network_store.lock().await;
    store.insert(id.clone(), network.clone());
    store.clone()  // snapshot for persistence
};
// DB write happens outside the lock
state.db.call(move |c| persist_networks(c, &snapshot)).await?;
```
Never call `.await` while holding a `tokio::sync::MutexGuard`.

### Pitfall 2-C: Schema migrations must complete before serving requests

**What goes wrong:**
The axum server starts listening on the Docker socket before schema migrations run. A container create request arrives before `CREATE TABLE containers` completes, and the handler gets `no such table` errors that are returned as 500s.

**Prevention:**
Run migrations synchronously at daemon startup, before the axum router is bound to the Unix socket. Use a simple `include_str!("migrations/001_init.sql")` pattern — no migration framework needed for Speck's scale. Confirm migration success before calling `axum::serve(listener, router).await`.

### Pitfall 2-D: Multiple daemon instances write the same SQLite file, corrupting state

**What goes wrong:**
If `spk up` is called twice in rapid succession (race between launchd and the user's shell), two daemon processes start and both open the same SQLite file. With default journal mode, the second writer will see `SQLITE_BUSY` errors on every write and either silently discard state or crash.

**Prevention:**
Open the SQLite file with `PRAGMA journal_mode=WAL` and `PRAGMA busy_timeout=5000`. WAL mode allows concurrent readers and one writer, and `busy_timeout` retries instead of returning `SQLITE_BUSY` immediately. Additionally, the `spk up` lock file (already used for the PID file) should gate on a lock before opening the DB connection.

### Pitfall 2-E: Rehydrating exec state from SQLite on restart creates stale "running" entries

**What goes wrong:**
If a container exec was in-flight when the daemon died, its `ExecSpec.running = true` is persisted to SQLite. On restart, the exec row is reloaded and the Docker API reports it as still running. `docker exec inspect` returns `Running: true` indefinitely, breaking testcontainers' `execInContainer` wait logic.

**Prevention:**
On startup, after loading persisted exec state, set all `running = true` entries to `running = false` with `exit_code = -1`. This is the same strategy Docker Engine uses: it marks in-flight execs as `ExitCode: -1, Running: false` after a restart. Log a `tracing::warn!` for each entry repaired so the operator can see it happened.

---

## Area 3: Apple Developer ID Notarization

### Pitfall 3-A: Binary signed without `--options runtime` — notarization rejects immediately

**What goes wrong:**
Notarization requires the Hardened Runtime to be enabled. Signing without `--options runtime` produces a valid signature that passes `codesign -v` locally, but `notarytool submit` returns a rejection: "The executable does not have the Hardened Runtime enabled." No log is needed — the error is in the submission response.

**Prevention:**
All `codesign` calls in `xtask/` must include `--options runtime`. The entitlements plist must be passed explicitly so the hardened runtime exceptions (only `com.apple.security.virtualization` is needed) are embedded alongside the runtime flag.
```bash
codesign \
  --sign "Developer ID Application: ..." \
  --options runtime \
  --entitlements entitlements.plist \
  --timestamp \
  --force \
  /path/to/spk
```

### Pitfall 3-B: `com.apple.security.virtualization` entitlement stripped or malformed

**What goes wrong:**
Two failure modes:

1. The entitlement is absent from the plist passed to `codesign`. The binary signs and notarizes successfully but silently, then fails at **runtime** with an opaque `SIGKILL` (no error message, no log entry) when it tries to create a `VZVirtualMachine`. macOS kills the process without explanation when a required entitlement is missing.

2. The entitlement value is a string (`<string>true</string>`) instead of a boolean (`<true/>`). The `codesign` tool accepts it, but the kernel's entitlement check fails at runtime with the same silent SIGKILL.

**Why it happens:**
Entitlements files edited manually are prone to type errors. The boolean-vs-string distinction is not caught by any linter.

**Prevention:**
- Entitlements plist must contain exactly `<key>com.apple.security.virtualization</key><true/>`.
- After signing, always verify: `codesign -d --entitlements :- ./spk | grep virtualization`. This must print `com.apple.security.virtualization` with the boolean value.
- Add this verification as a step in `xtask dist` that fails the build if the entitlement is absent or has the wrong type.

### Pitfall 3-C: `notarytool submit` exits 0 on failure — must check the output, not the exit code

**What goes wrong:**
`xcrun notarytool submit ... --wait` exits with status 0 even when notarization is rejected. CI scripts that check only `$?` will see success, tag the release, and ship a non-notarized binary that Gatekeeper blocks on user machines.

**Why it happens:**
`notarytool` separates "submission succeeded" (process ran without error) from "notarization passed" (Apple approved the submission). Both map to exit code 0.

**Prevention:**
Parse the JSON output from `notarytool submit --output-format json`. Check the `status` field — it must be `"Accepted"`. Any other value (`"Rejected"`, `"Invalid"`) is a failure. In shell:
```bash
STATUS=$(xcrun notarytool submit spk.tar.gz \
  --keychain-profile speck-notarize \
  --wait \
  --output-format json | jq -r '.status')
[ "$STATUS" = "Accepted" ] || { echo "Notarization failed: $STATUS"; exit 1; }
```

### Pitfall 3-D: Packaging with `zip -qr` instead of `ditto` breaks quarantine handling

**What goes wrong:**
`zip -qr spk.zip spk` creates an archive that, when extracted on the user's machine, loses the extended attributes needed for quarantine propagation. Gatekeeper may fail to verify the notarization ticket even though the binary is stapled. The symptom is "Apple cannot verify this software" dialogs on user machines.

**Prevention:**
Create the distribution archive with `ditto -c -k --keepParent spk spk.tar.gz` (or with `tar` preserving extended attributes). For Homebrew distribution specifically, create a tarball matching what the Formula/Cask `url` field points to. The Homebrew cask `sha256` must match this exact artifact.

### Pitfall 3-E: Forgetting to staple — notarization ticket not attached to the binary

**What goes wrong:**
After `notarytool submit` succeeds, the ticket exists on Apple's CDN. Without stapling, offline machines or machines that have never seen the binary before must contact Apple's OCSP/Gatekeeper servers to verify it. `xcrun stapler staple` embeds the ticket into the binary so verification works offline. If stapling is skipped, the binary works for most users but fails for users with restricted network access (corporate proxies, air-gapped machines).

**Prevention:**
```bash
xcrun stapler staple ./spk
xcrun stapler validate ./spk  # Must print "The validate action worked!"
```
Run staple on the binary **before** creating the distribution tarball. The tarball must contain the stapled binary, not the pre-stapled one. If you staple after creating the tarball, users who extract the tarball get an unstapled binary — rebuild the tarball after stapling.

### Pitfall 3-F: Homebrew re-sign stripping the virtualization entitlement

**What goes wrong:**
Homebrew re-signs binaries during installation. If `brew install` uses `codesign` without preserving entitlements, the `com.apple.security.virtualization` entitlement is removed during re-sign. The binary installs successfully but fails at runtime with a silent SIGKILL.

**Prevention:**
Homebrew's re-sign step uses `codesign --preserve-metadata=entitlements` when the cask specifies it. For a Formula (not Cask), Homebrew re-signs with `--preserve-metadata=entitlements,identifier,flags` by default as of Homebrew 4.x. Verify after installation: `codesign -d --entitlements :- $(which spk)` must show `com.apple.security.virtualization`. If it doesn't, the Formula needs a `bottle do ... end` block that explicitly lists required entitlements or the cask should be used instead of a formula for binary distribution.

### Pitfall 3-G: `notarytool --wait` hangs indefinitely — Apple service degradation

**What goes wrong:**
Apple's notarization service is occasionally slow or degraded. `notarytool submit --wait` has been known to hang for 30+ hours on first submissions or during service outages. CI jobs time out, leaving submissions in an ambiguous "In Progress" state. Re-submitting while the original is in progress creates duplicate submissions.

**Prevention:**
- Set a CI timeout: `timeout 1800 xcrun notarytool submit ... --wait || xcrun notarytool wait "$SUBMISSION_ID"`.
- Save the submission ID from the initial submit response so you can poll separately if `--wait` times out.
- Do not re-submit if the original submission is still "In Progress" — check with `notarytool info <id>`.
- The first submission from a new Apple Developer team takes longer (Apple validates the team). Schedule an hour-long window for the first notarization.

---

## Area 4: `spk restart` Atomicity and Race Conditions

### Pitfall 4-A: Starting a new VM before the old one fully stops — `do_start` returns `AlreadyRunning`

**What goes wrong:**
`do_stop` calls `stopWithCompletionHandler` which is asynchronous: it returns immediately and delivers the result via a callback block (which signals `done_tx`). If the implementation of `spk restart` calls `Guest::stop()` and immediately calls `Guest::start()` without waiting for `InternalState::Stopped`, the `VmThread` returns `Error::AlreadyRunning` from `do_start` because `ctrl.state` is still `InternalState::Stopping`.

**Why it happens:**
Looking at `vm_thread.rs:719-780`: `do_stop` does wait for the completion handler via `done_rx.recv_timeout(stop_timeout)`. However, any restart code that bypasses this — e.g. sending `VmCommand::Start` immediately after `VmCommand::Stop` without awaiting the reply — will race against the async completion.

**Prevention:**
`spk restart` must be implemented as:
1. Send `VmCommand::Stop` and await the reply channel (`done_rx.recv()`).
2. Only after `Ok(InternalState::Stopped)` is received, send `VmCommand::Start`.
Never fire-and-forget the stop command. The axum server must remain live during the entire sequence; only `AppState.guest` needs to be in a transitional state.

### Pitfall 4-B: Tokio tasks holding stale vsock fds from the stopped VM

**What goes wrong:**
When a VM stops, the `VZVirtioSocketDevice` closes all vsock connections. The DNS proxy (`spawn_dns_proxy`) holds a `RawFd` for the vsock connection; when the VM dies, `read_exact_fd` returns 0 (EOF) and the proxy thread exits cleanly. However, the `tokio::task::JoinHandle` for the proxy task is owned by the caller and must be `await`-ed before the new VM starts — otherwise the `JoinHandle` is dropped, the task is detached, and the OS fd number may be reused by the new VM's vsock connection while the old task is still (briefly) running.

The `netstack_fd` and `dns_vsock_fd` stored in `VmControl` are `Option<RawFd>`. On stop, these are not explicitly closed by `do_stop` — they are raw file descriptors that become invalid when the VM socket device closes. If the port map loop or the DNS proxy loop are still reading from a raw fd when it is closed and a new fd with the same number is opened for the new VM, data from the new connection could be misinterpreted by the stale loop.

**Prevention:**
- Track all `JoinHandle`s spawned for VM-lifetime tasks (DNS proxy, port map loop, net stack thread) in a `Vec<JoinHandle>` on the host side, associated with the current VM lifecycle.
- On `Guest::stop()` completion, explicitly join (or cancel with a `CancellationToken`) all VM-lifetime task handles before returning `Stopped`.
- Explicitly close `netstack_fd` and `dns_vsock_fd` raw fds after the VM confirms stopped state and before starting a new VM.

### Pitfall 4-C: AppState's gRPC channel pointing at dead vsock proxy after restart

**What goes wrong:**
`AppState::containerd_client()` caches the containerd Unix proxy socket path in `containerd_proxy: Arc<tokio::sync::Mutex<Option<PathBuf>>>`. On the first call after VM start, it creates the Unix socket proxy. On restart, the old proxy path is stale (the old vsock proxy was part of the stopped VM's lifecycle). The next Docker API request after restart calls `containerd_client()`, the path is still cached as `Some(old_path)`, and the returned `ContainerdClient` fails to connect — returning 500 errors to the user until the cache is manually cleared.

**Prevention:**
On `VmCommand::Stop` completion, reset `AppState.containerd_proxy` to `None`:
```rust
*state.containerd_proxy.lock().await = None;
```
This forces `containerd_path()` to re-establish the proxy via `guest.containerd_unix_proxy()` on the next request after the new VM is running. No further changes needed because the lazy-init pattern in `containerd_path()` is already correct.

### Pitfall 4-D: GCD serial queue is shared across VM lifetimes — don't re-create it

**What goes wrong:**
The `DispatchQueue` created in `VmThread::spawn()` is a single serial queue for the entire `VmThread` lifetime. A naive restart implementation might create a new `DispatchQueue` per VM lifecycle, but this is incorrect: if any prior `queue.exec_sync` call is still in flight (a block that hasn't returned yet), creating a new queue and sending work on it can result in two GCD queues each processing VZ callbacks concurrently — violating the "all VZ calls on one serial queue" invariant.

**Prevention:**
The existing design is correct: one `DispatchQueue` per `VmThread`, shared across all Start/Stop cycles. The `do_stop` path takes the queue as `&DispatchQueue` and uses `exec_sync`, which drains the queue before returning. Do not create new queues in restart implementations. Do not replace the `VmThread` with a new one on restart — reuse the existing `VmThread` by sending `VmCommand::Stop` then `VmCommand::Start`.

### Pitfall 4-E: Docker API socket stays open during restart — requests arriving mid-restart return 503-level errors

**What goes wrong:**
`spk restart` that stops the VM, does cleanup, then starts the new VM has a window (typically 2-5 seconds) where Docker API requests succeed at the HTTP layer (the axum socket is still open) but fail internally because the guest is stopped. A testcontainers client calling `GET /containers/json` during this window gets a 500 with "VM not running" — which testcontainers interprets as a broken runtime and aborts the test.

**Prevention:**
During the restart window, Docker API handlers that require guest connectivity should return `503 Service Unavailable` with `Retry-After: 5` instead of 500. This is semantically correct (the service is temporarily unavailable) and HTTP clients will retry. Add a `VmState::Restarting` state that handlers can check. Do not return 500 — 503 is retriable, 500 is not (by most clients).

---

## Area 5: VZVirtioConsoleDeviceConfiguration (CONSOLE-01)

### Pitfall 5-A: Pipe buffer fills up — guest kernel write to /dev/hvc0 blocks, VM stalls

**What goes wrong:**
macOS default pipe buffer is 65,536 bytes (64 KB). When `VZFileHandleSerialPortAttachment` is configured with a pipe, the guest kernel writes serial console output to the pipe. If the consumer (the Rust code reading from the pipe's read end) does not drain the pipe fast enough, the pipe buffer fills. The guest kernel's `hvc_console_write()` call then **blocks** — and because the HVC console driver runs in a kernel context, this can stall the entire VM boot sequence. The symptom is the VM appearing to hang during boot with no error in the Speck log.

**Why it happens:**
Guest kernel boot produces a burst of console output (dmesg, initrd log) — easily 10-50 KB in the first 500ms. If the reader is started lazily (after the VM is "ready") rather than immediately at VM start, the burst fills the pipe before the reader is attached.

**Prevention:**
- Start the pipe reader **before** calling `VZVirtualMachine.start()` (i.e., during `do_start`, before the `startWithCompletionHandler` call).
- The reader must be a dedicated `std::thread` (not a tokio async task) because `read()` on a pipe is a blocking syscall. Use `std::thread::Builder::new().name("speck-console").spawn(...)`.
- Write to a `BufWriter<File>` around the log file. This avoids one `write()` syscall per byte.
- If you use `VZFileHandleSerialPortAttachment` with `fileHandleForWriting` pointing at the pipe write end (guest output), ensure the **read** end is consumed continuously on this dedicated thread. Never open the write end of the pipe in the Rust process (only the framework holds it).

### Pitfall 5-B: Using `VZVirtioConsolePortConfiguration` with `isConsole = false` — kernel boot messages are silently dropped

**What goes wrong:**
`VZVirtioConsoleDeviceConfiguration` has two types of ports: console ports (`isConsole = true`, typically `/dev/hvc0` in the guest) and regular virtio console ports (`isConsole = false`, `/dev/hvc1` and up). The Linux kernel's `hvc_console` driver only binds to the port marked as the console. If `isConsole = false`, the kernel still creates the device but does not route `printk` output to it. You see no boot messages and cannot diagnose VM startup failures.

**Prevention:**
Set `isConsole = true` on the `VZVirtioConsolePortConfiguration` that should receive kernel console output. In `objc2-virtualization`:
```rust
let port_config = VZVirtioConsolePortConfiguration::new();
port_config.setIsConsole(true);
port_config.setAttachment(Some(&attachment));
```
Verify by checking the guest's `/proc/consoles` — it should list `hvc0` with the `C` (console) flag.

### Pitfall 5-C: `nil` attachment — device appears, data is silently discarded

**What goes wrong:**
If the `attachment` property of `VZVirtioConsolePortConfiguration` is not set (nil), the framework still creates the console device in the guest. The guest kernel writes to `/dev/hvc0` normally. From the Rust side, no data is ever received — the writes succeed on the guest side but go nowhere on the host side. The device appears to work (guest sees no error), but `console.log` is never written.

**Why it happens:**
In `objc2-virtualization`, the property is optional at the type level (`Option<&VZSerialPortAttachment>`). Forgetting to set it compiles and runs without error.

**Prevention:**
Assert the attachment is set in `do_start` before calling `validateWithError:`:
```rust
debug_assert!(
    port_config.attachment().is_some(),
    "console port attachment must be set before VM start"
);
```
For `VZFileSerialPortAttachment` (write-only, simpler option): pass the path to `console.log` directly and skip the pipe entirely. The framework opens the file and writes all guest console output there. This is the simpler approach for capture-only use cases where stdin to the guest is not needed.

### Pitfall 5-D: Console device added AFTER VM starts — framework ignores it

**What goes wrong:**
`VZVirtualMachineConfiguration` is immutable after `VZVirtualMachine` is created. Any console device configuration added after `initWithConfiguration:` returns is silently ignored. The VM runs without a console device.

**Prevention:**
Console device configuration must be complete in `do_start` before calling `VZVirtualMachineConfiguration.setConsoleDevices(...)` and then `VZVirtualMachine.initWithConfiguration(config)`. The existing `do_start` in `vm_thread.rs` already follows this pattern for storage and network devices — console must follow the same ordering.

### Pitfall 5-E: Reading from NSFileHandle in the Rust thread via blocking I/O — tokio runtime starvation

**What goes wrong:**
`VZFileHandleSerialPortAttachment` produces an `NSFileHandle`. If the Rust code reads from the associated file descriptor using `tokio::io::AsyncRead` on a tokio task (not a dedicated thread), the fd read is registered with tokio's reactor. Tokio's reactor uses `kqueue` on macOS. **Pipe fds are not supported by `kqueue` for edge-triggered readiness on all macOS versions.** Older macOS versions (pre-12.x) silently never wake the kqueue waiter for pipe reads, causing the task to hang indefinitely.

**Prevention:**
Read the console pipe on a `std::thread` with blocking `read()` calls, not in a tokio async task. Use `tokio::sync::mpsc` or a `std::sync::mpsc` channel to forward captured log lines to an async context if needed. This is the same pattern used by `spawn_dns_proxy` in `speck-net/src/dns.rs` — it explicitly runs on a `spawn_blocking` thread rather than an async task for the same reason (raw fd blocking I/O).

---

## Phase-Specific Warning Table

| Phase Topic | Feature | Likely Pitfall | Mitigation |
|-------------|---------|---------------|------------|
| DNS-TEST-01 | DNS proxy unit tests | getaddrinfo not mockable directly | Extract `trait Resolver`, inject in tests |
| DNS-TEST-01 | DNS proxy unit tests | Parallel test global state pollution | Use cargo nextest (process-per-test) |
| VMINIT-TEST-01 | vminitd unit tests | unsafe libc calls not testable in host process | Use conditional compilation stubs; test logic separately from syscall layer |
| CONSOLE-01 | Serial console capture | Pipe buffer fills during boot burst | Start reader thread before VM.start(); use dedicated std::thread |
| CONSOLE-01 | Serial console capture | kqueue + pipe on older macOS | Use blocking std::thread, not tokio AsyncFd |
| STATE-01/02 | SQLite persistence | rusqlite blocks tokio workers | Use tokio-rusqlite; never call rusqlite directly from async handlers |
| STATE-01/02 | SQLite persistence | Lock held across .await deadlocks | Clone data out of lock before calling DB |
| CONFORM-01 | Docker API conformance | exec state rehydrated as "running" | Reset all running execs to exit_code=-1 on startup |
| BREW-DEVID-01 | Developer ID signing | Missing --options runtime → notarization rejects | Always sign with --options runtime + entitlements plist |
| BREW-DEVID-01 | Developer ID signing | Entitlement boolean vs string type | Verify with `codesign -d --entitlements :-` after every sign |
| BREW-DEVID-01 | Developer ID signing | notarytool exits 0 on rejection | Parse JSON output, check `.status == "Accepted"` |
| BREW-DEVID-02/03 | .pkg / Cask | Homebrew re-sign strips entitlement | Verify entitlement post-install; use --preserve-metadata=entitlements |
| BREW-DEVID-02/03 | .pkg / Cask | zip vs ditto for archive | Always use ditto -c -k --keepParent for notarizable archives |
| DAEMON-RESTART | spk restart | Start issued before stop completes | Await stop reply before sending start command |
| DAEMON-RESTART | spk restart | Stale tokio tasks holding old vsock fds | Join all VM-lifetime task handles before starting new VM |
| DAEMON-RESTART | spk restart | containerd_proxy path cached after stop | Reset containerd_proxy to None on stop |
| DAEMON-RESTART | spk restart | 500 during restart window breaks testcontainers | Return 503 + Retry-After during VmState::Restarting |

---

## Sources

- Apple Developer Documentation: [Resolving common notarization issues](https://developer.apple.com/documentation/security/resolving-common-notarization-issues) — HIGH confidence
- Apple Developer Documentation: [Hardened Runtime](https://developer.apple.com/documentation/security/hardened-runtime) — HIGH confidence
- Apple Developer Documentation: [com.apple.security.virtualization entitlement](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.security.virtualization) — HIGH confidence
- Apple Developer Forums: [Virtual serial ports thread 758354](https://developer.apple.com/forums/thread/758354) — HIGH confidence — VZVirtioConsoleDeviceConfiguration isConsole pitfalls
- rsms/gist: [macOS distribution — code signing, notarization, quarantine](https://gist.github.com/rsms/929c9c2fec231f0cf843a1a746a416f5) — HIGH confidence — ditto vs zip, staple workflow, signing order
- tokio-rs/axum Discussion #964: [How to share SQLite connection?](https://github.com/tokio-rs/axum/discussions/964) — HIGH confidence — rusqlite parallelism limitation, tokio-rusqlite recommendation
- tokio-rs/axum Discussion #2629: [Deadlock when using same shared state](https://github.com/tokio-rs/axum/discussions/2629) — HIGH confidence — MutexGuard across .await deadlock pattern
- docs.rs/tokio-rusqlite — HIGH confidence — async wrapper design, connection-per-channel pattern
- cargo-nextest docs: [Environment variables](https://nexte.st/docs/configuration/env-vars/) — HIGH confidence — process-per-test isolation
- DeepWiki apple/containerization: [VM lifecycle management](https://deepwiki.com/apple/containerization/2.3-virtual-machine-management) — MEDIUM confidence — stop ordering, AsyncLock pattern
- github.com/Code-Hex/vz serial_console.go — MEDIUM confidence — FileHandleSerialPortAttachment vs FileSerialPortAttachment design
- speck-vz/src/vm_thread.rs source analysis — HIGH confidence — do_stop implementation, VmControl state machine
- speck-net/src/dns.rs source analysis — HIGH confidence — resolve_dns direct getaddrinfo call, ResolverTable watch::Receiver injection point
- speck-dockerd/src/state.rs source analysis — HIGH confidence — AppState field layout, containerd_proxy lazy-init pattern
