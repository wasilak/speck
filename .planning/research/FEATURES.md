# Feature Research — v1.2 Hardened Runtime

**Domain:** Stability/testing hardening, testcontainers conformance, Developer ID distribution, daemon lifecycle polish for an existing Rust container runtime on Apple Silicon.
**Researched:** 2026-07-06
**Confidence:** HIGH for testcontainers API surface (bollard source + deepwiki confirmed), HIGH for notarytool workflow (Apple docs + confirmed primary source), MEDIUM for serial console API (Go-vz mirror + objc2-virtualization docs — no direct Rust example found), HIGH for daemon restart semantics (Docker live-restore docs + OrbStack confirmed).

---

## Feature Area 1: testcontainers Conformance (CONFORM-01)

### What testcontainers-rs actually calls

testcontainers-rs wraps `bollard` and exercises the following Docker API endpoints. These are the specific HTTP method + path translations from bollard's source:

| bollard method | Docker Engine API endpoint | Correct response must include |
|---|---|---|
| `create_container` | `POST /containers/create` | `{Id: string, Warnings: []}` |
| `start_container` | `POST /containers/{id}/start` | HTTP 204 |
| `inspect_container` | `GET /containers/{id}/json` | `State.Running`, `State.ExitCode`, `NetworkSettings.Ports`, `NetworkSettings.Networks`, `HostConfig.NetworkMode` (see critical fields below) |
| `stop_container` | `POST /containers/{id}/stop` | HTTP 204 |
| `remove_container` | `DELETE /containers/{id}` | HTTP 204; called with `?v=1&force=1` |
| `pause_container` | `POST /containers/{id}/pause` | HTTP 204 |
| `unpause_container` | `POST /containers/{id}/unpause` | HTTP 204 |
| `list_containers` | `GET /containers/json` | Array with label-filter support |
| `logs` | `GET /containers/{id}/logs` | Multiplexed Docker stream (8-byte frame header) |
| `create_exec` | `POST /containers/{id}/exec` | `{Id: string}` |
| `start_exec` | `POST /exec/{id}/start` | Hijacked connection (HTTP 200 upgrade) |
| `inspect_exec` | `GET /exec/{id}/json` | `{Running: bool, ExitCode: int}` |
| `create_network` | `POST /networks/create` | `{Id: string, Warning: string}` |
| `inspect_network` | `GET /networks/{id}` | `{IPAM.Config[0].Gateway, IPAM.Config[0].Subnet}` |
| `list_networks` | `GET /networks` | Array; used for existence checks by name |
| `remove_network` | `DELETE /networks/{id}` | HTTP 204 |
| `create_image` | `POST /images/create?fromImage=X&tag=Y` | Streaming JSON-stream lines `{"status":…}` |
| `inspect_image` | `GET /images/{name}/json` | Standard image inspect fields |
| `remove_image` | `DELETE /images/{name}` | HTTP 200 with deleted list |
| `build_image` | `POST /build` | Streaming JSON-stream |
| `upload_to_container` | `PUT /containers/{id}/archive?path=X` | HTTP 200 (tar body upload) |
| `download_from_container` | `GET /containers/{id}/archive?path=X` | Tar stream |

The system endpoints are also required:
- `GET /_ping` — must return HTTP 200, the `Api-Version` header is used for version negotiation
- `GET /version` — must include `ApiVersion` as a string (e.g. `"1.44"`); bollard targets ~1.40, returning any valid ≥1.40 value is fine

### Critical response field details

**`GET /containers/{id}/json` is the most load-bearing endpoint.** testcontainers-rs polls it (not `wait_container` events) to determine readiness.

Port mapping — `get_host_port_ipv4(containerPort)` reads:
```json
{
  "NetworkSettings": {
    "Ports": {
      "6379/tcp": [
        { "HostIp": "0.0.0.0", "HostPort": "8080" }
      ]
    }
  }
}
```
`HostPort` must be a string (not an integer). `HostIp` must be `"0.0.0.0"` for the primary IPv4 binding. An empty `Ports` map or null entry causes `get_host_port_ipv4` to return an error.

