<!-- GSD:project-start source:PROJECT.md -->

## Project

**Speck (`spk`)**

Speck is an ultra-fast, minimalist container runtime built exclusively for **Apple Silicon macOS** (`aarch64-apple-darwin`). A Rust core drives Apple's native `Virtualization.framework` directly through a Swift bridge — no QEMU, no cross-compilation, no Intel legacy. It boots a microscopic Linux kernel + initrd micro-VM in milliseconds, running `containerd` + `BuildKit` inside, and exposes a Docker-compatible API socket so existing tooling works unchanged. Its defining trait: networking that inherits the host's routing table and DNS in real time, so it keeps working under corporate VPNs and Cloudflare WARP — exactly where Docker Desktop breaks. It is built as a product from day one (open-source community tool with a commercial Enterprise path), with a Core-as-library architecture so a future SwiftUI GUI and K8s/K3s can bolt on without a rewrite.

**Core Value:** **A container runtime on Apple Silicon that never loses the network — micro-VMs inherit the host's routing/DNS live, surviving corporate VPNs and Cloudflare WARP where Docker Desktop fails.** Everything else (speed, beauty, Docker compatibility) reinforces this; if this one thing fails, Speck has no reason to exist.

### Constraints

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

<!-- GSD:project-end -->

<!-- GSD:stack-start source:research/STACK.md -->

## Technology Stack

## Executive guidance (read first)

## Recommended Stack

### Core Technologies

| Technology | Version | Purpose | Why Recommended |
|------------|---------|---------|-----------------|
| Rust (toolchain) | 1.x stable, edition 2024 | Core engine language, single target `aarch64-apple-darwin` | Memory safety, no GC, zero-overhead FFI to Obj-C. Mandated by PROJECT.md. |
| `objc2` | 0.6.4 | Objective-C runtime + sound message-send/retain semantics | The foundation `objc2-virtualization` builds on. Actively maintained (madsmtm), regenerated within ~1 week of new Xcode SDKs. |
| `objc2-virtualization` | 0.3.2 | Direct Rust bindings to Apple's Virtualization.framework | Provides `VZVirtualMachine`, `VZLinuxBootLoader`, `VZVirtioSocketDevice`, `VZVirtioFileSystemDeviceConfiguration`, `VZ*NetworkDeviceAttachment`, delegates. Bindings generated from Xcode 16.4 SDK. Feature-gated — enable only the classes you use. |
| `objc2-foundation` | 0.3.2 | `NSString`, `NSURL`, `NSError`, `NSFileHandle`, `NSArray` bridging | Required to pass paths/URLs/file handles into Virtualization APIs. Must match the `objc2` 0.6.x generation. |
| `block2` | (matches objc2 gen, ~0.6.x) | Obj-C block support for completion handlers | `VZVirtualMachine.start`, socket connect, etc. take completion blocks; `block2` wraps Rust closures as Obj-C blocks. |
| `dispatch2` | (matches objc2 gen) | GCD queue handling | Virtualization.framework requires all `VZVirtualMachine` calls happen on a single serial `dispatch_queue`; you'll create and pin one. |
| `tokio` | 1.52.3 | Async runtime for the host core (gRPC client, Docker API server, vsock I/O) | Industry-standard async runtime; required by tonic/hyper. Pin to a recent 1.x. |
| `tonic` | 0.14.6 | gRPC client to in-guest containerd/CRI and to `vminitd` over vsock | Native Rust gRPC; talks to containerd's gRPC API and your own vminitd protobuf service. Pairs with `prost`. |
| `prost` | 0.14.4 | Protobuf codegen for containerd/CRI + vminitd protocols | Must match `tonic` 0.14.x. Generate from containerd's `.proto` files. |

### Guest Micro-VM Stack

