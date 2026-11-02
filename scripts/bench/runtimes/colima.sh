# shellcheck shell=bash
# Colima adapter: profile $BENCH_PROFILE. Create the bench profile once with
#   colima start bench --vm-type vz --cpu 4 --memory 4 --disk 30 --runtime docker
colima_dir() {
    if [ "$BENCH_PROFILE" = default ]; then echo "$HOME/.colima/_lima/colima"; else echo "$HOME/.colima/_lima/colima-$BENCH_PROFILE"; fi
}

rt_start() { colima start "$BENCH_PROFILE" >/dev/null 2>&1; }

rt_stop() { colima stop "$BENCH_PROFILE" >/dev/null 2>&1; }

rt_socket() { echo "unix://$HOME/.colima/$BENCH_PROFILE/docker.sock"; }

rt_version() { colima version 2>/dev/null | awk '/colima version/ {print $3}'; }

rt_pids() {
    local dir; dir="$(colima_dir)"
    # The lima host agent, plus the VM process that holds the disks.
    { cat "$dir/ha.pid" 2>/dev/null; echo
      pids_of_files "$dir/diffdisk" "$dir/basedisk" "$HOME/.colima/_lima/_disks/colima-$BENCH_PROFILE/datadisk"
    } | rg '^[0-9]+$' | sort -u
}