Bridge IP — `get_bridge_ip_address()` reads:
```json
{
  "NetworkSettings": {
    "Networks": {
      "bridge": { "IPAddress": "172.17.0.2" }
    }
  },
  "HostConfig": { "NetworkMode": "bridge" }
}
```

State polling — `is_running()` and `exit_code()` read:
```json
{
  "State": { "Running": true, "ExitCode": 0 }
}
```
After `start_container`, `State.Running` must be `true` before the next inspect poll or testcontainers' wait loop will time out.

**Log multiplexing format** — testcontainers-rs uses `WaitFor::message_on_stdout(...)` which reads the log stream. The Docker multiplexed stream format is required (not raw bytes):
```
[stream_type: 1 byte][0x00 0x00 0x00: 3 bytes][frame_size: 4 bytes big-endian][frame_data]
```
stream_type: `0x01` = stdout, `0x02` = stderr. Getting this wrong causes all log-line wait strategies to hang or fail.

**Exec response** — after `create_exec` returns an exec ID, `start_exec` must hijack the HTTP connection (Upgrade to TCP stream). `inspect_exec` after exec completes must return `Running: false` with a valid exit code.

### Table stakes for testcontainers conformance

| Capability | Why required | Complexity | Status in speck-dockerd |
|---|---|---|---|
| Correct `inspect_container` port mapping | Every `get_host_port_ipv4` call fails without it | LOW (data marshaling) | Partial — needs audit |
| Correct log multiplexing format | Log-based wait strategies fail silently | LOW | Unknown — needs verification |
| Network IPAM in `inspect_network` | testcontainers creates named networks for container isolation | LOW | Stub — needs implementation |
| `PUT /containers/{id}/archive` (tar upload) | testcontainers copies files into containers | MEDIUM | Stub |
| `GET /containers/{id}/archive` (tar download) | testcontainers reads files from containers | MEDIUM | Stub |
| `State.Running = true` immediately after `start` | Wait loop polls within milliseconds of start | LOW | Needs timing verification |
| `GET /version` returns parseable `ApiVersion` | bollard negotiates version on connect | LOW | Likely implemented, needs confirmation |

### Differentiators (beyond minimum viable)

| Capability | Value | Complexity |
|---|---|---|
| `POST /containers/{id}/wait?condition=not-running` | Some clients use event-based wait instead of polling; also needed for compose | MEDIUM |
| `State.Health.Status` in inspect | Supports `WaitFor::healthcheck()` strategy | MEDIUM |
| Full label filtering on `GET /containers/json` | Testcontainers module community uses label selectors | LOW |

### Anti-features for testcontainers conformance

**Do NOT implement Ryuk/reaper support.** testcontainers-rs does NOT use Ryuk by default — cleanup is RAII (Drop calls `stop_container` + `remove_container`). Implementing Docker socket mounting into containers for Ryuk adds complexity and security surface without any benefit for testcontainers-rs.

**Do NOT implement `GET /events` streaming just for testcontainers.** testcontainers-rs doesn't call it. Compose and Portainer do; that's a later concern.

**Do NOT stub out `inspect_container` port fields with empty values.** Returning `"Ports": {}` is worse than returning correct data — it fails silently and testcontainers hangs for the full startup timeout before failing.

---

## Feature Area 2: Docker State Persistence (STATE-01, STATE-02)

### The architectural difference from real dockerd

Speck's `speck-dockerd` is a translation shim, not moby/dockerd. Containers actually live inside containerd in the guest VM. This changes what "persistence" means:

**What containerd already persists (no work needed):**
- Container IDs and task state (containerd has its own BoltDB in the guest data disk)
- Image layers and snapshots
- Container filesystem state (overlayfs in guest)
- Named volumes (if backed by guest storage paths)

