# Speck (`spk`)

## What This Is

Speck is an ultra-fast, minimalist container runtime built exclusively for **Apple Silicon macOS** (`aarch64-apple-darwin`). A Rust core drives Apple's native `Virtualization.framework` directly through a Swift bridge — no QEMU, no cross-compilation, no Intel legacy. It boots a microscopic Linux kernel + initrd micro-VM in milliseconds, running `containerd` + `BuildKit` inside, and exposes a Docker-compatible API socket so existing tooling works unchanged. Its defining trait: networking that inherits the host's routing table and DNS in real time, so it keeps working under corporate VPNs and Cloudflare WARP — exactly where Docker Desktop breaks. It is built as a product from day one (open-source community tool with a commercial Enterprise path), with a Core-as-library architecture so a future SwiftUI GUI and K8s/K3s can bolt on without a rewrite.

## Core Value

**A container runtime on Apple Silicon that never loses the network — micro-VMs inherit the host's routing/DNS live, surviving corporate VPNs and Cloudflare WARP where Docker Desktop fails.** Everything else (speed, beauty, Docker compatibility) reinforces this; if this one thing fails, Speck has no reason to exist.

## Requirements

### Validated

<!-- Shipped and confirmed valuable. -->

(None yet — ship to validate)

### Active

<!-- Current scope. Building toward these. "Full CLI vision" v1. -->

**Engine & virtualization**
- [ ] Boot a micro-VM (tiny Linux kernel + initrd) via `Virtualization.framework` in milliseconds
- [ ] Rust ⇄ Swift bridge (`swift-bridge` or `cxx`) calling `Virtualization.framework` with zero intermediary layer
- [ ] `containerd` runtime + `BuildKit` running inside the guest micro-VM
- [ ] Pull and run container images
- [ ] Build container images (`spk build` via BuildKit)
- [ ] Mount host volumes into containers via VirtioFS

**Networking (the killer feature)**
- [ ] Shared-host socket networking mode — no synthetic routers/subnets that fight corporate tunnels
- [ ] Micro-VM inherits host routing table + DNS servers from macOS in real time
- [ ] Survives Cloudflare WARP / corporate VPN DNS changes automatically (live adaptation)

**Docker compatibility**
- [ ] Expose a Docker/containerd-compatible API socket so `docker` CLI, Compose, testcontainers, and IDE integrations work unchanged

**CLI & UX**
- [ ] `clap` v4+ command surface: `spk up`, `spk down`, `spk ps` (and friends)
- [ ] Cyberpunk terminal theme (neon cyan/violet) honoring `CLICOLOR` / `NO_COLOR`
- [ ] Async progress bars with time estimates (`indicatif`)
- [ ] Microsecond-resolution structured logging (`tracing`)
- [ ] Interactive `spk dashboard` TUI for live container view (`ratatui`)
- [ ] Native shell completion generation for `zsh`, `fish`, `bash` (`spk completion <shell>`)

**Architecture (product readiness)**
- [ ] Core compiled as a reusable library (`.a` / `.dylib`); CLI is just one frontend
- [ ] Strict separation of business logic from presentation (no console prints inside core/system functions)
- [ ] Container control via containerd gRPC / CRI socket (K8s-ready seam, K3s not built yet)

**Licensing & contribution (product from day one)**
- [ ] AGPLv3 license applied to the project
- [ ] CLA process for contributors (preserves copyright for dual-licensing)
- [ ] Documented commercial "Speck Enterprise" exception path
- [ ] Homebrew-installable distribution

### Out of Scope

<!-- Explicit boundaries with reasoning to prevent re-adding. -->

- **Intel / x86_64 support** — Apple Silicon only by design; zero legacy debt, maximal ARM optimization
- **Cross-compilation to other targets** — single target `aarch64-apple-darwin` keeps the build and runtime assumptions tight
- **QEMU or any third-party emulator** — native `Virtualization.framework` only; emulators defeat the speed goal
- **Linux / Windows host support** — macOS-exclusive; the whole value prop is native macOS virtualization
- **SwiftUI GUI (this milestone)** — architecture is made ready for it (Core-as-library), but the GUI is a future milestone
- **Kubernetes / K3s bundled in the image (this milestone)** — the containerd gRPC/CRI seam is built so K3s can be injected later, but no K8s now
- **BSL 1.1 licensing** — rejected: its auto-conversion to Apache after ~3 years is unwanted; AGPLv3 has no expiry

