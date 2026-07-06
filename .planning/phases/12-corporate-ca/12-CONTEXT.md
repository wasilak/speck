# Phase 12 — Corporate CA Injection: Design Context

## Goal

Containers pull images from private registries and make HTTPS calls to internal
services without TLS errors when custom CA certificates are configured.

## Requirements

| ID | Description | Verification |
|----|-------------|-------------|
| CERT-01 | User specifies CA cert paths in `config.yaml` under `ca.extra_certs` | `config.yaml` with `ca.extra_certs: [/path/to/ca.pem]` is parsed without warnings |
| CERT-02 | Certs injected into guest OS trust bundle before dockerd starts | `update-ca-certificates` runs in guest before dockerd spawn |
| CERT-03 | Certs configured in dockerd's internal containerd `hosts.toml` and buildkitd `config.toml` | Registry pull over HTTPS with custom CA succeeds |
| CERT-04 | Missing / invalid PEM causes `spk up` to fail fast before VM starts | Non-existent path → `file not found`; garbage file → `not valid PEM` |

## Key Decisions (agreed in discussion)

### D-12-01: Injection mechanism — Dedicated VirtioFS share

A dedicated VirtioFS share with tag `ca-certs` exposes the host directory
`$SPECK_HOME/ca-certs/` into the guest at `/var/lib/speck/ca-certs/`.

**Why not alternatives:**
- Kernel cmdline encoding: fragile, size-limited (~128 KB total cmdline)
- Vsock file transfer: more complex, needs a new vsock protocol
- Initrd bake: can't be dynamic per-user config, requires rootfs rebuild

**Flow:**
1. Host copies configured cert files into `$SPECK_HOME/ca-certs/` (deduplicated)
2. Host adds VirtioFS device with tag `ca-certs` + share dir `$SPECK_HOME/ca-certs/`
3. Host appends `ca_certs_tag=ca-certs` to kernel cmdline
4. vminitd mounts the share at the well-known guest path
5. vminitd runs cert injection steps before dockerd starts

### D-12-02: Config schema — `ca.extra_certs: [paths]`

```yaml
# config.yaml
version: 1
vm:
  cpus: 4
  memory_mb: 4096
ca:
  extra_certs:
    - /Users/me/certs/corporate-root.pem
    - /Users/me/certs/proxy-ca.crt
```

**Host-side resolution:**
- Paths are absolute host filesystem paths (no glob expansion, no `~`)
- Each path is validated at config time: exists? → is valid PEM? (via `pem` crate)
- Valid certs are copied to `$SPECK_HOME/ca-certs/{sha256}.pem` (deduplicated by content hash)
- Missing paths → error: `ca.extra_certs: file not found: /path/to/missing.pem`
- Invalid PEM → error: `ca.extra_certs: not valid PEM: /path/to/bad.pem` (names the file + PEM parse error)

**Env var override (deferred, not in scope):** `SPECK_CA_EXTRA_CERTS` could
support colon-separated paths in a future phase.

### D-12-03: Trust store update — Alpine `update-ca-certificates`

Alpine reads `/usr/local/share/ca-certificates/*.crt` (copy certs there) and
writes the combined bundle to `/etc/ssl/certs/ca-certificates.crt`.

**vminitd injection sequence** (runs after rootfs mount, before dockerd spawn):

```
1. Mount ca-certs VirtioFS share at /var/lib/speck/ca-certs/  (new mount function)
2. For each *.pem/*.crt in /var/lib/speck/ca-certs/:
   a. Copy to /usr/local/share/ca-certificates/{basename}.crt
   b. Run `update-ca-certificates` inside chroot
3. Proceed to detect_dockerd_bin_in_chroot() + spawn_dockerd_with_restart()
```

**Why `update-ca-certificates` (not manual concatenation):**
- Alpine's tool also rebuilds the hash symlinks under `/etc/ssl/certs/`
- Programs that use OpenSSL `SSL_CTX_load_verify_locations()` rely on hash lookup
- `docker build` contexts and `curl` inside containers both use the system trust store

### D-12-04: Validation timing — Config-load-time, before VM starts

All cert validation happens in `speck-cli` config resolution, **before** the VM
is created. This gives fast failure (no waiting for VM boot + vsock + dockerd).

**Where in the boot sequence:**
```
read_config()        ← existing, extended to parse ca.extra_certs
  ↓
validate_certs()     ← NEW: exists + PEM check + copy to $SPECK_HOME/ca-certs/
  ↓
resolve_effective_config()  ← existing, unchanged
  ↓
allocate_vm()        ← existing, only reached if certs are valid
```

**Error messages:**
```
# Missing file
Error: ca.extra_certs: file not found: /Users/me/certs/missing.pem

# Invalid PEM
Error: ca.extra_certs: not valid PEM: /Users/me/certs/bad.pem
  Caused by: expected PEM header "CERTIFICATE" at line 1, got "BEGIN RSA KEY"
```