**What speck-dockerd must persist (work needed):**
- Network metadata: name → ID, IPAM config (CIDR, gateway) — lives only in `AppState` in-memory; docker-compose and testcontainers create named networks and look them up by name after daemon restart
- Volume metadata: name → guest path, driver — `AppState` maps Docker volume names to guest paths; these must survive restart for compose named volumes to work
- Container-to-network assignments: which containers are on which networks — needed for `inspect_network` to return `Containers` field correctly

**The failure mode for testcontainers without STATE-01:**
1. `spk up` starts daemon + VM
2. Testcontainers test starts: creates container → `spk restart` (e.g. config change) → daemon loses in-memory state
3. Container still alive in containerd, but speck-dockerd returns 404 → testcontainers fails to remove, leaks container
4. Next test run finds orphaned container causing port conflicts

The more common failure is compose-based: named volumes defined in `docker-compose.yml` are gone after daemon restart because speck-dockerd lost the name → path mapping.

### Table stakes for state persistence

| State item | File path | When to write | Notes |
|---|---|---|---|
| Networks | `$SPECK_HOME/state/networks.json` | On every `POST /networks/create` and `DELETE /networks/{id}` | Contains name, ID, IPAM config, driver |
| Volumes | `$SPECK_HOME/state/volumes.json` | On every `POST /volumes/create` and `DELETE /volumes/{id}` | Contains name, guest path, driver, labels |
| Container metadata rebuild | On daemon startup | Query containerd for all containers, rebuild inspect cache | Required for consistent Docker API responses after restart |

**Minimum viable for testcontainers:** STATE-02 (network + volume persistence) is more critical than STATE-01 (container rebuild) for testcontainers workflows. testcontainers creates and destroys containers within a single test session — it doesn't expect containers to survive restart. What it does expect: named networks it creates to be removable without 404 errors.

**Format recommendation:** JSON files using serde_json with atomic writes (write to `.tmp` then rename). Simple enough to inspect/debug manually. No SQLite, no complex schema.

### Differentiators

| Capability | Value | Complexity |
|---|---|---|
| Container restart policies honored on `spk up` | `--restart=always` containers auto-start after VM restart | MEDIUM — requires reading policy from persisted metadata + calling containerd start |
| State checksum validation on load | Detect corrupted state and recover gracefully | LOW |

### Anti-features for state persistence

**Do NOT implement a full dockerd-style `/var/lib/docker/containers/{id}/config.v2.json` schema.** That schema has 80+ fields and is moby-specific. Speck only needs the fields it actually uses in API responses.

**Do NOT write state synchronously in the hot path** for every request that reads state. Write-on-mutate, read-on-startup. Never lock on reads.

**Do NOT persist exec state.** Exec sessions are ephemeral — a restarted daemon will never see their clients again. Clean up orphaned exec entries from containerd's perspective on startup.

---

## Feature Area 3: Apple Developer ID Distribution (BREW-DEVID-01/02/03)

### Prerequisites (not optional)

- Active Apple Developer Program membership ($99/year, apple.com/developer)
- Developer ID Application certificate in macOS Keychain (issued by Apple CA, tied to your Team ID)
- Developer ID Installer certificate (for `.pkg` — BREW-DEVID-02)
- App-specific password from appleid.apple.com (or App Store Connect API key — preferred for CI)

### Step-by-step workflow (BREW-DEVID-01 — binary signing + notarization)

**1. Build:**
```bash
cargo build --release --target aarch64-apple-darwin
```

**2. Create entitlements.plist** (must include virtualization entitlement):
```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" ...>
<plist version="1.0"><dict>
  <key>com.apple.security.virtualization</key><true/>
</dict></plist>
```
Without `com.apple.security.virtualization`, `VZVirtualMachine` creation fails at runtime regardless of signing. This entitlement is already required — it's what makes the current ad-hoc signed build work.

