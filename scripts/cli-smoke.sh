#!/usr/bin/env bash
# CLI smoke gate (milestone v1.5 CLI parity).
#
# Runs the colima-style CLI end to end on a throwaway CI profile: profile
# create/ls/rm, template, config, start --no-autostart --wait, status,
# version, -o json/yaml, a container through the profile's docker context,
# ssh and ssh-config, and delete. The default profile, the current profile and the active docker
# context must stay untouched.
#
# Usage: ./scripts/cli-smoke.sh
#   SPK=target/aarch64-apple-darwin/release/spk ./scripts/cli-smoke.sh
# The binary must be signed (cargo xtask sign release) to boot a VM.
set -u

SPK="${SPK:-spk}"
P="cli-smoke-$$"
CTX="speck-$P"
PASS=0; FAIL=0; FAILED=()

check() {
    local name="$1"; shift
    if "$@" >/dev/null 2>&1; then
        printf 'PASS  %s\n' "$name"; PASS=$((PASS+1))
    else
        printf 'FAIL  %s\n' "$name"; FAIL=$((FAIL+1)); FAILED+=("$name")
    fi
}

# Succeeds when the command fails: for inputs spk must reject.
fails() { ! "$@"; }

cleanup() { env -u SPECK_PROFILE "$SPK" profile rm "$P" --force >/dev/null 2>&1; }
trap cleanup EXIT

unset SPECK_PROFILE SPECK_HOME
context_before="$(docker context show 2>/dev/null)"
current_before="$("$SPK" profile current)"
home="$HOME/.speck/profiles/$P"

# Profiles and config, no VM yet
check "profile create"            "$SPK" profile create "$P" --cpus 1 --memory 1g --no-autostart
check "profile ls -o json"        sh -c "'$SPK' profile ls -o json | grep -q '\"profile\": \"$P\"'"
check "template (EDITOR=true)"    env EDITOR=true VISUAL= "$SPK" -p "$P" template
check "config -o json is valid"   sh -c "'$SPK' -p '$P' config -o json | grep -q '\"valid\": true'"
check "config keeps autostart"    grep -q 'autostart: false' "$home/config.yaml"
check "profile current -o yaml"   sh -c "SPECK_PROFILE='$P' '$SPK' profile current -o yaml | grep -q 'profile: $P'"
check "-o json rejected for ps"   fails "$SPK" -p "$P" ps -o json
check "invalid profile name"      fails "$SPK" -p "Bad_Name" status

# CI start: background daemon, no launchd agent, no context switch
check "start --edit --wait"       env EDITOR=true VISUAL= "$SPK" -p "$P" start --edit --wait --timeout 180
check "no launchd plist"          test ! -e "$HOME/Library/LaunchAgents/io.speck.vm.$P.plist"
check "status -o json running"    sh -c "'$SPK' -p '$P' status -o json | grep -q '\"running\": true'"
check "version -o yaml"           sh -c "'$SPK' -p '$P' version -o yaml | grep -q '^initrd:'"
check "doctor -o json document"   sh -c "'$SPK' -p '$P' doctor -o json | grep -q '\"checks\"'"
check "container via $CTX"        docker --context "$CTX" run --rm alpine:latest true
check "DOCKER_HOST to the socket" env DOCKER_HOST="unix://$home/speck.sock" docker ps
check "docker context unchanged"  test "$(docker context show 2>/dev/null)" = "$context_before"
check "start again is a no-op"    sh -c "'$SPK' -p '$P' start | grep -q 'already running'"

# SSH (rootfs 1.4.0+)
check "ssh -- true"               "$SPK" -p "$P" ssh -- true
check "ssh passes the exit code"  sh -c "'$SPK' -p '$P' ssh -- 'exit 3'; test \$? -eq 3"
check "ssh sees dockerd"          sh -c "'$SPK' -p '$P' ssh -- ps | grep -q dockerd"
"$SPK" -p "$P" ssh-config > "$home/ssh-config.test" 2>/dev/null
check "ssh-config host"           grep -q "^Host speck-$P\$" "$home/ssh-config.test"
check "ssh via ssh-config"        ssh -F "$home/ssh-config.test" "speck-$P" true
check "port 22 closed to containers" fails docker --context "$CTX" run --rm alpine:latest nc -z -w2 172.17.0.1 22

# Removal
check "profile rm"                env -u SPECK_PROFILE "$SPK" profile rm "$P" --force
check "profile dir removed"       test ! -e "$home"
check "docker context removed"    fails docker context inspect "$CTX"
check "current profile unchanged" test "$("$SPK" profile current)" = "$current_before"

echo
echo "$PASS passed, $FAIL failed"
if [ "$FAIL" -gt 0 ]; then
    printf '  failed: %s\n' "${FAILED[@]}"
    exit 1
fi