| Component | Choice | Purpose | Why |
|-----------|--------|---------|-----|
| Kernel | Kata Containers arm64 kernel (prebuilt) | Linux guest kernel | Apple's own `containerization` recommends Kata's kernel: optimized for containers, fast boot, all needed configs on. **VIRTIO drivers must be compiled in, not modules** (kernel boots before any module FS exists). |
| Boot path | `VZLinuxBootLoader` (kernel + optional initrd + cmdline) | Direct kernel boot, no bootloader/firmware | Skips GRUB/firmware → millisecond cold start. No disk image needed if rootfs comes via initrd/VirtioFS. |
| Guest init (PID 1) | Custom minimal `vminitd` (Rust, musl/static) | First process; supervises containerd, exposes control RPC | Mirrors Apple's `vminitd`. Static musl binary so it has zero runtime deps. Exposes gRPC-over-vsock. |
| Container runtime | `containerd` (arm64) inside guest | OCI image pull/store, container lifecycle | Mandated by PROJECT.md; gives you the CRI/gRPC control seam for future K3s. |
| Image builder | `BuildKit` (arm64) inside guest | `spk build` | Mandated. Runs as a containerd-integrated builder or standalone `buildkitd`. |
| Image/layer store | containerd content store on a guest block/overlay FS | OCI layers, snapshots | Lives inside the guest (ext4 root or a dedicated `VZVirtioBlockDeviceConfiguration` data disk). Apple uses a Swift ext4 impl; you can use a real ext4 data disk attached via virtio-blk — simpler than reimplementing a filesystem. |

### Host ⇄ Guest Communication

| Mechanism | Class / Crate | Purpose |
|-----------|---------------|---------|
| Control plane | `VZVirtioSocketDevice` + `tonic` gRPC | Host core dials a guest vsock port; vminitd serves gRPC. All lifecycle/exec/build commands ride this. **Connections can only be initiated host→guest via `VZVirtioSocketDevice` connect, or guest→host via a listener.** |
| containerd access | gRPC over the same vsock (forwarded) | Host speaks containerd's gRPC API to manage images/containers; this is also the K8s/CRI seam. |
| File sharing | `VZVirtioFileSystemDeviceConfiguration` (VirtioFS) | Host dir → guest mount for `-v` volumes. See VirtioFS section. |

### Networking (the killer feature)

