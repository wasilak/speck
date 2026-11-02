# shellcheck shell=bash
# Speck adapter: profile $BENCH_PROFILE (bench: 4 CPUs, 4 GiB, no launchd agent).
SPK="${SPK:-spk}"

speck_home() {
    if [ "$BENCH_PROFILE" = default ]; then echo "$HOME/.speck"; else echo "$HOME/.speck/profiles/$BENCH_PROFILE"; fi
}

rt_start() {
    if [ "$BENCH_PROFILE" = default ]; then
        "$SPK" start --wait --timeout 300 >/dev/null
    else
        # The sizes go on every start: start saves them to config.yaml.
        "$SPK" -p "$BENCH_PROFILE" start --cpus 4 --memory 4g --no-autostart --wait --timeout 300 >/dev/null 2>&1
    fi
}

rt_stop() { "$SPK" -p "$BENCH_PROFILE" stop >/dev/null 2>&1; }

rt_socket() { echo "unix://$(speck_home)/speck.sock"; }

rt_version() { "$SPK" --version | awk '{print $2}'; }

rt_pids() {
    local home; home="$(speck_home)"
    { cat "$home/run/speck.pid" 2>/dev/null; echo; pids_of_files "$home/data.img"; } | rg '^[0-9]+$' | sort -u
}
