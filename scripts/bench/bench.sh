#!/usr/bin/env bash
# Compare Speck with Colima and Podman on this Mac: memory and CPU on the
# host, hangs, speed, and Docker compatibility.
#
# Usage: scripts/bench/bench.sh [--runtimes speck,colima,podman] [--suites resources,speed,compat] [--quick]
#   SPK=target/aarch64-apple-darwin/release/spk   use another spk binary
#
# Each runtime runs alone, in its own "bench" profile with 4 CPUs and 4 GiB,
# so your default instances stay untouched. Stop the other runtimes first:
# they compete for the same CPU and memory. Results go to
# ~/.cache/speck-bench/<timestamp>/ (results.ndjson, report.md).
set -u

BENCH_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$BENCH_DIR/../.." && pwd)"
. "$BENCH_DIR/lib.sh"

RUNTIMES="speck,colima,podman"
SUITES="resources,speed,compat"
QUICK=0
while [ $# -gt 0 ]; do
    case "$1" in
        --runtimes) RUNTIMES="$2"; shift 2 ;;
        --suites) SUITES="$2"; shift 2 ;;
        --quick) QUICK=1; shift ;;
        *) echo "usage: $0 [--runtimes a,b] [--suites resources,speed,compat] [--quick]" >&2; exit 2 ;;
    esac
done
has_suite() { [[ ",$SUITES," == *",$1,"* ]]; }
# --quick: fewer repetitions, for checking the harness itself.
reps() { if [ "$QUICK" -eq 1 ]; then echo "$2"; else echo "$1"; fi; }

OUT="$HOME/.cache/speck-bench/$(date +%Y%m%d-%H%M%S)"
WORK="$HOME/.cache/speck-bench/work"
mkdir -p "$OUT" "$WORK"
RESULTS="$OUT/results.ndjson"
: > "$RESULTS"
unset DOCKER_CONTEXT

# Host files for the bind-mount tests: a node_modules-like tree and a 512 MiB file.
prepare_work() {
    [ -d "$WORK/tree" ] || python3 - "$WORK/tree" <<'EOF'
import os, sys
root = sys.argv[1]
for d in range(400):
    path = os.path.join(root, f"pkg{d}", "lib")
    os.makedirs(path)
    for f in range(25):
        with open(os.path.join(path, f"f{f}.js"), "w") as fh:
            fh.write("module.exports = %d;\n" % f * 40)
EOF
    [ -f "$WORK/blob" ] || dd if=/dev/urandom of="$WORK/blob" bs=1048576 count=512 2>/dev/null
}

docker_ok() { "$TIMEOUT_BIN" 10 docker info >/dev/null 2>&1; }

# Host scheduling jitter: how late 10 ms sleeps wake up, p99 in ms. High
# values mean the Mac itself feels sluggish.
host_jitter_p99() {
    perl -MTime::HiRes=time,sleep -e '
        my @late; for (1..1000) { my $t = time; sleep 0.01; push @late, (time - $t - 0.01) * 1000 }
        @late = sort { $a <=> $b } @late; printf "%.1f\n", $late[989]'
}

suite_resources() {
    echo "resources"
    local pids cpu0 cpu1 peak fp i
    sleep "$(reps 30 5)"
    pids="$(rt_pids)"
    if [ -z "$pids" ]; then note_failure "find host processes" 1; return; fi
    # shellcheck disable=SC2086
    metric idle_memory "$(footprint_mb $pids)" MiB lower "host footprint after start"
    # shellcheck disable=SC2086
    cpu0="$(cpu_seconds $pids)"; sleep "$(reps 60 10)"
    # shellcheck disable=SC2086
    cpu1="$(cpu_seconds $pids)"
    metric idle_cpu "$(echo "$cpu0 $cpu1 $(reps 60 10)" | awk '{printf "%.1f", ($2 - $1) / $3 * 100}')" '%' lower "one core = 100%"
    metric host_jitter_idle "$(host_jitter_p99)" ms lower "p99 lateness of 10 ms sleeps on the Mac"

    # 2 GiB touched in a container: how much the Mac pays, and whether it
    # gets the memory back after the container exits.
    docker rm -f bench-mem >/dev/null 2>&1
    docker run -d --name bench-mem python:3.12-alpine python3 -c '
import time
b = bytearray(2 * 1024**3)
for i in range(0, len(b), 4096): b[i] = 1
time.sleep(25)' >/dev/null || { note_failure "memory load" 1; return; }
    peak=0
    for i in 1 2 3 4 5; do
        sleep 4
        # shellcheck disable=SC2086
        # shellcheck disable=SC2046
        fp="$(footprint_mb $(rt_pids))"; [ "$fp" -gt "$peak" ] && peak="$fp"
    done
    metric load_memory "$peak" MiB lower "peak while a container holds 2 GiB"
    local code rc
    code="$("$TIMEOUT_BIN" 120 docker wait bench-mem 2>/dev/null)"; rc=$?
    if [ $rc -ne 0 ]; then note_failure "memory load wait" $rc
    elif [ "$code" != 0 ]; then note_failure "memory load: the container could not hold 2 GiB (exit $code)" 1; fi
    docker rm -f bench-mem >/dev/null 2>&1
    sleep "$(reps 90 15)"
    # shellcheck disable=SC2086
    # shellcheck disable=SC2046
    metric after_load_memory "$(footprint_mb $(rt_pids))" MiB lower "$(reps 90 15) s after that container exited"

    # Busy loops on every vCPU: does the Mac stay responsive?
    docker rm -f bench-cpu >/dev/null 2>&1
    docker run -d --name bench-cpu alpine sh -c 'for i in 1 2 3 4; do (while :; do :; done) & done; sleep 40' >/dev/null \
        || { note_failure "cpu load" 1; return; }
    sleep 3
    metric host_jitter_load "$(host_jitter_p99)" ms lower "p99 lateness while 4 vCPUs spin"
    docker rm -f bench-cpu >/dev/null 2>&1
}

