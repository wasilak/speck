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
check "wait"                      sh -c 'docker run -d --name smokew alpine true && docker wait smokew && docker rm smokew'
check "events (2s window)"        sh -c 'timeout 2 docker events; [ $? -eq 124 -o $? -eq 0 ]'
check "volume create/ls/rm"       sh -c 'docker volume create smokevol && docker volume ls | grep -q smokevol && docker volume rm smokevol'
check "network ls"                docker network ls

# Build / push / pull round-trip against a local registry (de-risks ECR workflow)
check "build local registry"      sh -c '
    docker rm -f smokereg >/dev/null 2>&1
    docker run -d -p 15000:5000 --name smokereg registry:2
    sleep 3
'
check "build image"               sh -c '
    docker build -t localhost:15000/smoke-test:latest -f- . <<EOF
FROM alpine
RUN echo "built-by-speck" > /build-marker
EOF
'
check "push to registry"          docker push localhost:15000/smoke-test:latest
check "rmi local"                 docker rmi localhost:15000/smoke-test:latest
check "pull from registry"        docker pull localhost:15000/smoke-test:latest
check "verify pulled image"       sh -c 'docker run --rm localhost:15000/smoke-test:latest cat /build-marker | grep -q built-by-speck'

docker rm -f smoke1 smoke2 smokew smokereg >/dev/null 2>&1

echo
echo "── conformance smoke: ${PASS} passed, ${FAIL} failed"
if [ "$FAIL" -gt 0 ]; then
    printf '   failing: %s\n' "${FAILED[@]}"
    exit 1
fi
