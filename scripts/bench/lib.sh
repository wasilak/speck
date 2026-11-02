# shellcheck shell=bash
# Shared helpers for scripts/bench. Sourced, not run.
#
# Every runtime adapter (runtimes/<name>.sh) defines:
#   rt_start    start the bench instance; return when the Docker API answers
#   rt_stop     stop it
#   rt_socket   print the Docker API socket (unix://...)
#   rt_version  print the runtime version
#   rt_pids     print the host PIDs of the instance (VM process + daemons)

BENCH_PROFILE="${BENCH_PROFILE:-bench}"

TIMEOUT_BIN="$(command -v timeout || command -v gtimeout || true)"
[ -n "$TIMEOUT_BIN" ] || { echo "bench: needs timeout (brew install coreutils)" >&2; exit 2; }
for tool in jq hyperfine footprint lsof perl python3; do
    command -v "$tool" >/dev/null || { echo "bench: needs $tool" >&2; exit 2; }
done

now_ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time * 1000'; }

load_runtime() {
    local name="$1"
    # shellcheck source=/dev/null
    . "$BENCH_DIR/runtimes/$name.sh" || { echo "bench: unknown runtime $name" >&2; exit 2; }
}

# pids_of_files FILE...: PIDs that hold any of the files open (the VM
# process holds its disk images).
pids_of_files() {
    local f
    for f in "$@"; do [ -e "$f" ] && lsof -t "$f" 2>/dev/null; done | sort -u
}

# footprint_mb PID...: summed physical footprint in MiB, the number
# Activity Monitor shows as Memory. For a VM process it includes the guest
# memory the VM has touched; `ps rss` does not.
footprint_mb() {
    local pid total=0 line value
    for pid in "$@"; do
        line="$(footprint -p "$pid" 2>/dev/null | rg -o 'Footprint: [0-9.]+ [KMG]?B' | head -1)"
        [ -n "$line" ] || continue
        value="$(echo "$line" | awk '{v=$2; u=$3; if (u=="KB") v/=1024; else if (u=="GB") v*=1024; else if (u=="B") v/=1048576; print v}')"
        total="$(echo "$total $value" | awk '{print $1 + $2}')"
    done
    printf '%.0f\n' "$total"
}

# cpu_seconds PID...: summed CPU time used so far (the VM process includes
# the guest vCPU threads).
cpu_seconds() {
    local pid
    for pid in "$@"; do ps -o time= -p "$pid" 2>/dev/null; done | awk '
        { n = split($1, p, ":"); s = 0; for (i = 1; i <= n; i++) s = s * 60 + p[i]; total += s }
        END { printf "%.2f\n", total + 0 }'
}

# metric NAME VALUE UNIT BETTER [NOTE]: append one result line.
# BETTER is "lower", "higher" or "info".
metric() {
    jq -nc --arg runtime "$RUNTIME" --arg metric "$1" --arg value "$2" --arg unit "$3" \
        --arg better "$4" --arg note "${5:-}" \
        '{runtime: $runtime, metric: $metric, value: ($value | tonumber? // $value),
          unit: $unit, better: $better, note: $note}' >> "$RESULTS"
    printf '  %-28s %10s %s\n' "$1" "$2" "$3"
}

# timed SECONDS CMD...: run CMD with a timeout, print elapsed ms. Returns
# the command status; 124 means it hung past the timeout.
timed() {
    local limit="$1"; shift
    local start rc
    start="$(now_ms)"
    "$TIMEOUT_BIN" "$limit" "$@" >/dev/null 2>&1
    rc=$?
    echo $(( $(now_ms) - start ))
    return $rc
}

HANGS=0
FAILURES=0
# note_failure NAME STATUS: count a failed or hung step.
note_failure() {
    if [ "$2" -eq 124 ]; then
        HANGS=$((HANGS + 1)); echo "  HANG  $1 (timed out)"
    else
        FAILURES=$((FAILURES + 1)); echo "  FAIL  $1 (exit $2)"
    fi
}

# hf NAME RUNS CMD: hyperfine median in ms, as a metric.
hf() {
    local name="$1" runs="$2" cmd="$3" json
    json="$(mktemp)"
    if hyperfine --style none --warmup 2 --runs "$runs" --export-json "$json" "$cmd" >/dev/null 2>&1; then
        # hyperfine 1.x has .median and .times; 2.x has .summary and .measurements.
        local stats='.results[0] | {median: (.median // .summary.time_wall_clock.median),
            times: (.times // [.measurements[].time_wall_clock.value])}'
        metric "$name" "$(jq "$stats | .median * 1000 | floor" "$json")" ms lower \
            "median of $runs, p95 $(jq "$stats | .times | sort | .[(length * 0.95 | floor)] * 1000 | floor" "$json") ms"
    else
        note_failure "$name" 1
    fi
    rm -f "$json"
}
