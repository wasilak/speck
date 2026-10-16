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
#   EGRESS_SSH_HOST      ssh endpoint on :22              (default github.com)
#   EGRESS_NTP_HOST      NTP server for the UDP check     (default time.cloudflare.com)
#   EGRESS_INTERNAL_HOST internal split-DNS https host    (check skipped if unset)
#   EGRESS_INTERNAL_TCP  internal host:port, e.g. db:5432 (check skipped if unset)
#   EGRESS_DOCKER_HOST   compare another runtime, e.g. unix://$HOME/.colima/default/docker.sock
set -u

export DOCKER_HOST="${EGRESS_DOCKER_HOST:-unix://${SPECK_HOME:-$HOME/.speck}/speck.sock}"
SSH_HOST="${EGRESS_SSH_HOST:-github.com}"
NTP_HOST="${EGRESS_NTP_HOST:-time.cloudflare.com}"
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

# TCP re-origination (EGRESS-01), real internet endpoints
check "tcp 443"                   in_c "nc -z -w5 github.com 443"
check "tcp 80"                    in_c "nc -z -w5 github.com 80"
check "tcp 22 banner ($SSH_HOST)" in_c "(sleep 2) | nc -w5 $SSH_HOST 22 | grep -q SSH"

# TCP to non-web ports on stable public services (any-port test services
# rate-limit repeated runs; a local server is blocked by the macOS app
# firewall). Banner checks prove data flows back, not just the handshake.
check "tcp 587 banner (smtp)"     in_c "(sleep 2) | nc -w5 smtp.gmail.com 587 | grep -q 220"
check "tcp 6667 banner (irc)"     in_c "(sleep 3) | nc -w5 irc.libera.chat 6667 | grep -qi notice"
check "tcp 993 (imaps)"           in_c "nc -z -w5 imap.gmail.com 993"
check "tcp 853 (dns-over-tls)"    in_c "nc -z -w5 1.1.1.1 853"
check "tcp 9418 (git)"            in_c "nc -z -w5 git.kernel.org 9418"

HOST_IP="$(ipconfig getifaddr en0 2>/dev/null || ipconfig getifaddr en1 2>/dev/null)"
if [ -n "$HOST_IP" ]; then
    # A closed port must look closed from the container (EGRESS-03): the
    # netstack may only accept a guest SYN after the host connect succeeded.
    check "tcp closed port refused"   in_c "s=\$(date +%s); ! nc -z -w8 $HOST_IP 1 && [ \$((\$(date +%s)-s)) -lt 4 ]"
else
    printf 'INFO  closed-port check skipped (no en0/en1 address)\n'
fi

# UDP re-origination (EGRESS-04): busybox ntpd one-shot query
check "udp 123 (ntp $NTP_HOST)"   in_c "timeout 10 ntpd -n -q -p $NTP_HOST"

# DNS record types (RES-04)
check "dns A"                     in_c "nslookup -type=A github.com | grep -q 'Address'"
check "dns TXT"                   in_c "nslookup -type=TXT example.com | grep -q 'text ='"
check "dns SRV"                   in_c "nslookup -type=SRV _imaps._tcp.gmail.com | grep -q 'service ='"
check "dns MX"                    in_c "nslookup -type=MX gmail.com | grep -q 'mail exchanger'"
# google.com TXT is ~1.2 KB; compare the record count with the host.
TXT_EXPECTED="$(dig +short txt google.com 2>/dev/null | wc -l | tr -d ' ')"
# DNS on a user-defined network (docker compose): containers query Docker's
# embedded DNS, which forwards to the guest forwarder from another bridge.
check "dns on user network"       sh -c 'docker network create egress-net >/dev/null 2>&1; docker run --rm --network egress-net alpine nslookup github.com | grep -q "^Address"'
docker network rm egress-net >/dev/null 2>&1

# Answers above 512 bytes: an EDNS client gets them over UDP, a plain UDP
# client gets a truncated answer and retries over TCP:53 (RES-05). busybox
# nslookup does not retry over TCP on any Docker, so dig is used here.
check "dns large answer (EDNS)"   in_c "apk add -q --no-cache bind-tools >/dev/null && [ \$(dig +bufsize=4096 +short txt google.com | wc -l) -ge ${TXT_EXPECTED:-1} ]"
check "dns large answer (TCP retry)" in_c "[ \$(dig +noedns +short txt google.com | wc -l) -ge ${TXT_EXPECTED:-1} ]"

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
# fakes echo replies locally (EGRESS-06; it did until smoltcp's
# auto-icmp-echo-reply was disabled).
check "icmp not faked"            in_c "! ping -c1 -W3 192.0.2.1"
# Real pings are forwarded from the host (EGRESS-07). 8000 bytes is fragmented
# in both directions, which needs smoltcp reassembly/fragmentation >1500 B.
check "icmp ping 1.1.1.1"         in_c "ping -c2 -W3 1.1.1.1"
check "icmp ping fragmented (8000B)" in_c "ping -c2 -W3 -s 8000 1.1.1.1"
check "icmp ping gateway"         in_c "ping -c1 -W2 172.16.0.1"

# Services on the Mac (Docker Desktop/Colima parity): host.docker.internal
# and --add-host x:host-gateway reach the Mac's loopback.
HOST_PORT=$((20000 + RANDOM % 10000))
python3 -m http.server "$HOST_PORT" --bind 127.0.0.1 --directory /tmp >/dev/null 2>&1 &
HTTP_PID=$!
sleep 1
check "host.docker.internal"      in_c "wget -q -T5 -O /dev/null http://host.docker.internal:$HOST_PORT/"
check "host-gateway"              docker run --rm --add-host mac:host-gateway alpine wget -q -T5 -O /dev/null "http://mac:$HOST_PORT/"
kill "$HTTP_PID" 2>/dev/null

docker rm -f "$C" >/dev/null 2>&1

echo
echo "── egress smoke: $PASS passed, $FAIL failed"
if [ "$FAIL" -gt 0 ]; then
    printf '   failed: %s\n' "${FAILED[@]}"
    exit 1
fi
