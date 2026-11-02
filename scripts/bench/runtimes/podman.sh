# shellcheck shell=bash
# Podman adapter: machine $BENCH_PROFILE, Docker-compatible API socket.
# Create the bench machine once with
#   podman machine init bench --cpus 4 --memory 4096 --disk-size 30 --rootful
podman_machine() {
    if [ "$BENCH_PROFILE" = default ]; then echo podman-machine-default; else echo "$BENCH_PROFILE"; fi
}

rt_start() { podman machine start "$(podman_machine)" >/dev/null 2>&1; }

rt_stop() { podman machine stop "$(podman_machine)" >/dev/null 2>&1; }

rt_socket() {
    echo "unix://$(podman machine inspect "$(podman_machine)" --format '{{.ConnectionInfo.PodmanSocket.Path}}')"
}

rt_version() { podman --version | awk '{print $3}'; }

rt_pids() {
    # vfkit and gvproxy carry the machine name on their command lines; the
    # guest memory lives in the Virtualization.framework process that holds
    # the disk.
    local m; m="$(podman_machine)"
    { pgrep -f "(vfkit|krunkit|gvproxy).*$m"
      pids_of_files "$HOME/.local/share/containers/podman/machine/applehv/$m-arm64.raw"
    } | sort -u
}
