# Speck (`spk`)

**An ultra-fast container runtime for Apple Silicon that never loses the network.**

Speck boots a micro-VM using Apple's native `Virtualization.framework` — no QEMU, no emulation — and exposes a Docker-compatible socket so your existing tooling (`docker`, `docker compose`, `testcontainers`, BuildKit) works unchanged. Its defining trait: containers inherit the host's routing table and DNS in real time, so they keep working under corporate VPNs and Cloudflare WARP — exactly where Docker Desktop breaks.

> **Status: alpha.** Core VM boot, Docker API socket, and VPN-proof networking are implemented. Expect rough edges; bug reports welcome.

## Requirements

- Apple Silicon Mac (M1–M4+)
- macOS 13 Ventura or later

## Why Speck?

Docker Desktop creates a synthetic NAT subnet for its VM. That subnet collides with VPN routing tables and WARP's overlay — containers lose DNS and network access the moment you connect to VPN.

Speck routes raw L2 frames through a user-space TCP/IP stack that re-originates every connection from the host's own network namespace. There is no synthetic subnet. Containers share your live routing table and resolve DNS through macOS's own resolver, which picks up VPN split-DNS, mDNS, and `/etc/hosts` changes in real time.

## Install

### Homebrew (recommended)

```bash
brew tap wasilak/speck https://github.com/wasilak/speck
brew install wasilak/speck/speck
```

macOS may quarantine the binary on first install (ad-hoc signing). Clear it with:

```bash
xattr -dr com.apple.quarantine $(brew --prefix)/bin/spk
```

### Build from source

Requires Rust stable, Xcode Command Line Tools, and `protoc`:

```bash
git clone https://github.com/wasilak/speck
cd speck
brew install protobuf          # provides protoc
cargo build --release --package speck-cli --target aarch64-apple-darwin
codesign --sign - \
  --entitlements speck.entitlements \
  --force \
  target/aarch64-apple-darwin/release/spk
cp target/aarch64-apple-darwin/release/spk /usr/local/bin/spk
```

## First boot (step by step)

**1. Start the VM**

```bash
spk up
```

On first run, `spk up` downloads the guest assets (~200 MB: Linux kernel, initrd with `vminitd`, Alpine-based rootfs with dockerd). Subsequent boots use the cached assets and start in milliseconds.

**2. Point your Docker tooling at Speck — one-time setup**

```bash
spk init --set-docker-host
# then open a new shell (or source your rc file)
```

This writes `DOCKER_HOST=unix://$HOME/.speck/run/docker.sock` into your shell config. All `docker` and `docker compose` commands now route through Speck.

For a per-session override instead:

```bash
eval $(spk env)
```

**3. Verify**

```bash
docker info          # should show the Speck runtime
docker run --rm alpine echo "hello from Speck"
```

**4. Stop the VM**

```bash
spk down
```

## Configuration

`~/.speck/config.yaml` (override the directory with `SPECK_HOME`):

```yaml
vm:
  cpus: 4
  memory_mb: 4096
  disk_gb: 60

ca:
  extra_certs:
    - /etc/ssl/certs/corporate-ca.pem
```

Per-invocation flags override the config file:

```bash
spk up --cpus 8 --memory 8192
```

## Diagnostics

```bash
spk doctor                              # full health check (codesign, VM state, DNS, certs, Docker socket)
spk doctor dns                          # trace DNS through the guest resolver
spk doctor dns internal.corp.example   # verify VPN split-DNS reaches a scoped nameserver
```

`spk doctor` exits 0 when everything is healthy, 1 on any failure, with an actionable hint per failing check.

## Other commands

```bash
spk ps                  # list running containers
spk build -t myapp .    # build an image with BuildKit
spk dashboard           # live TUI (VM stats, containers, logs)
spk completion zsh      # shell completion (zsh/bash/fish)
```

## Alpha status

This is an alpha release intended for early testing and feedback. Known areas still under validation:

| Feature | Status |
|---------|--------|
| VM boot via Virtualization.framework | ✅ |
| Docker-compatible API socket | ✅ |
| VPN-proof networking (live host routing table) | ✅ |
| VPN-proof DNS (SCDynamicStore live reload) | ✅ |
| Corporate CA injection into guest trust bundle | ✅ |
| BuildKit (`spk build`) | ✅ |
| Shell integration (`spk env`, `spk init`) | ✅ |
| Config file + VM resource controls | ✅ |
| Background daemon (launchd LaunchAgent) | ✅ |
| `spk dashboard` TUI | ✅ |
| `spk doctor` diagnostics | ✅ |
| Homebrew Formula distribution | ✅ |
| testcontainers conformance | 🔧 in progress |
| K3s / Kubernetes | 📋 planned |
| Developer ID signing + notarization | 📋 planned |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). A CLA is required for all contributions.

## License

[AGPLv3](LICENSE) with CLA.