### D-12-05: Registry targeting — hosts.toml + buildkitd config.toml

Docker Engine's internal containerd reads per-registry TLS config from
`/etc/containerd/certs.d/<registry>/hosts.toml`. BuildKit reads its own
`config.toml`.

**containerd hosts.toml** (written by vminitd into chroot):

For each registry domain that needs custom CA, write:
```toml
# /etc/containerd/certs.d/registry.internal.corp:443/hosts.toml
server = "https://registry.internal.corp:443"

[host."https://registry.internal.corp:443".capabilities]
pull = ["pull", "resolve"]

[host."https://registry.internal.corp:443".ca]
certs = ["/etc/ssl/certs/ca-certificates.crt"]
```

**How we determine which registries need custom CA:**
- Write a single catch-all `hosts.toml` for `_default` that points at the
  system trust store (which already includes the injected CAs)
- Since we injected into the system bundle, containerd automatically trusts
  those CAs for all registries

**buildkitd config.toml:**
- BuildKit is not yet running in the guest (Phase 06 delivered Docker API
  compatibility via dockerd only). BuildKit is a future phase.
- For now, `CERT-03` is satisfied by containerd `hosts.toml` alone.
- When BuildKit is added, add `[registry."host".ca]` entries to its config.

## Implementation Plan

### Crate: `speck-cli` (config validation + cert copy)

1. Add `CaConfig` struct to `config.rs`:
   ```rust
   #[derive(Debug, Clone, Default, Deserialize)]
   pub struct CaConfig {
       #[serde(default)]
       pub extra_certs: Vec<String>,
   }
   ```
   Add `ca: CaConfig` field to `AppConfig`.

2. Add `ca` to `collect_unknown_keys()` whitelist.

3. Implement validation function:
   ```rust
   pub fn validate_and_prepare_ca_certs(
       speck_home: &Path,
       extra_certs: &[String],
   ) -> anyhow::Result<Vec<PathBuf>>
   ```
   - Iterates paths, checks `path.exists()`, reads content, parses with `pem::parse_many()`
   - Copies to `$SPECK_HOME/ca-certs/{sha256}.pem`
   - Returns the list of copied files (for VM share setup)

4. Call `validate_and_prepare_ca_certs` in `run_up()` before `GuestConfig::builder()`.

5. Pass `ca_certs_tag` into kernel cmdline when certs are present.

### Crate: `speck-vz` (VirtioFS device for ca-certs)

6. Add method `with_ca_certs_share(path: &Path, tag: &str)` to `GuestConfig::builder()`.
   Creates `VZVirtioFileSystemDeviceConfiguration` with tag `ca-certs` and share directory.

### Crate: `speck-guest` (vminitd injection)

7. Add cmdline parse: `parse_cmdline_ca_certs_tag()`.

8. Add `mount_ca_certs()` that mounts the share and copies certs into
   `/usr/local/share/ca-certificates/` then runs `update-ca-certificates`.

9. Call `mount_ca_certs()` in `main()` between `mount_disks()` and
   `spawn_dockerd_with_restart()`.

## Files to modify

| File | Change |
|------|--------|
| `crates/speck-cli/src/config.rs` | Add `CaConfig`, extend `AppConfig`, add `validate_and_prepare_ca_certs()`, extend `collect_unknown_keys()` |
| `crates/speck-cli/src/commands/up.rs` | Call cert validation, add `ca_certs_tag=` to kernel cmdline |
| `crates/speck-vz/src/lib.rs` or `guest.rs` | Add `with_ca_certs_share()` to builder |
| `crates/speck-guest/src/bin/vminitd.rs` | Add `parse_cmdline_ca_certs_tag()`, `mount_ca_certs()`, wire into boot sequence |
| `.planning/STATE.md` | Update CA dependency, mark `pem` crate as used |

## Future scope (not in this phase)

- `SPECK_CA_EXTRA_CERTS` env var support
- macOS Keychain auto-detection (CERT-V2 in REQUIREMENTS.md)
- Per-container CA via Docker CLI env vars (`SSL_CERT_FILE`)
- BuildKit `config.toml` registry CA entries

## Implementation Decisions — Resolved

1. **CA certs tag:** Constant `CA_CERTS_TAG = "speck-ca-certs"` in `crates/speck-vz/src/virtiofs.rs:27`. Not an env var — compile-time constant passed via kernel cmdline as `ca_certs_tag=speck-ca-certs`.

2. **Duplicate cert handling:** Silent dedup by SHA256 content hash. `validate_and_prepare_ca_certs` uses a `HashSet<String>` of hex hashes; duplicate content is silently skipped. Files named `{sha256}.pem` already present in `$SPECK_HOME/ca-certs/` are not overwritten.

3. **update-ca-certificates timeout:** No timeout in current implementation. Called via blocking `std::process::Command` + `.status()`. Failure is logged but non-fatal (vminitd continues). Deferred to a future phase if slow cert bundles become a real issue.