## Context

- **Greenfield.** Empty repo; first command is `cargo init speck --bin`. No existing code to map.
- **Origin problem:** Docker Desktop (and similar) break under corporate VPN / Cloudflare WARP because they create synthetic network routers/subnets that collide with the tunnel. Speck exists to solve this real, widespread engineer pain by inheriting host networking instead.
- **Naming:** "Speck" = a speck/mote of dust — evokes the microscopic system overhead. CLI `speck`, alias `spk`. Homebrew name is available.
- **Vision horizon:** Framed as a "2027 edition" — deliberately built on tomorrow's stack with no backward-compat debt.
- **Future frontends already planned (not this milestone):** native SwiftUI GUI sharing the same Rust Core; K8s via dropping a `k3s` binary into the guest image and exposing a port.
- **Author:** Senior DevOps/observability engineer building a no-compromise, top-quality tool intended for public release and community.

## Constraints

- **Platform**: macOS on Apple Silicon (`aarch64-apple-darwin`) only — hardware-accelerated virtualization, native ARM instructions (M1–M4+), no cross-compile.
- **Tech stack — core**: Rust (memory safety, no GC, top performance) as the single engine language.
- **Tech stack — macOS bridge**: `swift-bridge` or `cxx` for zero-overhead Rust ⇄ Swift calls into `Virtualization.framework`.
- **Tech stack — guest**: Minimal Linux kernel + initrd, `containerd` + `BuildKit` only — no heavy guest OS.
- **Tech stack — storage**: VirtioFS for host→container volume mounts.
- **Tech stack — CLI**: `clap` v4+, `indicatif`, `ratatui`, `tracing`.
- **Performance**: micro-VM cold start measured in milliseconds, not seconds; "microscopic system overhead" is a brand promise, not a nice-to-have.
- **Networking**: must never introduce synthetic routers/subnets; must inherit host routing + DNS live.
- **Architecture**: Core must be a library with zero presentation coupling — CLI, future GUI, and K8s seam are all consumers of the same Core.
- **Licensing**: AGPLv3 + CLA; no permissive license that would let a vendor close the source, and no time-delayed conversion.

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| Apple Silicon only (`aarch64-apple-darwin`) | Eliminate cross-compile + Intel legacy; exploit hardware-accelerated ARM virtualization | — Pending |
| Rust core + Swift bridge to `Virtualization.framework` (no QEMU) | Zero-overhead native path → millisecond VM starts; memory safety, no GC | — Pending |
| Host-inheriting shared-socket networking | Direct solution to VPN/WARP breakage — the core value | — Pending |
| Docker-compatible API socket | Adoption: existing `docker` CLI, Compose, testcontainers, IDEs work unchanged | — Pending |
| Full CLI vision in v1 (run + build + dashboard + completions) | User wants a complete, product-grade first milestone, not a thin MVP | — Pending |
| Product from day one | Licensing, CLA, contribution model, and Core/Frontend split are first-class, not afterthoughts | — Pending |
| AGPLv3 + CLA (reject BSL 1.1) | Copyleft as anti-capture deterrent; CLA enables Enterprise dual-licensing; no unwanted Apache auto-conversion | — Pending |
| containerd gRPC/CRI control seam | K8s-ready architecture without building K8s now — K3s injectable later | — Pending |
| Core-as-library (`.a`/`.dylib`), CLI as one frontend | Avoids a rewrite when SwiftUI GUI arrives; same engine/speed/networking for all frontends | — Pending |

## Evolution

This document evolves at phase transitions and milestone boundaries.

**After each phase transition** (via `/gsd-transition`):
1. Requirements invalidated? → Move to Out of Scope with reason
2. Requirements validated? → Move to Validated with phase reference
3. New requirements emerged? → Add to Active
4. Decisions to log? → Add to Key Decisions
5. "What This Is" still accurate? → Update if drifted

**After each milestone** (via `/gsd:complete-milestone`):
1. Full review of all sections
2. Core Value check — still the right priority?
3. Audit Out of Scope — reasons still valid?
4. Update Context with current state

---
*Last updated: 2026-06-24 after initialization*
