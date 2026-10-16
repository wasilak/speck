#!/usr/bin/env bash
# Conformance smoke gate (decision D-21, CLAUDE.md "Architecture Invariants").
#
# Runs the SYSTEM docker CLI against a live speck daemon and exercises the
# everyday workflows a colima user expects. This script is the executable
# definition of Docker API conformance: a phase touching the API layer is
# not done while any step fails.
#
# Usage: ./scripts/conformance-smoke.sh   (daemon must be running: spk up)
set -u

export DOCKER_HOST="unix://${SPECK_HOME:-$HOME/.speck}/speck.sock"
PASS=0; FAIL=0; FAILED=()

check() {
    local name="$1"; shift
    if "$@" >/dev/null 2>&1; then
        printf 'PASS  %s\n' "$name"; PASS=$((PASS+1))
    else
        printf 'FAIL  %s\n' "$name"; FAIL=$((FAIL+1)); FAILED+=("$name")
    fi
}

# Cleanup from previous runs
docker rm -f smoke1 smoke2 >/dev/null 2>&1

check "ping"                      docker info
check "pull"                      docker pull alpine:latest
check "images list"               docker images
check "run detached, short name"  docker run -d --name smoke1 alpine sleep 30
check "ps shows container"        sh -c 'docker ps | grep -q smoke1'
check "ps -a"                     docker ps -a
check "inspect"                   docker inspect smoke1
check "exec"                      docker exec smoke1 echo ok
check "logs"                      docker logs smoke1
check "stop"                      docker stop smoke1
check "rm"                        docker rm smoke1
check "run foreground w/ output"  sh -c 'docker run --rm alpine echo hello | grep -q hello'
check "run image CMD fallback"    docker run -d --name smoke2 nginx:alpine
check "port publish"              sh -c 'docker rm -f smoke2 >/dev/null 2>&1; docker run -d --name smoke2 -p 18099:80 nginx:alpine && sleep 3 && curl -sf --max-time 5 http://localhost:18099/ >/dev/null'
check "port inspect HostIp"       sh -c 'docker inspect smoke2 | grep -q '"'"'"HostIp": "127.0.0.1"'"'"''
check "docker port output"        sh -c 'docker port smoke2 | grep -q "127.0.0.1:18099"'
check "udp port publish"          sh -c 'docker rm -f smokeudp >/dev/null 2>&1; docker run -d --name smokeudp -p 18098:9999/udp alpine sh -c "while true; do nc -u -l -p 9999 -e cat; done" && sleep 2 && python3 -c "
import socket; s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s.settimeout(5)
s.sendto(b\"speck-udp\", (\"127.0.0.1\", 18098)); assert s.recvfrom(64)[0] == b\"speck-udp\"
"; rc=$?; docker rm -f smokeudp >/dev/null; exit $rc'
# Ryuk, devcontainers and docker:cli mount the Docker socket; testcontainers
# mounts the DOCKER_HOST path.
check "docker socket mount"       docker run --rm -v /var/run/docker.sock:/var/run/docker.sock docker:cli docker version
check "DOCKER_HOST socket mount"  docker run --rm -v "${DOCKER_HOST#unix://}":/var/run/docker.sock docker:cli docker version
# The containerd image store lets the default builder do multi-platform.
check "multi-platform build"      sh -c 'printf "FROM alpine\nRUN uname -m\n" | docker buildx build --platform linux/amd64,linux/arm64 -o type=cacheonly -'
check "wait"                      sh -c 'docker run -d --name smokew alpine true && docker wait smokew && docker rm smokew'
check "events (2s window)"        sh -c 'timeout 2 docker events; [ $? -eq 124 -o $? -eq 0 ]'
check "volume create/ls/rm"       sh -c 'docker volume create smokevol && docker volume ls | grep -q smokevol && docker volume rm smokevol'
check "network ls"                docker network ls

# Build / push / pull round-trip against a local registry (de-risks ECR workflow)
#
# Architecture note: the Docker daemon runs INSIDE the guest VM, but
# Speck's port-publish middleware maps ports on the macOS host — there is
# no docker-proxy inside the VM.  That means:
#   • localhost:15000 has no listener inside the VM
#   • Docker's embedded DNS (127.0.0.11) only serves *containers*, not the daemon
#   • Container IPs on a bridge network ARE reachable from the daemon
# So we capture the registry's IP and push/pull via IP.
check "build local registry"      sh -c '
    docker rm -f smokereg >/dev/null 2>&1
    docker network create smoke-net >/dev/null 2>&1 || true
    docker run -d --network smoke-net --name smokereg registry:2
    sleep 3
'
REGISTRY_IP=$(docker inspect -f "{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}" smokereg)
if [ -z "$REGISTRY_IP" ]; then
    echo "SKIP (registry IP not resolved)" && return 2>/dev/null || exit 77
fi
REGISTRY="$REGISTRY_IP:5000"

check "build image"               sh -c '
    docker build -t '"$REGISTRY"'/smoke-test:latest -f- . <<EOF
FROM alpine
RUN echo "built-by-speck" > /build-marker
EOF
'
check "push to registry"          docker push "$REGISTRY"/smoke-test:latest
check "rmi local"                 docker rmi "$REGISTRY"/smoke-test:latest
check "pull from registry"        docker pull "$REGISTRY"/smoke-test:latest
check "verify pulled image"       sh -c 'docker run --rm '"$REGISTRY"'/smoke-test:latest cat /build-marker | grep -q built-by-speck'

docker rm -f smoke1 smoke2 smokew smokereg >/dev/null 2>&1

echo
echo "── conformance smoke: ${PASS} passed, ${FAIL} failed"
if [ "$FAIL" -gt 0 ]; then
    printf '   failing: %s\n' "${FAILED[@]}"
    exit 1
fi
