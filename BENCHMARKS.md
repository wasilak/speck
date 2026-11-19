# Speck vs Colima vs Podman: benchmark results

Measured on 2026-11-02 and 2026-11-03 with `scripts/bench/bench.sh`. The method and every metric are described in [docs/benchmarking.md](docs/benchmarking.md).

## Setup

- MacBook Pro, Apple M3 Pro (12 cores), 36 GiB RAM, macOS 27.0.
- Each runtime ran alone, in its own instance with 4 vCPUs and 4 GiB. The other runtimes were stopped.
- All three run on Apple Virtualization.framework.

| Runtime | Version | Guest stack |
|---|---|---|
| Speck | 1.5.2, before and after the fsync fix (`501af1a`) | Kata kernel 6.18, dockerd 26.1.5, containerd 1.7.17, runc 1.1.14 |
| Colima | 0.10.3 (`--vm-type vz`) | Ubuntu kernel 6.8, docker 29.5.2, containerd 2.2.4, runc 1.3.5 |
| Podman | 6.1.4 (applehv, rootful) | Podman machine, Buildah for builds |

Speck and Colima both use the containerd image store in dockerd.

## Results

Lower is better in every row. Times are medians; the repeat count is in `docs/benchmarking.md`.

| Metric | Speck before fix | Speck after fix | Colima | Podman |
|---|---|---|---|---|
| Start, stopped → Docker API ready | 2.4 s | **2.3 s** | 10.7 s | 10.4 s |
| Host memory after start | **444 MiB** | **444 MiB** | 1077 MiB | 1166 MiB |
| Host memory, container holding 2 GiB | 2534 MiB | **2533 MiB** | 3075 MiB | 2931 MiB |
| Host memory 90 s after that container exits | 2534 MiB | **2533 MiB** | 3075 MiB | 2932 MiB |
| Idle CPU (one core = 100%) | 1.7% | 1.3% | **0.3%** | 1.0% |
| Mac jitter at rest, p99 | 5.1 ms | **2.6 ms** | 2.9 ms | **2.6 ms** |
| Mac jitter, 4 vCPUs busy, p99 | 5.0 ms | 2.6 ms | **2.5 ms** | 4.2 ms |
| `docker run --rm alpine true` | 427 ms | 175 ms | **128 ms** | 180 ms |
| `docker exec` | 35 ms | 40 ms | **30 ms** | 76 ms |
| Pull `node:22-alpine` | 3.8 s | 3.5 s | 3.8 s | **3.3 s** |
| Build (`apk add build-base`, no cache) | 11.0 s | 11.1 s | 9.0 s | **4.2 s** |
| CPU loop in a container | 2472 ms | 2430 ms | **2179 ms** | 2297 ms |
| Bind mount: read 10 000 small files | 1480 ms | 1371 ms | **1249 ms** | 1442 ms |
| Bind mount: copy 10 000 small files | 4295 ms | 6614 ms | **4009 ms** | 4498 ms |
| Bind mount: write 512 MiB | 654 ms | 515 ms | 409 ms | **400 ms** |
| Bind mount: read 512 MiB | 573 ms | 356 ms | 266 ms | **259 ms** |
| 512 MiB from the Mac into a container | 787 ms | **563 ms** | 1284 ms | 2911 ms |
| HTTP request to a published port | **13 ms** | 16 ms | 17 ms | **13 ms** |
| Failed conformance checks (of 30) | **0** | **0** | 7 | 8 |
| Failed egress checks (of 25) | **0** | **0** | 1 | 2 |
| Hangs / failed steps | **0 / 0** | **0 / 0** | **0 / 0** | **0 / 0** |

### Notes on the results

- **Idle CPU.** The harness measures 30 to 90 s after start, so boot work is still included. Measured by hand about 2 minutes after start, Speck used 0.26% (daemon 0.03%, VM 0.23%).
- **Copying small files after the fix (6614 ms)** has a p95 of 8547 ms, so the run was noisy. A rerun on 2026-11-03 gave 3.5 to 4.3 s for Speck and 3.0 to 3.2 s for Colima, about 15% apart.
- **Conformance and egress checks** were written for Speck. They test Speck's own promises, such as `HostIp` in `docker port`, UDP publishing, socket mounts and ICMP. Failures for Colima and Podman show where they behave differently. They are not a general quality score.
  - Colima failed: port inspect `HostIp`, `docker port` output, UDP port publish, `DOCKER_HOST` socket mount, push to, pull from and verify against a local registry, and ICMP not faked.
  - Podman failed: port inspect `HostIp`, `docker port` output, docker socket mount, `DOCKER_HOST` socket mount, push, pull and verify against a local registry, `rmi` of a local image, and two ICMP pings (8000 B fragmented, gateway).

