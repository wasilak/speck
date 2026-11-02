#!/usr/bin/env bash
# Long-running watch of runtimes you already use: every INTERVAL seconds,
# record each one's host memory and CPU, whether a container starts, and
# whether a container reaches the network. Leave it running for days, then
# summarize. Memory that only grows, failed probes and hangs show up here.
#
# Usage: scripts/bench/watch.sh speck colima        (Ctrl-C to stop)
#        scripts/bench/watch.sh --summary ~/.cache/speck-bench/watch-<date>.csv
#   INTERVAL=60                    seconds between samples
#   WATCH_PROFILE=default          profile of each runtime to watch
#   WATCH_URL=https://github.com   URL the network probe fetches (use an internal host at work)
set -u

BENCH_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck disable=SC2034  # read by lib.sh and the adapters
BENCH_PROFILE="${WATCH_PROFILE:-default}"
. "$BENCH_DIR/lib.sh"

if [ "${1:-}" = --summary ]; then
    # Per runtime: samples, failed and hung probes, memory first/max/last.
    awk -F, 'NR > 1 {
        rt = $2; n[rt]++
        if ($5 != "ok") run_fail[rt]++
        if ($5 == "hang") hang[rt]++
        if ($7 != "ok") net_fail[rt]++
        if (!(rt in first)) first[rt] = $3
        if ($3 > max[rt]) max[rt] = $3
        last[rt] = $3; cpu[rt] += $4
    } END {
        printf "%-8s %8s %9s %6s %9s %10s %8s %9s %8s\n", "runtime", "samples", "run_fail", "hangs", "net_fail", "mem_first", "mem_max", "mem_last", "cpu_avg"
        for (rt in n) printf "%-8s %8d %9d %6d %9d %9dM %7dM %8dM %7.1f%%\n", rt, n[rt], run_fail[rt], hang[rt], net_fail[rt], first[rt], max[rt], last[rt], cpu[rt] / n[rt]
    }' "$2"
    exit 0
fi

[ $# -gt 0 ] || { echo "usage: $0 <runtime>... | --summary <csv>" >&2; exit 2; }
INTERVAL="${INTERVAL:-60}"
WATCH_URL="${WATCH_URL:-https://github.com}"
CSV="$HOME/.cache/speck-bench/watch-$(date +%Y%m%d-%H%M%S).csv"
mkdir -p "$(dirname "$CSV")"
echo "time,runtime,memory_mib,cpu_pct,run,run_ms,net" > "$CSV"
rm -f /tmp/speck-watch-*.cpu
echo "watch: every ${INTERVAL}s, writing $CSV"
unset DOCKER_CONTEXT

while :; do
    for rt in "$@"; do
        (
            load_runtime "$rt"
            export DOCKER_HOST; DOCKER_HOST="$(rt_socket)"
            pids="$(rt_pids)"
            # shellcheck disable=SC2086
            mem="$(footprint_mb $pids)"
            # shellcheck disable=SC2086
            echo "$(cpu_seconds $pids) $(date +%s)" > "/tmp/speck-watch-$rt.cpu.new"
            ms="$(timed 30 docker run --rm alpine true)"; rc=$?
            case $rc in 0) run=ok ;; 124) run=hang ;; *) run=fail ;; esac
            "$TIMEOUT_BIN" 30 docker run --rm alpine wget -q -T 10 -O /dev/null "$WATCH_URL" >/dev/null 2>&1 \
                && net=ok || net=fail
            echo "$(date +%Y-%m-%dT%H:%M:%S),$rt,$mem,CPU,$run,$ms,$net"
        ) > "/tmp/speck-watch-$rt.row"
        now_cpu="$(cat "/tmp/speck-watch-$rt.cpu.new" 2>/dev/null || echo "0 0")"
        prev="$(cat "/tmp/speck-watch-$rt.cpu" 2>/dev/null || echo "$now_cpu")"
        # CPU seconds used per wall second since the previous sample.
        cpu="$(echo "$prev $now_cpu" | awk '{d = $3 - $1; t = $4 - $2; if (d < 0 || t <= 0) d = 0; printf "%.1f", (t > 0 ? d / t : 0) * 100}')"
        echo "$now_cpu" > "/tmp/speck-watch-$rt.cpu"
        sed "s/,CPU,/,$cpu,/" "/tmp/speck-watch-$rt.row" | tee -a "$CSV"
    done
    sleep "$INTERVAL"
done
