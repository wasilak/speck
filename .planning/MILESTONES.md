# Milestones

## v1.1 — Production Runtime

**Shipped:** 2026-07-06
**Phases:** 07–14 (8 phases, 26 plans)
**Timeline:** 2026-07-01 → 2026-07-06 (5 days)
**Stats:** ~50 commits, 54 files changed, 7,736 insertions(+), 475 deletions(-), 17,048 Rust LOC total

### Delivered

`spk` graduated from a prototype to a daily-driver container runtime. Containers run end-to-end with working network egress, the daemon survives terminal close, DNS stays alive under VPN and WARP toggles, corporate CA certs inject cleanly, and `spk doctor` gives operators a single command to diagnose the full stack.

### Key Accomplishments

1. End-to-end container flow — network egress, Unix socket DockerClient, port bindings activated at container start
2. Persistent daemon via launchd LaunchAgent re-exec (zero fork/daemonize UB)
3. `$SPECK_HOME/config.yaml` with env > CLI > file > default precedence; vCPU/RAM/disk validation
4. `spk env` + idempotent `spk init` for bash/zsh/fish — one-time DOCKER_HOST setup
5. VPN-proof DNS: VM-internal proxy, SCDynamicStore live reload, split-DNS, NXDOMAIN→SERVFAIL
6. Corporate CA injection into guest OS trust bundle + containerd hosts.toml with fail-fast PEM validation
7. `spk doctor` 8-check health suite + `spk doctor dns <hostname>` resolver trace
8. Homebrew Formula (ad-hoc signed tarball) — `brew install speck-runtime/speck/spk` installs working `spk`

### Known Deferred Items

- DAEMON-04: First-class `spk restart` (workaround: `spk down && spk up`)
- DNS-01/03/05: Live lsof + WARP + VPN toggle verification (requires VPN-connected machine)
- BREW-DEVID: Developer ID + `.pkg` + notarytool + Cask distribution (requires Apple Developer Program)

### Archive

- Roadmap: `.planning/milestones/v1.1-ROADMAP.md`
- Requirements: `.planning/milestones/v1.1-REQUIREMENTS.md`

---

## v1.0 — Foundation (Phases 1–6)

*Shipped earlier — see v1.0 archive when created.*