## What changed between the two Speck runs

Commit `501af1a` attaches the VM disks with `VZDiskImageSynchronizationMode.fsync` instead of the default full fsync (`F_FULLFSYNC`). The default mode makes every flush from the guest wait for the drive's own cache.

- 100 × 4 KiB writes with fsync in the guest: 0.64 s before, 0.04 s after. Colima: 0.067 s.
- `docker run`: 427 ms → 175 ms.
- Mac jitter p99: 5.1 ms → 2.6 ms.

Risk: if the Mac loses power suddenly, data still in the drive's cache can be lost. Lima, which Colima is built on, is believed to use the same mode; this was not checked in Lima's source.

## Follow-up measurements

### Builds: the time goes to gzip, not to the file system

Breakdown of the same build on Speck, after the fix:

| Step | Time |
|---|---|
| `apk add build-base` as a plain `docker run` | 2.6–3.3 s |
| of which: download | 2.0 s |
| of which: install | 0.7 s |
| `docker build` total | 8.9–9.5 s |
| of which: `RUN apk add` | 2.1 s |
| of which: export of the 247 MiB layer | 6.6 s (5.4 s of it gzip) |

Network and disk are not the bottleneck. The containerd image store keeps layers compressed, and BuildKit compresses them with the single-threaded gzip from Go's standard library. The default is hard-coded in BuildKit (`var Default = Gzip`). Neither dockerd nor `buildkitd.toml` has a setting to change it.

The same build with only the layer compression changed:

| Compression | Layer export | Build total |
|---|---|---|
| gzip (default) | 5.3 s | 8.85 s |
| zstd | 0.9 s | 4.55 s |
| uncompressed | 0.9 s | 3.56 s |

How the other runtimes handle this:

| | Image store | Layer export | Build total | Multi-platform builds |
|---|---|---|---|---|
| Speck, containerd store (default) | compressed blobs | 5.3 s | 8.9–9.5 s | yes |
| Speck, overlay2 (test only) | unpacked layers | 0.5–0.7 s | 4.0 s | no: dockerd asks for the containerd store |
| Colima 0.10.3, docker 29.5.2 | compressed blobs | 4.9 s | 8.0 s | yes |
| Podman 6.1.4 | unpacked layers + `tar-split.gz` | none | 4.0–4.8 s | yes: `--platform … --manifest` built amd64 + arm64 in 15.9 s |

Podman keeps layers unpacked and stores a small `tar-split.gz` per layer. It rebuilds the exact tar and compresses it only on push. Docker has to choose: overlay2 is fast but cannot hold multi-platform images, and the containerd store can hold them but compresses every layer at build time.

Speck keeps the containerd store, because multi-platform builds matter more than this speed. For local builds, `docker build -o type=image,name=myimage,compression=zstd .` halves the build time (see the README).

The remaining gap to Colima, about 1 s, matches the CPU loop result (Speck about 10% slower). The cause is not investigated yet.

### Bind mounts

Speck and Colima mount VirtioFS with the same options (`virtiofs rw,relatime`), and all three runtimes use Apple's VirtioFS server. The differences come from the guest side. Possible causes are the guest kernel (Kata 6.18 vs Ubuntu 6.8) and per-container start overhead. Neither is verified.

### Memory is not returned to the Mac

After a container that held 2 GiB exits, no runtime gives the memory back to the Mac (compare the "90 s after" row). A memory balloon was tested in Speck:

- The guest released about 3.2 GiB within 1 s.
- The host footprint of the VM stayed at 2507 MiB for 20 s.

Virtualization.framework does not release ballooned pages to the host. The VZ balloon device offers only `MUST_TELL_HOST` and `DEFLATE_ON_OOM`, with no free page reporting. The same limit is reported for Lima ([lima#2789](https://github.com/lima-vm/lima/issues/2789)). The experiment was reverted.

## Caveats

- One Mac, one run per runtime for the full table. Small differences (under about 10%) are within noise.
- Pull, build and network rows depend on the internet connection at the time.
- Speck's numbers come from a build of `main`, not from a released binary. The fsync fix is not released yet.
- Raw results are in `~/.cache/speck-bench/20261102-*` on the test Mac and are not in this repo.

## Reproduce

```bash
scripts/bench/bench.sh                                  # all runtimes, about 40 minutes
scripts/bench/bench.sh --runtimes speck --suites speed  # one runtime, one suite
```

Each runtime needs its own `bench` instance. The setup is in [docs/benchmarking.md](docs/benchmarking.md).
