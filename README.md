# Speck (`spk`)

**An ultra-fast container runtime for Apple Silicon that never loses the network.**

Speck boots a micro-VM using Apple's native `Virtualization.framework` — no QEMU, no emulation — and exposes a Docker-compatible socket so your existing tooling (`docker`, `docker compose`, `testcontainers`, BuildKit) works unchanged. Its defining trait: containers inherit the host's routing table and DNS in real time, so they keep working under corporate VPNs and Cloudflare WARP — exactly where Docker Desktop breaks.

> **Status: pre-stable, active battle-testing.** Core features are implemented and under validation — Docker API compatibility, VPN/WARP DNS, BuildKit, and corporate CA injection. Not yet recommended for production use.

## Requirements

- Apple Silicon Mac (M1–M4+)
- macOS 13 Ventura or later
- Xcode Command Line Tools (`xcode-select --install`)

## Why Speck?

Docker Desktop creates a synthetic NAT subnet for its VM. That subnet collides with VPN routing tables and WARP's overlay — containers lose DNS and network access the moment you connect to VPN.

Speck routes raw L2 frames through a user-space TCP/IP stack that re-originates every connection from the host's own network namespace. There is no synthetic subnet. Containers share your live routing table and resolve DNS through macOS's own resolver, which picks up VPN split-DNS, mDNS, and `/etc/hosts` changes in real time.

## Install

### Build from source

```bash
git clone https://github.com/speck-vm/speck
cd speck
cargo build --release --target aarch64-apple-darwin
```

### Sign the binary (required)

`VZVirtualMachine` requires the `com.apple.security.virtualization` entitlement to be embedded in the binary. Without it the runtime fails at VM creation. For local development, ad-hoc signing works:

```bash
codesign --sign - \
  --entitlements speck.entitlements \
  --force \
  target/aarch64-apple-darwin/release/spk
```

> Production releases will be signed with Apple Developer ID and distributed via Homebrew. Ad-hoc signing is the current approach while the project is in battle-testing.

### Put it on your PATH

```bash
cp target/aarch64-apple-darwin/release/spk /usr/local/bin/spk
```

### Homebrew (coming)

Once releases stabilize, you will be able to install via:

```bash
brew install speck-vm/speck/speck
```

The Formula uses Homebrew's `--preserve-metadata=entitlements` re-signing path, which preserves the `com.apple.security.virtualization` entitlement.

## Quick start

```bash
# Boot the VM (millisecond cold start)
spk up

# Point Docker tooling at Speck — one-time persistent setup
spk init        # writes a shell block to ~/.zshrc / ~/.bashrc / fish config
# or per-session:
eval $(spk env)

# Now use docker as normal
docker run --rm alpine echo "hello from Speck"
docker compose up

# BuildKit
spk build -t myapp:latest .

# Live dashboard
spk dashboard

# Stop the VM
spk down
```

## Configuration

`~/.speck/config.yaml` (override with `SPECK_HOME`):

```yaml
vm:
  cpus: 4
  memory_mb: 4096
  disk_gb: 60

ca:
  extra_certs:
    - /etc/ssl/certs/corporate-ca.pem
```

Per-invocation flags override the config file, which overrides env vars, which override built-in defaults:

```bash
spk up --cpus 8 --memory 8192
SPECK_VM_CPUS=6 spk up
```

## Diagnostics

```bash
spk doctor                              # full health check (codesign, VM state, DNS, certs, Docker socket)
spk doctor dns                          # trace DNS through the guest resolver
spk doctor dns internal.corp.example   # verify VPN split-DNS reaches a scoped nameserver
```

`spk doctor` exits 0 when everything is healthy, 1 on any failure, with an actionable hint per failing check.

## Feature status

| Feature | Status |
|---------|--------|
| VM boot via Virtualization.framework | ✅ |
| Docker-compatible API socket | ✅ |
| VPN-proof networking (live host routing table) | ✅ |
| VPN-proof DNS (SCDynamicStore live reload, no `:53` binding) | ✅ |
| Corporate CA injection into guest trust bundle | ✅ |
| BuildKit (`spk build`) | ✅ |
| Shell integration (`spk env`, `spk init`) | ✅ |
| Config file + VM resource controls | ✅ |
| Background daemon (launchd LaunchAgent) | ✅ |
| `spk dashboard` TUI | ✅ |
| `spk doctor` diagnostics | ✅ |
| Homebrew Formula distribution | 🔧 in progress |
| testcontainers conformance | 🔧 in progress |
| K3s / Kubernetes | 📋 planned |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). A CLA is required for all contributions.

## License

[AGPLv3](LICENSE) with CLA. No permissive license that allows closing the source.