suite_speed() {
    echo "speed"
    hf run_container "$(reps 10 3)" "docker run --rm alpine true"
    docker rm -f bench-exec >/dev/null 2>&1
    docker run -d --name bench-exec alpine sleep 600 >/dev/null
    hf exec "$(reps 20 3)" "docker exec bench-exec true"
    docker rm -f bench-exec >/dev/null 2>&1

    local ms rc
    docker rmi node:22-alpine >/dev/null 2>&1
    ms="$(timed 600 docker pull node:22-alpine)"; rc=$?
    if [ $rc -eq 0 ]; then metric pull_node_alpine "$((ms / 1000)).$(( (ms % 1000) / 100 ))" s lower "network dependent"; else note_failure pull $rc; fi

    printf 'FROM alpine:latest\nRUN apk add --no-cache build-base\n' > "$WORK/Dockerfile"
    ms="$(timed 600 docker build --no-cache -q -t bench-build "$WORK")"; rc=$?
    if [ $rc -eq 0 ]; then metric build_apk "$((ms / 1000)).$(( (ms % 1000) / 100 ))" s lower "apk add build-base, no cache"; else note_failure build $rc; fi

    hf cpu_loop "$(reps 3 1)" "docker run --rm alpine sh -c 'i=0; while [ \$i -lt 2000000 ]; do i=\$((i+1)); done'"

    # Bind mounts: read and copy 10 000 small files, write and read 512 MiB.
    hf bind_read_small "$(reps 3 1)" "docker run --rm -v $WORK/tree:/w:ro alpine tar -cf /dev/null -C /w ."
    hf bind_write_small "$(reps 3 1)" "docker run --rm -v $WORK:/w alpine sh -c 'rm -rf /w/copy && cp -r /w/tree /w/copy'"
    hf bind_write_big "$(reps 3 1)" "docker run --rm -v $WORK:/w alpine dd if=/dev/zero of=/w/big bs=1M count=512 conv=fsync"
    hf bind_read_big "$(reps 3 1)" "docker run --rm -v $WORK:/w:ro alpine dd if=/w/blob of=/dev/null bs=1M"
    rm -rf "$WORK/copy" "$WORK/big"

    # Network: 512 MiB from the Mac into a container, and a published port.
    local server port=18731
    (cd "$WORK" && exec python3 -m http.server "$port" --bind 0.0.0.0 >/dev/null 2>&1) & server=$!
    sleep 1
    hf net_from_host "$(reps 3 1)" "docker run --rm --add-host benchhost:host-gateway alpine wget -q -O /dev/null http://benchhost:$port/blob"
    kill "$server" 2>/dev/null
    docker rm -f bench-web >/dev/null 2>&1
    docker run -d --name bench-web -p 127.0.0.1:18732:80 nginx:alpine >/dev/null
    sleep 2
    hf published_port "$(reps 50 5)" "curl -sf http://127.0.0.1:18732/"
    docker rm -f bench-web >/dev/null 2>&1
}

suite_compat() {
    echo "compat"
    local out
    for gate in conformance egress; do
        out="$(CONFORMANCE_DOCKER_HOST="$DOCKER_HOST" EGRESS_DOCKER_HOST="$DOCKER_HOST" \
            "$TIMEOUT_BIN" 900 "$REPO_DIR/scripts/$gate-smoke.sh" 2>&1)"
        echo "$out" > "$OUT/$RUNTIME-$gate.log"
        local passed failed
        passed="$(echo "$out" | rg -o '([0-9]+) passed' -r '$1' | tail -1)"
        failed="$(echo "$out" | rg -o '([0-9]+) failed' -r '$1' | tail -1)"
        metric "${gate}_failed" "${failed:-?}" checks lower "$((${passed:-0} + ${failed:-0})) checks; see $RUNTIME-$gate.log"
    done
}

bench_runtime() {
    RUNTIME="$1"
    HANGS=0; FAILURES=0
    load_runtime "$RUNTIME"
    echo "== $RUNTIME $(rt_version)"
    metric version "$(rt_version)" "" info

    rt_stop
    local ms rc starts=() i
    for i in $(seq "$(reps 3 1)"); do
        [ "$i" -gt 1 ] && rt_stop && sleep 5
        ms="$(now_ms)"; rt_start; rc=$?; ms=$(( $(now_ms) - ms ))
        [ $rc -eq 0 ] || { note_failure "start" $rc; return; }
        starts+=("$ms")
    done
    export DOCKER_HOST; DOCKER_HOST="$(rt_socket)"
    for i in $(seq 60); do docker_ok && break; sleep 1; done
    metric start "$(printf '%s\n' "${starts[@]}" | sort -n | awk '{a[NR]=$1} END {printf "%.1f", a[int((NR+1)/2)] / 1000}')" s lower \
        "median of ${#starts[@]} starts from stopped"

    for image in alpine:latest python:3.12-alpine nginx:alpine docker:cli; do
        "$TIMEOUT_BIN" 600 docker pull -q "$image" >/dev/null 2>&1 || note_failure "pull $image" $?
    done

    has_suite resources && suite_resources
    has_suite speed && suite_speed
    has_suite compat && suite_compat

    metric hangs "$HANGS" steps lower "steps that ran past their timeout"
    metric failures "$FAILURES" steps lower "steps that failed"
    rt_stop
}

echo "bench: results in $OUT"
prepare_work
for rt in ${RUNTIMES//,/ }; do
    ( bench_runtime "$rt" )
done
"$BENCH_DIR/report.sh" "$RESULTS" | tee "$OUT/report.md"
