#!/usr/bin/env bash
# Egress smoke gate (milestone v1.4 Daily Driver, Core Value).
#
# Runs checks from INSIDE a container on a live speck daemon to prove that
# guest traffic is re-originated from the host network stack: any TCP port,
# UDP, DNS record types and TLS. Baseline 2026-10-11: only TCP 80/443 pass.
#
# Usage: ./scripts/egress-smoke.sh   (daemon must be running: spk up)
#
# Optional overrides (for the corporate Mac):
#   EGRESS_TCP_HOST      host that accepts any TCP port   (default portquiz.net)
#   EGRESS_SSH_HOST      ssh endpoint on :22              (default github.com)
#   EGRESS_NTP_HOST      NTP server for the UDP check     (default pool.ntp.org)
#   EGRESS_INTERNAL_HOST internal split-DNS https host    (check skipped if unset)
#   EGRESS_INTERNAL_TCP  internal host:port, e.g. db:5432 (check skipped if unset)
#   EGRESS_DOCKER_HOST   compare another runtime, e.g. unix://$HOME/.colima/default/docker.sock
set -u

export DOCKER_HOST="${EGRESS_DOCKER_HOST:-unix://${SPECK_HOME:-$HOME/.speck}/speck.sock}"
TCP_HOST="${EGRESS_TCP_HOST:-portquiz.net}"
SSH_HOST="${EGRESS_SSH_HOST:-github.com}"
NTP_HOST="${EGRESS_NTP_HOST:-pool.ntp.org}"
C=egress-smoke
PASS=0; FAIL=0; FAILED=()

check() {
    local name="$1"; shift
    if "$@" >/dev/null 2>&1; then
        printf 'PASS  %s\n' "$name"; PASS=$((PASS+1))
    else
        printf 'FAIL  %s\n' "$name"; FAIL=$((FAIL+1)); FAILED+=("$name")
    fi
}

# Best-effort probes: reported, never counted as failures.
info() {
    local name="$1"; shift
    if "$@" >/dev/null 2>&1; then printf 'INFO  %s: ok\n' "$name"; else printf 'INFO  %s: no\n' "$name"; fi
}

in_c() { docker exec "$C" sh -c "$1"; }

docker rm -f "$C" >/dev/null 2>&1
if ! docker run -d --name "$C" alpine:latest sleep 600 >/dev/null; then
    echo "cannot start probe container on $DOCKER_HOST (is the daemon running?)"; exit 1
fi

# TCP re-origination (EGRESS-01)
check "tcp 443"                   in_c "nc -z -w5 $TCP_HOST 443"
check "tcp 80"                    in_c "nc -z -w5 $TCP_HOST 80"
check "tcp 22 (ssh $SSH_HOST)"    in_c "nc -z -w5 $SSH_HOST 22"
check "tcp 5432"                  in_c "nc -z -w5 $TCP_HOST 5432"
check "tcp 9200"                  in_c "nc -z -w5 $TCP_HOST 9200"
check "tcp 8443"                  in_c "nc -z -w5 $TCP_HOST 8443"
check "tcp 27017 (arbitrary)"     in_c "nc -z -w5 $TCP_HOST 27017"

# UDP re-origination (EGRESS-04): busybox ntpd one-shot query
check "udp 123 (ntp $NTP_HOST)"   in_c "timeout 10 ntpd -n -q -p $NTP_HOST"

# DNS record types (RES-04)
check "dns A"                     in_c "nslookup -type=A github.com | grep -q 'Address'"
check "dns TXT"                   in_c "nslookup -type=TXT google.com | grep -q 'text ='"
check "dns SRV"                   in_c "nslookup -type=SRV _xmpp-server._tcp.gmail.com | grep -q 'service ='"
check "dns MX"                    in_c "nslookup -type=MX gmail.com | grep -q 'mail exchanger'"

# TLS through whatever sits on the host path (TLS-01..03)
check "https wget"                in_c "wget -q -T 10 -O /dev/null https://github.com"
check "apk add over TLS"          in_c "apk add --no-cache -q curl"

# Corporate-only checks
if [ -n "${EGRESS_INTERNAL_HOST:-}" ]; then
    check "internal dns ($EGRESS_INTERNAL_HOST)"   in_c "nslookup $EGRESS_INTERNAL_HOST | grep -q 'Address'"
    check "internal https ($EGRESS_INTERNAL_HOST)" in_c "wget -q -T 10 -O /dev/null https://$EGRESS_INTERNAL_HOST"
fi
if [ -n "${EGRESS_INTERNAL_TCP:-}" ]; then
    check "internal tcp ($EGRESS_INTERNAL_TCP)"    in_c "nc -z -w5 ${EGRESS_INTERNAL_TCP%:*} ${EGRESS_INTERNAL_TCP##*:}"
fi

# ICMP: 192.0.2.1 (TEST-NET-1) must NOT answer. An answer means the netstack
# fakes echo replies locally (baseline 2026-10-11: it does, ~0.2 ms RTT).
info  "icmp not faked"            in_c "! ping -c1 -W3 192.0.2.1"

docker rm -f "$C" >/dev/null 2>&1

echo
echo "── egress smoke: $PASS passed, $FAIL failed"
if [ "$FAIL" -gt 0 ]; then
    printf '   failed: %s\n' "${FAILED[@]}"
    exit 1
fi
