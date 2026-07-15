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

Requires Rust stable and Xcode Command Line Tools:

```bash
git clone https://github.com/wasilak/speck
cd speck
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

## Environment variables

| Variable | Default | Description |
|----------|---------|-------------|
| `SPECK_HOME` | `~/.speck` | Directory for VM assets, config, socket, and logs. Override to run multiple isolated instances. |
| `DOCKER_HOST` | *(not set)* | Set by `spk init --set-docker-host` or `eval $(spk env)` to `unix://$SPECK_HOME/speck.sock`. Directs Docker-compatible clients to Speck. |
| `SPECK_VM_CPUS` | config / 2 | vCPU count for the VM. Takes precedence over `config.yaml` and `--cpus`. |
| `SPECK_VM_MEMORY_MB` | config / 2048 | VM memory in MiB. Takes precedence over `config.yaml` and `--memory`. |
| `SPECK_VM_DISK_GB` | config / 20 | VM data disk size in GiB. Takes precedence over `config.yaml` and `--disk`. |
| `SPECK_LOG_LEVEL` | config / `info` | [tracing `EnvFilter`](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html) directive. Takes precedence over `config.yaml` `log_level`. |
| `SPECK_DAEMONIZED` | *(not set)* | Set to `1` internally by the launchd LaunchAgent. Switches the process to file-appender logging and suppresses progress output. Do not set manually. |

## Logs

The daemon writes structured logs to `$SPECK_HOME/speck.log` (rotates at 10 MiB, keeps 5 generations). The guest serial console is captured separately to `$SPECK_HOME/console.log`.

```bash
spk logs                   # print the full daemon log
spk logs --tail            # print the last 20 lines
spk logs --follow          # stream new entries as they are written (like tail -f)
```

## Shell completion

```bash
# zsh — add to ~/.zshrc
eval "$(spk completion zsh)"

# bash — add to ~/.bashrc
eval "$(spk completion bash)"

# fish — add to ~/.config/fish/conf.d/speck_completion.fish
spk completion fish | source
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

## Docker CLI compatibility

Speck exposes a Docker-compatible Unix socket at `$SPECK_HOME/speck.sock`. The system `docker` CLI, `docker compose`, `testcontainers`, and BuildKit all work unchanged — no custom client needed.

The conformance gate (`scripts/conformance-smoke.sh`) validates the everyday workflows against a live daemon:
pull, run, exec, logs, stop, rm, port publish, volume/network management, and build/push/pull round-trip.

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
| Homebrew Formula distribution | 📋 planned |
| testcontainers conformance | ✅ |
| K3s / Kubernetes | 📋 planned |
| Developer ID signing + notarization | 📋 planned |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). A CLA is required for all contributions.

## License

[AGPLv3](LICENSE) with CLA.
