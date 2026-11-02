# Benchmarking Speck against Colima and Podman

`scripts/bench/` answers three questions on a real Mac:

- How much memory and CPU does each runtime take from the Mac?
- Does anything hang or fail?
- Is Speck faster for everyday work?

## Prerequisites

- `brew install hyperfine podman colima` (`jq`, `coreutils`, `lsof` and `footprint` come with the usual setup).
- One bench profile per runtime, each with 4 CPUs and 4 GiB. Your default instances stay untouched.
  - Speck: the script creates profile `bench` itself.
  - Colima: `colima start bench --vm-type vz --cpu 4 --memory 4 --disk 30 --runtime docker`, then `colima stop bench`.
  - Podman: `podman machine init bench --cpus 4 --memory 4096 --disk-size 30 --rootful`.
- Stop other runtimes during a run. They compete for the same CPU and memory.

## One-off comparison

```bash
scripts/bench/bench.sh                                  # all runtimes, all suites (about 40 minutes)
scripts/bench/bench.sh --runtimes speck,colima --suites resources
scripts/bench/bench.sh --quick                          # fewer repetitions, to check the harness
```

The script runs one runtime at a time: it starts the runtime, runs the suites, and stops it.
Results go to `~/.cache/speck-bench/<timestamp>/`: `results.ndjson`, `report.md` and the gate logs.
The best value in each row of `report.md` is in bold.

| Metric | What it measures |
|---|---|
| `start` | Seconds from stopped until the Docker API answers (median of 3). |
| `idle_memory` | Host memory of the runtime's processes after start, as Activity Monitor counts it (`footprint`). This includes guest memory the VM has touched. |
| `load_memory`, `after_load_memory` | Peak memory while a container holds 2 GiB, and the memory 90 s after it exits. A high `after_load_memory` means the Mac does not get the memory back. |
| `idle_cpu` | CPU the runtime uses while nothing runs (100% = one core). This affects the battery. |
| `host_jitter_idle`, `host_jitter_load` | p99 lateness of 10 ms sleeps on the Mac, at rest and while 4 vCPUs spin. A high value means the Mac feels sluggish. |
| `run_container`, `exec`, `cpu_loop` | Container start, `docker exec` and a CPU loop (hyperfine medians). |
| `pull_node_alpine`, `build_apk` | Image pull and an `apk add` build. These depend on the network. |
| `bind_*` | Bind mounts: read and copy 10 000 small files, write and read 512 MiB. |
| `net_from_host`, `published_port` | 512 MiB from the Mac into a container, and an HTTP request to a published port. |
| `conformance_failed`, `egress_failed` | Failed checks of `scripts/conformance-smoke.sh` and `scripts/egress-smoke.sh`. |
| `hangs`, `failures` | Steps that timed out or failed. |

## Long-running watch

Short benchmarks do not show slow memory growth or failures after sleep or a VPN change. For those, leave `watch.sh` running on the instances you use every day:

```bash
WATCH_URL=https://internal.example.com scripts/bench/watch.sh speck colima
```

Every 60 s it records, per runtime: host memory, CPU, whether `docker run` works (`ok`, `fail` or `hang`), and whether a container reaches `WATCH_URL`. It writes `~/.cache/speck-bench/watch-<timestamp>.csv`. After a few days, summarize it:

```bash
scripts/bench/watch.sh --summary ~/.cache/speck-bench/watch-<timestamp>.csv
```

Memory that only grows (`mem_first` far below `mem_last`), and any `run_fail`, `hangs` or `net_fail`, are the things to report.

## Troubleshooting

- `bench: needs <tool>`: install the tool that is named.
- A runtime shows a very low `idle_memory`: its adapter in `scripts/bench/runtimes/` did not find the VM process. Each adapter finds the VM process through the open disk image.
- `start` fails: start the runtime by hand once and read its own error.