**3. Code sign with Developer ID + Hardened Runtime:**
```bash
codesign -f \
  -s "Developer ID Application: Your Name (TEAMID)" \
  -o runtime \
  --entitlements entitlements.plist \
  target/aarch64-apple-darwin/release/spk
```
`-o runtime` (Hardened Runtime) is required for notarization. Without it, notarytool rejects the submission.

**4. Store credentials (one-time setup):**
```bash
xcrun notarytool store-credentials "speck-notary" \
  --apple-id your@email.com \
  --team-id TEAMID \
  --password APP_SPECIFIC_PASSWORD
```
Or use an App Store Connect API key (better for CI, no 2FA dependency).

**5. Archive for submission (use ditto, not zip):**
```bash
ditto -c -k --keepParent spk spk.zip
```
`zip` loses extended attributes required for notarization; `ditto` preserves them.

**6. Submit for notarization:**
```bash
xcrun notarytool submit spk.zip \
  --keychain-profile "speck-notary" \
  --wait
```
`--wait` polls until Apple's CDN completes processing (usually 30 seconds–2 minutes). CRITICAL: notarytool exits 0 on failure. Always inspect the output for `status: Accepted` vs `status: Invalid`.

**7. On failure — fetch log:**
```bash
xcrun notarytool log SUBMISSION_ID \
  --keychain-profile "speck-notary" \
  notarytool-log.json
```
Common failure: Hardened Runtime not enabled, or missing entitlements, or binary contains unsigned third-party frameworks (none for Speck — it's a self-contained Rust binary).

**8. Verify locally:**
```bash
spctl -a -vvv -t exec ./spk
# Expected: source=Notarized Developer ID
```

**Note on stapling a bare binary:** Notarization tickets cannot be stapled directly to a bare binary (only to `.app`, `.pkg`, `.dmg`). For a bare binary distributed via zip, Gatekeeper verifies the ticket via online lookup. This works if the user has internet access at first launch. For offline-first guarantee, use BREW-DEVID-02.

### BREW-DEVID-02 — .pkg installer

```bash
# Create pkg root directory structure
mkdir -p pkg_root/usr/local/bin
cp spk pkg_root/usr/local/bin/

# Build and sign the pkg (Developer ID Installer cert — different from Application cert)
pkgbuild \
  --root pkg_root \
  --identifier io.speck.spk \
  --version 1.2.0 \
  --sign "Developer ID Installer: Your Name (TEAMID)" \
  speck-1.2.0.pkg

# Notarize the .pkg
xcrun notarytool submit speck-1.2.0.pkg \
  --keychain-profile "speck-notary" \
  --wait

# Staple the ticket directly to the .pkg
xcrun stapler staple speck-1.2.0.pkg

# Verify
spctl -a -vvv -t install speck-1.2.0.pkg
```

The `.pkg` format allows stapling — the notarization ticket is embedded in the file, enabling offline Gatekeeper verification. This is the right choice for a Homebrew Cask since Casks download the artifact and Gatekeeper checks it before installation.

### BREW-DEVID-03 — Homebrew Cask

**Formula vs Cask distinction:** Formulae compile from source. Casks distribute pre-built binaries. Speck ships a Developer-ID-signed Apple-Silicon binary — it must be a Cask.

**Cask ruby file skeleton:**
```ruby
cask "speck" do
  version "1.2.0"

  on_arm do
    url "https://github.com/your-org/speck/releases/download/v#{version}/speck-#{version}-aarch64-apple-darwin.pkg"
    sha256 "the-sha256-of-the-notarized-pkg"
  end

  name "Speck"
  desc "Ultra-fast container runtime for Apple Silicon"
  homepage "https://github.com/your-org/speck"

  binary "#{staged_path}/usr/local/bin/spk"
end
```

**SHA256 must be of the signed+notarized artifact**, not the raw binary. The hash changes after signing.

**The virtualization entitlement survives Homebrew re-signing:** Homebrew re-signs downloaded cask binaries with `codesign --preserve-metadata=entitlements`, which preserves the `com.apple.security.virtualization` entitlement. This is confirmed in STACK.md and PROJECT.md.

**Distribution path for v1.2:**
1. BREW-DEVID-01: sign + notarize the binary (unblocks dev adoption, removes "unverified developer" popup)
2. BREW-DEVID-02: wrap in a `.pkg` (enables stapling, offline Gatekeeper)
3. BREW-DEVID-03: submit Cask to homebrew/homebrew-cask or maintain own tap at `brew tap speck-io/speck`

### Table stakes for Developer ID distribution

| Step | Why | Complexity | Dependency |
|---|---|---|---|
| Developer ID Application cert | Without it, notarytool rejects | LOW (Apple portal) | Apple Developer Program |
| Hardened Runtime (`-o runtime`) | Required for notarytool | LOW | Must not conflict with existing entitlements |
| `com.apple.security.virtualization` in plist | Without it, VM creation fails at runtime | LOW | Already present in ad-hoc signing |
| `notarytool submit --wait` | Polling; must parse output not just exit code | LOW | Network access to Apple CDN |
| Stapling `.pkg` | Offline Gatekeeper verification | LOW | Requires `.pkg` wrapper (BREW-DEVID-02) |
| SHA256 in Cask from the signed artifact | Hash changes after signing — must re-generate | LOW | Ordered: sign → hash → write cask |

### Anti-features for Developer ID distribution

**Do NOT ad-hoc sign for distribution.** Ad-hoc signatures cannot carry a verifiable identity chain. Homebrew removes un-notarized casks from the official tap (~Sept 2026 per PROJECT.md). Any unsigned binary triggers Gatekeeper quarantine quarantine for end users.

**Do NOT use the `com.apple.vm.networking` bridged networking entitlement.** It requires Apple review approval and is not needed for Speck's architecture. Keep the minimum entitlement surface.

**Do NOT automate CI notarization without App Store Connect API key.** Apple ID + password in CI is fragile (2FA prompts, account lockouts). Use `xcrun notarytool store-credentials` with an API key from App Store Connect.

**Do NOT sign with the wrong certificate type.** The binary needs `Developer ID Application`; the `.pkg` needs `Developer ID Installer`. Using the wrong one causes notarytool rejection with a confusing error.

---

## Feature Area 4: Daemon Polish (DAEMON-RESTART, DAEMON-DOWN, PORT-E2E, EXEC-E2E)

### `spk restart` semantics

**What restart must preserve:**
- Named volumes (disk state in guest data disk — survives across VM restarts because the ext4 data disk is persistent)
- Named networks (from `$SPECK_HOME/state/networks.json` once STATE-02 is implemented)
- The launchd LaunchAgent plist (so `spk up` works after `spk restart`)
- CA cert configuration and DNS config

**What restart must reset (ephemeral by definition):**
- Exec sessions — TCP connections to exec processes die when VM stops
- Log streaming connections — per-request, not persistent
- In-memory AppState caches — must be rebuilt from disk + containerd on `spk up`
- Port forwarding state — re-established on each `spk up`

**Minimum viable implementation:** `spk restart` = `spk down` + `spk up`. This is correct semantics for v1.2. The VM must fully reboot because the daemon, VM, and containerd are tightly coupled.

**What happens to running containers:**
- Containers are stopped by `spk down` (vminitd stops → containerd stops → containers stop)
- Containerd's data disk persists (container filesystems and metadata survive)
- Containers with `--restart=always` should auto-start: currently they won't (STATE-01 must exist first to re-issue containerd start calls on VM boot)

**OrbStack comparison:** `orb restart docker` restarts just the Docker engine inside the Linux VM without rebooting the VM. This is more sophisticated (daemon-only restart) but not needed for v1.2 — the improvement is opt-in later once STATE-01 makes it safe.

### `spk down` reliability (DAEMON-DOWN)

The known bug: when Speck was started via `cargo run` (not via `launchctl bootstrap`), `spk down` cannot find the process because it uses launchctl to stop it, and the process isn't registered with launchd.

**Fix:** After trying launchctl, fall back to reading `$SPECK_HOME/run/speck.pid` and sending SIGTERM directly. This is already partially in place (PROJECT.md mentions PID file fallback). The fix is to make the fallback code path reliable.

**Table stakes:**
- `spk down` must work regardless of how the daemon was started
- `spk down` must wait for the VM to actually stop before returning
- `spk down` must clean up the PID file and control socket

### PORT-E2E and EXEC-E2E

**Port publishing end-to-end verification:**
- Test: `spk run -p 8080:80 nginx:alpine`
- Verify: `curl localhost:8080` returns 200 from the macOS host
- The critical path: `PortPublishBridge` in `speck-net` must accept TCP connections on the host port and forward them to the container port in smoltcp

**Exec end-to-end verification:**
- Test: `spk exec <container> echo hello`
- Verify: output "hello" returned to stdout
- The critical path: `POST /containers/{id}/exec` → containerd exec via vsock → `POST /exec/{id}/start` → hijacked HTTP → bidirectional byte bridge → `GET /exec/{id}/json` returns exit code 0

**These are verification tasks, not new implementations.** The code is already in place. The goal is to confirm it works and add regression tests.

### Table stakes for daemon polish

| Feature | Why | Complexity | Status |
|---|---|---|---|
| `spk restart` as `down + up` | Users expect restart to just work | LOW — compose existing commands | Not yet implemented |
| `spk down` PID fallback | Dev workflow requires non-launchd start to be stoppable | LOW — fix existing fallback | Known bug, fix scoped |
| PORT-E2E confirmed working | Port mapping is table stakes; unknown if it works end-to-end | LOW — test + fix smoltcp path | Unknown — needs e2e test |
| EXEC-E2E confirmed working | `exec` is used by testcontainers and `docker exec` | LOW — test + fix vsock path | Unknown — needs e2e test |

### Anti-features for daemon polish

**Do NOT implement daemon-only restart** (restart speck-dockerd without rebooting the VM) in v1.2. Without STATE-01/STATE-02, restarting the daemon alone means it loses all network/volume metadata but the VM still has containers running — a worse-than-useless state.

**Do NOT add a `--graceful-timeout` flag to `spk down`** in v1.2. Stopping containers gracefully before VM poweroff requires STATE-01 (knowing which containers exist). Implement graceful stop only once persistence exists.

---

## Feature Area 5: Serial Console Capture (CONSOLE-01)

### What this is

The Linux guest kernel and vminitd write boot diagnostics to the virtio console device (`hvc0`). Currently there is no way to read this output — boot failures are opaque. Wiring `VZVirtioConsoleDeviceSerialPortConfiguration` + `VZFileSerialPortAttachment` captures this output to `$SPECK_HOME/console.log`.

### The API chain in objc2-virtualization

Based on the Go-vz mirror of Apple's Virtualization.framework and objc2-virtualization docs:

**Class hierarchy:**
- `VZVirtioConsoleDeviceSerialPortConfiguration` inherits from `VZSerialPortConfiguration`
- `VZFileSerialPortAttachment` inherits from `VZSerialPortAttachment`
- `VZVirtualMachineConfiguration.setSerialPorts(NSArray<VZSerialPortConfiguration>)`

**Wire-up pattern:**
1. `VZVirtioConsoleDeviceSerialPortConfiguration::new()` — create serial port config
2. `VZFileSerialPortAttachment::initWithURL_append_error(url, should_append, &mut err)` — create file attachment
   - `url`: `NSURL` file URL to `$SPECK_HOME/console.log`
   - `should_append`: `false` (overwrite per boot — boot logs are small, don't accumulate)
3. `serial_config.setAttachment(Some(&file_attachment))` — wire attachment to port
4. `vm_config.setSerialPorts(&NSArray::from_slice(&[serial_config]))` — register on VM config

**Also required in kernel cmdline:** Add `console=hvc0` to `VZLinuxBootLoader.commandLine`. Without this, Linux doesn't write to the virtio serial device and the log file stays empty.

This is already in `speck-vz/src/config.rs` which lists `VZVirtioConsoleDeviceConfiguration` and `VZFileSerialPortAttachment` in the class coverage. The implementation gap is wiring them together and adding `console=hvc0` to the kernel cmdline.

### Table stakes for serial console capture

| Step | Complexity | Notes |
|---|---|---|
| Add `console=hvc0` to kernel cmdline | LOW | One-line change in `config.rs` cmdline builder |
| Create `VZFileSerialPortAttachment` pointing to `$SPECK_HOME/console.log` | LOW | objc2 FFI, follow existing patterns in `config.rs` |
| Register with `VZVirtualMachineConfiguration.setSerialPorts` | LOW | Same pattern as other device configs |
| Ensure `$SPECK_HOME` directory exists before VM boot | LOW | Already done for other paths |

### Differentiators

| Capability | Value | Complexity |
|---|---|---|
| `spk doctor` shows last 20 lines of `console.log` on boot failure | Dramatically improves debugging | LOW |
| `spk console` subcommand to tail the log live | Interactive debugging | MEDIUM (`tail -f` equivalent in Rust) |
| Guest version check (VERSION-01) | Parse a version string from console output or vsock at READY time | MEDIUM |

### Anti-features for serial console capture

**Do NOT use `VZFileHandleSerialPortAttachment`** for log capture. That class uses `read`/`write` file handles for bidirectional serial communication (interactive TTY piped to a host process). It's correct for interactive access. For write-to-file capture, `VZFileSerialPortAttachment` is the right class.

**Do NOT append across boots** (use `should_append: false`). Boot logs from the previous boot are irrelevant when the new boot is healthy. Appending would accumulate logs indefinitely and makes it unclear which boot a log line belongs to.

**Do NOT add `console=tty0` in addition to `console=hvc0`** unless there's a specific need for a framebuffer console. One console target is enough and keeps the cmdline minimal.

**Do NOT expose the console log at a predictable public path** beyond `$SPECK_HOME`. It's a diagnostic artifact, not user data.

---

## Feature Dependencies

```
testcontainers conformance (CONFORM-01)
  └── requires: inspect_container correct port/state/network fields
  └── requires: log multiplexing format correct
  └── requires: exec hijack working (PORT-E2E prerequisite)
  └── requires: STATE-02 for named networks (compose integration)

state persistence (STATE-01/02)
  └── enables: spk restart preserving named volumes/networks
  └── enables: container restart policies after VM reboot
  └── STATE-02 is a prerequisite for full docker-compose reliability

Developer ID distribution (BREW-DEVID-01/02/03)
  └── BREW-DEVID-01 (sign+notarize) is prerequisite for BREW-DEVID-02 and BREW-DEVID-03
  └── BREW-DEVID-02 (pkg) is prerequisite for proper stapling in BREW-DEVID-03 (Cask)
  └── No dependency on runtime features — pure distribution concern

spk restart (DAEMON-RESTART)
  └── requires: spk down reliable (DAEMON-DOWN fixed first)
  └── depends on: STATE-01/02 for meaningful state preservation
  └── The implementation is trivial (down + up) but only useful after persistence exists

serial console (CONSOLE-01)
  └── is independent — can be done at any point in VM config setup
  └── enables: VERSION-01 (guest version check)
  └── enables: better spk doctor diagnostics
```

---

## MVP Definition per Feature Area

### testcontainers conformance — minimum viable
1. `inspect_container` returns correct `NetworkSettings.Ports` (string HostPort, 0.0.0.0 HostIp)
2. `inspect_container` returns correct `State.Running` and `State.ExitCode`
3. Log stream uses Docker multiplexing frame format
4. `inspect_network` returns IPAM config
5. `PUT /containers/{id}/archive` accepts tar upload
6. `GET /containers/{id}/archive` returns tar stream

Everything else (health checks, `wait_container` events) is differentiator.

### state persistence — minimum viable
1. `$SPECK_HOME/state/networks.json` written on create/delete, read on startup
2. `$SPECK_HOME/state/volumes.json` written on create/delete, read on startup
3. On daemon startup, query containerd for existing containers to rebuild inspect cache

### Developer ID — minimum viable
1. Sign with Developer ID Application + Hardened Runtime + virtualization entitlement (BREW-DEVID-01)
2. Notarize with notarytool
3. Update Homebrew Formula to point to signed binary

BREW-DEVID-02 and BREW-DEVID-03 are the production distribution story; BREW-DEVID-01 alone unblocks trusted installation.

### daemon polish — minimum viable
1. `spk down` works via PID file fallback (DAEMON-DOWN)
2. `spk restart` implemented as `spk down && spk up` (DAEMON-RESTART)
3. PORT-E2E verified with an nginx container
4. EXEC-E2E verified with a simple exec command

### serial console — minimum viable
1. `console=hvc0` in kernel cmdline
2. `VZFileSerialPortAttachment` writing to `$SPECK_HOME/console.log`
3. Log visible after `spk up` — boot messages captured

---

## Sources

- testcontainers-rs bollard API calls: [deepwiki/testcontainers-rs/6](https://deepwiki.com/testcontainers/testcontainers-rs/6-docker-client-and-configuration), [deepwiki/testcontainers-rs/3.3](https://deepwiki.com/testcontainers/testcontainers-rs/3.3-container-state-and-inspection), confirmed against [testcontainers-rs/client.rs](https://github.com/testcontainers/testcontainers-rs/blob/main/testcontainers/src/core/client.rs) — HIGH
- Docker log multiplexing format: [Docker Engine API docs](https://docs.docker.com/engine/api/v1.44/#tag/Container/operation/ContainerLogs) — HIGH
- Ryuk not used by default in testcontainers-rs: confirmed no Ryuk code in client.rs; RAII cleanup only — HIGH
- Apple notarytool workflow: [developer.apple.com/documentation/security/notarizing-macos-software-before-distribution](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution), [rsms/macOS-distribution-gist](https://gist.github.com/rsms/929c9c2fec231f0cf843a1a746a416f5) — HIGH
- Homebrew Cask vs Formula: [goreleaser discussion #5563](https://github.com/goreleaser/goreleaser/discussions/5563) — MEDIUM
- Homebrew entitlement preservation: `--preserve-metadata=entitlements` — confirmed in STACK.md — HIGH
- VZFileSerialPortAttachment API: [Code-Hex/vz v3 Go mirror](https://pkg.go.dev/github.com/Code-Hex/vz/v3), [objc2-virtualization 0.3.2 docs](https://docs.rs/objc2-virtualization/0.3.2/objc2_virtualization/) — MEDIUM (Go mirror; Rust API mirrors it)
- VZVirtualMachineConfiguration.setSerialPorts: [objc2-virtualization docs confirmed](https://docs.rs/objc2-virtualization/0.3.2/objc2_virtualization/struct.VZVirtualMachineConfiguration.html) — HIGH
- Docker live restore / daemon state: [Docker docs live-restore](https://docs.docker.com/engine/daemon/live-restore/) — MEDIUM
- OrbStack restart semantics: [docs.orbstack.dev/machines/commands](https://docs.orbstack.dev/machines/commands) — MEDIUM

---
*Feature research for: Speck v1.2 Hardened Runtime*
*Researched: 2026-07-06*