| Layer | Choice | Why |
|-------|--------|-----|
| VM attachment | `VZFileHandleNetworkDeviceAttachment` | Raw L2 ethernet frames over a `socketpair()` file handle. Gives you full control of the packet stream — the *only* attachment that lets you avoid a synthetic subnet. |
| Host net stack | User-space TCP/IP stack (gVisor `netstack` via `gvisor-tap-vsock`-style design, or a Rust netstack like `smoltcp` for L3/L4) | Terminates guest TCP/UDP in user space on the host, then re-originates the connection from the host's own network namespace — so it uses the **live host routing table** automatically. |
| DNS | Host-resolver proxy (resolve in-process using macOS's `getaddrinfo`/SCDynamicStore) | Resolving on the host with the system resolver means split-DNS, mDNS, `/etc/hosts`, and VPN/WARP DNS changes are picked up **live**, exactly the Lima `hostResolver` pattern that "deals correctly with VPN configurations and split-DNS setups." |

### CLI / Presentation

| Library | Version | Purpose | Notes |
|---------|---------|---------|-------|
| `clap` | 4.6.1 | Command surface (`spk up/down/ps/build/dashboard/completion`) | Use the derive API + `clap_complete` for zsh/fish/bash completion generation. Honor `CLICOLOR`/`NO_COLOR` via `clap`'s color settings + `anstream`. |
| `clap_complete` | matches clap 4.6 | `spk completion <shell>` | Generates zsh/fish/bash/pwsh completions at build time or runtime. |
| `indicatif` | 0.18.4 | Async progress bars with ETA for pulls/builds | Wire to `tokio` tasks; consider `tracing-indicatif` to drive bars from spans. |
| `ratatui` | 0.30.2 | `spk dashboard` live TUI | Note the 0.30 split into `ratatui` + `ratatui-core`; pin the umbrella `ratatui` crate. Pair with `crossterm` backend. |
| `tracing` | 0.1.44 | Microsecond-resolution structured logging | Core must log via `tracing` events only (no `println!`) to keep Core/presentation separation per PROJECT.md. CLI installs a `tracing-subscriber` formatter. |
| `tracing-subscriber` | 0.3.x | Subscriber/formatter for the CLI frontend | Lives in the CLI binary, NOT the core library — the library only emits events. |

### Development / Distribution Tools

| Tool | Purpose | Notes |
|------|---------|-------|
| Xcode Command Line Tools / SDK 16.4+ | Provides the Virtualization.framework headers `objc2-virtualization` links against | objc2-virtualization 0.3.2 bindings are generated from the Xcode 16.4 SDK. |
| `codesign` (Apple) | Sign the binary with the **`com.apple.security.virtualization` entitlement** | **MANDATORY.** Without this entitlement the process cannot create a `VZVirtualMachine` — it will fail at runtime. See distribution section. |
| `notarytool` + `stapler` | Notarize the signed binary for Gatekeeper | **MANDATORY for Homebrew Cask** as of the Sept 2026 policy (unsigned/un-notarized casks removed from the official tap). |
| Apple Developer ID certificate | Sign + notarize for distribution outside the App Store | Required to obtain a real (non-ad-hoc) signature that can carry the virtualization entitlement and pass notarization. |
| Homebrew | Distribution channel (`brew install`) | Formula for a CLI binary, or Cask if shipping an app bundle. Homebrew re-ad-hoc-signs with `--preserve-metadata=entitlements`, so the entitlement survives. |

## Installation (Cargo dependencies)

# Apple framework bindings (the binding decision)

# Async + gRPC (host ⇄ guest control plane)

# CLI / presentation (CLI crate only — keep out of core lib)

## How to call Virtualization.framework (concrete API map)

| Goal | Class (via `objc2-virtualization`) | Notes |
|------|-----------------------------------|-------|
| Build VM config | `VZVirtualMachineConfiguration` | Set CPU count, memory, boot loader, devices, then `validateWithError:`. |
| Boot Linux directly | `VZLinuxBootLoader` | kernel URL + optional initrd URL + `commandLine`. No firmware. |
| Run the VM | `VZVirtualMachine` | **All methods must be called on one serial `dispatch_queue`** (use `dispatch2`). `start` takes a completion block (`block2`). |
| Track state | `VZVirtualMachineDelegate` | Implement in Rust via `objc2`'s "define an Obj-C class in Rust" support for stop/error callbacks. |
| Host↔guest sockets | `VZVirtioSocketDeviceConfiguration` → at runtime `VZVirtioSocketDevice` | `connectToPort:completionHandler:` (host→guest) yields a `VZVirtioSocketConnection` with a file descriptor you hand to `tonic`/tokio. Use `VZVirtioSocketListener` for guest→host. |
| Share host dirs | `VZVirtioFileSystemDeviceConfiguration` + `VZSharedDirectory` + `VZSingleDirectoryShare`/`VZMultipleDirectoryShare` | The config `tag` is the mount label the guest mounts (`mount -t virtiofs <tag> /mnt`). |
| Networking | `VZVirtioNetworkDeviceConfiguration.attachment = VZFileHandleNetworkDeviceAttachment(fileHandle:)` | The file handle is one end of a `socketpair`; your user-space net stack owns the other end. **Avoid `VZNATNetworkDeviceAttachment`.** |
| Block storage (image store / data disk) | `VZVirtioBlockDeviceConfiguration` + `VZDiskImageStorageDeviceAttachment` | Attach an ext4 image as the containerd content store — simpler than reimplementing a filesystem. |

## Docker API compatibility surface (table-stakes)

| Area | Endpoints | Why needed |
|------|-----------|-----------|
| Handshake | `GET /_ping`, `GET /version`, `GET /info` | testcontainers/Compose probe these on startup; `_ping` returns the `Api-Version` header used for negotiation. |
| Images | `POST /images/create` (pull), `GET /images/json`, `POST /build`, `POST /images/{name}/push`, `DELETE /images/{name}` | Pull/build/list. `build` streams to BuildKit. |
| Containers | `POST /containers/create`, `POST /containers/{id}/start`, `/stop`, `/kill`, `POST /containers/{id}/wait`, `GET /containers/json`, `GET /containers/{id}/json`, `DELETE /containers/{id}`, `GET /containers/{id}/logs` | Core lifecycle. testcontainers needs create/start/wait/logs/inspect. |
| Exec | `POST /containers/{id}/exec`, `POST /exec/{id}/start`, `GET /exec/{id}/json` | testcontainers `execInContainer`, healthchecks. |
| Streams | `POST /containers/{id}/attach` (hijacked stream), `GET /containers/{id}/archive` (copy in/out) | Log/IO streaming and file copy — testcontainers copies files in. |
| Networks/Volumes | `GET /networks`, `POST /networks/create`, `GET /volumes`, `POST /volumes/create` | Compose creates networks/volumes; can be minimal/no-op-ish but must respond sanely. |
| Events | `GET /events` | Compose/testcontainers (Ryuk) watch events. |

## Alternatives Considered

| Recommended | Alternative | When to Use Alternative |
|-------------|-------------|-------------------------|
| `objc2-virtualization` (direct Obj-C FFI) | `swift-bridge` 0.1.59 | Only if you wrap Apple's *Swift* `Containerization` package or need Swift-language async/generic types across the boundary. Adds a Swift toolchain + `.swift` glue to the build. Not needed to call Virtualization.framework. |
| `objc2-virtualization` | `cxx` 1.0.194 | Only for C++ interop. Virtualization is Obj-C, not C++; cxx is the wrong tool here. |
| `VZFileHandleNetworkDeviceAttachment` + user-space stack | `VZNATNetworkDeviceAttachment` | Only for a quick prototype where VPN survival doesn't matter. It creates the synthetic 192.168.64.x NAT subnet that breaks under WARP/VPN — the exact failure Speck targets. |
| `VZFileHandleNetworkDeviceAttachment` | `VZBridgedNetworkDeviceAttachment` | Bridged mode needs the special `com.apple.vm.networking` entitlement (Apple-approved only) and puts the VM directly on the LAN — heavier, still not "inherit host routing," and entitlement is gated. |
| ext4 data disk via virtio-blk for image store | Reimplement ext4 in Rust (Apple's approach) | Only if you need to construct rootfs images on the host without a guest. A real attached ext4 disk is far simpler and battle-tested. |
| Translate to Docker Engine REST API | Expose raw containerd gRPC only | Never for Docker-tool compatibility — containerd gRPC is not Docker-compatible and testcontainers won't work against it. Keep the gRPC seam internally for K8s/CRI. |
| Kata prebuilt arm64 kernel | Build your own kernel | Later, for size/boot optimization once the product works. Start with Kata's container-tuned kernel (Apple does). |

## What NOT to Use

| Avoid | Why | Use Instead |
|-------|-----|-------------|
| QEMU / any emulator | Defeats the millisecond-start brand promise; emulation overhead; PROJECT.md out-of-scope. | Native Virtualization.framework via `objc2-virtualization`. |
| `VZNATNetworkDeviceAttachment` | Synthetic NAT subnet collides with VPN/WARP — the bug Speck exists to fix. | `VZFileHandleNetworkDeviceAttachment` + host user-space stack. |
| x86_64 / Rosetta / cross-compile targets | Apple-Silicon-only by design; zero legacy debt. | `aarch64-apple-darwin` exclusively. |
| swift-bridge/cxx as the primary VZ binding | Adds a second toolchain for an Obj-C framework that has native Rust bindings. | `objc2` + `objc2-virtualization`. |
| Ad-hoc signing for distribution | Cannot carry a real `com.apple.security.virtualization` entitlement chain for notarization; un-notarized casks removed from Homebrew's tap by Sept 2026. | Developer ID signing + notarization. |
| containerd kernel modules for virtio | Guest boots before any module FS — virtio NIC/blk/fs won't init. | Compile VIRTIO drivers **into** the kernel (Kata kernel already does). |
| `println!`/`print!` inside the core library | Violates Core/presentation separation in PROJECT.md; breaks the future SwiftUI/GUI consumer. | Emit `tracing` events; let each frontend install its own subscriber. |

## Apple entitlement & code-signing requirements (MANDATORY — flag for roadmap)

## Version Compatibility

| Package A | Compatible With | Notes |
|-----------|-----------------|-------|
| `objc2-virtualization` 0.3.x | `objc2` 0.6.x, `objc2-foundation` 0.3.x | Whole objc2 ecosystem versions move together; mismatched generations won't interop. Generated from Xcode 16.4 SDK. |
| `tonic` 0.14.x | `prost` 0.14.x | Must match major/minor; tonic 0.14 expects prost 0.14 codegen. |
| `ratatui` 0.30.x | `ratatui-core`, `crossterm` backend | 0.30 split out `ratatui-core`; depend on the umbrella `ratatui` crate, pick the `crossterm` backend feature. |
| `clap` 4.6.x | `clap_complete` 4.6.x | Keep completion crate version aligned with clap. |
| Kata arm64 kernel | `VZLinuxBootLoader` | Kernel must have virtio drivers built-in; Kata's config satisfies this. |

## Sources

- crates.io API (`/api/v1/crates/*`) — **HIGH** — verified current stable versions 2026-06-24: clap 4.6.1, indicatif 0.18.4, ratatui 0.30.2, tracing 0.1.44, objc2 0.6.4, objc2-virtualization 0.3.2, swift-bridge 0.1.59, cxx 1.0.194, tonic 0.14.6, prost 0.14.4, bollard 0.21.0, tokio 1.52.3.
- docs.rs/objc2-virtualization (0.3.2) — **HIGH** — confirmed class coverage (VZVirtualMachine, VZLinuxBootLoader, VZVirtioSocketDevice, VZVirtioFileSystemDeviceConfiguration, VZNATNetworkDeviceAttachment, VZBridgedNetworkDeviceAttachment, VZFileHandleNetworkDeviceAttachment) and delegate traits; bindings generated from Xcode 16.4 SDK.
- github.com/madsmtm/objc2 — **HIGH** — objc2 0.6.x ecosystem, soundness goals, Obj-C-class-in-Rust support, regeneration cadence.
- github.com/apple/containerization + technical-overview — **HIGH** — reference architecture: VM-per-container, Kata kernel recommendation, virtio-built-in requirement, vminitd as PID 1 providing **gRPC over vsock**.
- anil.recoil.org/notes/apple-containerisation — **MEDIUM** — vminitd (Swift static/musl), Kata kernel sourcing, Protobuf-based daemon comms.
- developer.apple.com — VZVirtioFileSystemDeviceConfiguration, VZVirtioSocketDevice, com.apple.security.virtualization — **HIGH** — VirtioFS tag/mount semantics; vsock connect-from-host constraint; virtualization entitlement is a required Boolean.
- lima-vm.io / DeepWiki lima networking + gvisor-tap-vsock — **MEDIUM/HIGH** — the proven host-inheriting pattern: `VZFileHandleNetworkDeviceAttachment` → user-space gVisor netstack via `PassFDToUnix()`, plus host-resolver DNS that "deals correctly with VPN configurations and split-DNS." This validates Speck's networking design.
- docs.docker.com Engine API + Testcontainers requirements — **HIGH** — Unix socket default, API version negotiation/min ~v1.24, "containerd does not expose a Docker-compatible API," testcontainers needs Docker-API-compatible runtime.
- Homebrew discussions #5744 / #4725 + Workbrew "Homebrew 5.0.0" — **MEDIUM/HIGH** — entitlements preserved via `--preserve-metadata=entitlements` on re-sign; un-notarized casks removed from official tap ~Sept 2026; `com.apple.vm.networking` is an Apple-gated entitlement.

<!-- GSD:stack-end -->

<!-- GSD:conventions-start source:CONVENTIONS.md -->

## Conventions

Conventions not yet established. Will populate as patterns emerge during development.
<!-- GSD:conventions-end -->

<!-- GSD:architecture-start source:ARCHITECTURE.md -->

## Architecture

Architecture not yet mapped. Follow existing patterns found in the codebase.
<!-- GSD:architecture-end -->

<!-- GSD:skills-start source:skills/ -->

## Project Skills

No project skills found. Add skills to any of: `.claude/skills/`, `.agents/skills/`, `.cursor/skills/`, `.github/skills/`, or `.codex/skills/` with a `SKILL.md` index file.
<!-- GSD:skills-end -->

<!-- GSD:workflow-start source:GSD defaults -->

## GSD Workflow Enforcement

Before using Edit, Write, or other file-changing tools, start work through a GSD command so planning artifacts and execution context stay in sync.

Use these entry points:

- `/gsd:quick` for small fixes, doc updates, and ad-hoc tasks
- `/gsd:debug` for investigation and bug fixing
- `/gsd:execute-phase` for planned phase work

Do not make direct repo edits outside a GSD workflow unless the user explicitly asks to bypass it.
<!-- GSD:workflow-end -->

<!-- GSD:profile-start -->

## Developer Profile

> Profile not yet configured. Run `/gsd:profile-user` to generate your developer profile.
> This section is managed by `generate-claude-profile` -- do not edit manually.
<!-- GSD:profile-end -->

## Task Master AI Instructions
**Import Task Master's development workflow commands and guidelines, treat as if import is in the main CLAUDE.md file.**
@./.taskmaster/CLAUDE.md
