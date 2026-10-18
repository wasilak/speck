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

```bash
brew install wasilak/tap/speck
```

The binary is ad-hoc signed with the `com.apple.security.virtualization`
entitlement and not notarized. Pre-built archives are also on
[GitHub Releases](https://github.com/wasilak/speck/releases).

### Build from source

Requires Rust stable and Xcode Command Line Tools:

```bash
git clone https://github.com/wasilak/speck
cd speck
cargo build --release --package speck-cli --target aarch64-apple-darwin
cargo xtask sign release
cp target/aarch64-apple-darwin/release/spk /usr/local/bin/spk
```

## Usage (same as colima)

```bash
spk start       # start the VM; selects the 'speck' docker context
spk status      # -e: resources, container counts, versions
spk list        # colima-style instance table
spk version     # spk, docker and guest versions
spk restart
spk stop        # stop the VM; switches docker back to the 'default' context
spk stop -f     # kill a hung VM at once, without a graceful shutdown
spk config      # validate config.yaml and show effective settings and their sources
spk template    # edit config.yaml in $EDITOR (starts from a commented template)
spk prune       # delete cached downloads Speck no longer uses
spk delete      # remove the VM with all containers, images and volumes
spk recreate    # delete the VM and start a fresh one (keeps downloads and config)
spk profile ls  # profiles: independent instances (see below)
```

`status`, `list`, `profile ls`, `profile current`, `config`, `version` and
`doctor` print JSON or YAML with `-o json` or `-o yaml` (`-j` is short for
`-o json`). Only the document goes to stdout, and the exit code does not
depend on the format, so scripts can rely on both:

```bash
spk status -o json | jq .running
spk config -o yaml
```

On first start Speck downloads the guest assets (~200 MB: Linux kernel,
initrd with `vminitd`, Alpine-based rootfs with dockerd). Later starts use
the cached assets; after an upgrade only changed assets are downloaded again,
and containers, images and volumes are kept.

`docker`, `docker compose` and `docker buildx` work without any setup, through
the `speck` docker context. Speck starts again automatically at login.

`linux/amd64` images run and build (`--platform linux/amd64`) through Apple's
Rosetta. If Rosetta is not installed, install it once with
`softwareupdate --install-rosetta --agree-to-license` and run `spk restart`.

dockerd uses the containerd image store, like Colima, so multi-platform
builds (`docker buildx build --platform linux/amd64,linux/arm64 --push`) work
with the default builder.

As in Docker Desktop and Colima:

- containers reach services on the Mac at `host.docker.internal`, and
  `--add-host name:host-gateway` (compose `extra_hosts`) points at the Mac;
- `-v /var/run/docker.sock:/var/run/docker.sock` gives a container the Speck
  Docker socket (testcontainers Ryuk, devcontainers, `docker:cli`);
- `-p` publishes TCP and UDP ports on `localhost`.

Tools that ignore docker contexts and only read `DOCKER_HOST` (some
testcontainers and docker-py setups) need the environment variable:

```bash
spk init --set-docker-host   # persist in your shell config, or per session:
eval $(spk env)
```

Check that it works:

```bash
docker context show  # speck
docker run --rm alpine echo "hello from Speck"
```

## Configuration

`~/.speck/config.yaml` (`~/.speck/profiles/<name>/config.yaml` for a profile):

```yaml
vm:
  cpus: 4
  memory_mb: 6g     # or 6144 (MiB), 6G, 1.5g
  disk_gb: 100      # or 100g, 1t

ca:
  extra_certs:
    - /etc/ssl/certs/corporate-ca.pem
```

Corporate CA (TLS inspection): Speck adds the certificates you trust in the
Keychain (`ca.keychain`, on by default) and `ca.extra_certs` to the VM, so
`docker pull` works. Set `ca.inject_into_containers: true` to give containers
the same bundle (with `SSL_CERT_FILE`, `REQUESTS_CA_BUNDLE` and
`NODE_EXTRA_CA_CERTS` set). Build `RUN` steps do not get it, as in Docker
Desktop and Colima; copy the CA into the image in the Dockerfile when a build
step needs it.

`spk start --cpus/--memory/--disk` saves the values to the config file, like
`colima start`. Sizes take k/m/g/t units in any case (`--memory 8g`); a plain
number is MiB for memory and GiB for disk. A running VM keeps its settings
until `spk restart`.

```bash
spk start --cpus 8 --memory 8g
```

`spk template` opens the config file in `$VISUAL` or `$EDITOR`. If the file
does not exist yet, it starts from a commented template with every key and
its default. Speck checks the file after you close the editor and offers to
open it again when it is invalid. `spk start --edit` does the same, then
starts Speck.

Containers get the VM memory minus a reserve (1/8, 512 MiB to 1 GiB) kept for
dockerd and containerd, so a full container memory never makes `docker`
hang: the kernel stops processes inside the containers instead.

## Profiles

A profile is an independent Speck instance with its own VM, disks, config,
socket and docker context, like `colima -p`. Every command takes
`-p <name>` (or `SPECK_PROFILE=<name>`):

```bash
spk start -p work --cpus 4 --memory 8g   # second VM next to the default one
docker --context speck-work ps           # its docker context
spk list                                 # one row per profile
spk stop -p work
spk delete -p work                       # removes the profile completely
```

`spk profile use work` makes `work` the profile bare `spk` commands act on
(`spk profile use default` switches back). Order of precedence: `-p`,
`SPECK_PROFILE`, `spk profile use`, `default`. Other `spk profile` commands:
`ls`, `current`, `create <name> [--cpus/--memory/--disk]` and `rm <name>`.

The default profile stays in `~/.speck`; others live in
`~/.speck/profiles/<name>`. Downloads (kernel, initrd, rootfs) are shared, so
a new profile starts without downloading them again. Two running profiles
cannot publish the same host port.

For CI, start a throwaway profile without a launchd agent. It runs in the
background, does not come back after a reboot, and does not switch your
docker context, so parallel jobs on one Mac stay separate:

```bash
export SPECK_PROFILE=ci-$JOB_ID
spk start --no-autostart --cpus 2 --memory 2g --wait
DOCKER_HOST=unix://$HOME/.speck/profiles/$SPECK_PROFILE/speck.sock ./run-tests.sh
spk delete --force
```

## Environment variables

| Variable | Default | Description |
|----------|---------|-------------|
| `SPECK_PROFILE` | `default` | Profile to act on (see [Profiles](#profiles)). `-p` takes precedence. |
| `SPECK_HOME` | `~/.speck` | Raw instance directory, for tests. Such an instance runs only with `spk up --foreground`, gets no docker context and cannot be combined with a profile. |
| `DOCKER_HOST` | *(not set)* | Set by `spk init --set-docker-host` or `eval $(spk env)` to `unix://$SPECK_HOME/speck.sock`. Directs Docker-compatible clients to Speck. |
| `SPECK_VM_CPUS` | config / 2 | vCPU count for the VM. Takes precedence over `config.yaml` and `--cpus`. |
| `SPECK_VM_MEMORY_MB` | config / 2048 | VM memory: MiB or a size such as `6g`. Takes precedence over `config.yaml` and `--memory`. |
| `SPECK_VM_DISK_GB` | config / 100 | VM data disk: GiB or a size such as `1t`. Takes precedence over `config.yaml` and `--disk`. |
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
| Homebrew Formula distribution (`wasilak/tap/speck`) | ✅ |
| testcontainers conformance | ✅ |
| K3s / Kubernetes | 📋 planned |
| Developer ID signing + notarization | 📋 planned |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). A CLA is required for all contributions.

## License

[AGPLv3](LICENSE) with CLA.
