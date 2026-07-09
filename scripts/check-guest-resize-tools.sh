#!/usr/bin/env bash
set -euo pipefail

SPECK_HOME="${SPECK_HOME:-$HOME/.speck}"
INITRD="${SPECK_HOME}/initrd/initrd.cpio.gz"
REQUIRED_TOOLS=("/sbin/resize2fs" "/sbin/mke2fs" "/sbin/blkid")

echo "==> Speck: checking guest resize tools in ${INITRD}"

if [ ! -f "${INITRD}" ]; then
    echo "ERROR: initrd not found: ${INITRD} (/sbin/resize2fs cannot be verified)"
    exit 1
fi

TMPDIR="$(mktemp -d "${TMPDIR:-/tmp}/speck-resize-tools-check.XXXXXX")"
cleanup() {
    rm -rf "${TMPDIR}"
}
trap cleanup EXIT

unpack_initrd() {
    local archive="$1"
    case "$(LC_ALL=C od -An -tx1 -N4 "${archive}" | tr -d ' \n')" in
        1f8b*) gzip -dc "${archive}" ;;
        28b52ffd*) zstdcat -c "${archive}" ;;
        *) cat "${archive}" ;;
    esac
}

mkdir -p "${TMPDIR}/root"
(
    cd "${TMPDIR}/root"
    # macOS cpio cannot create Linux device nodes as an unprivileged user.
    # Those entries are irrelevant for this inspection gate, so continue and
    # verify the exact tool paths below.
    unpack_initrd "${INITRD}" | cpio -idm --quiet 2>"${TMPDIR}/cpio.err" || true
)

missing=()
for tool in "${REQUIRED_TOOLS[@]}"; do
    if [ -e "${TMPDIR}/root${tool}" ]; then
        echo "    found ${tool}"
    else
        echo "ERROR: missing guest resize tool ${tool}"
        missing+=("${tool}")
    fi
done

if [ "${#missing[@]}" -ne 0 ]; then
    echo "ERROR: guest initrd is missing required resize tooling; run scripts/provision-guest-resize-tools.sh and re-run this check. Required: /sbin/resize2fs /sbin/mke2fs /sbin/blkid"
    exit 1
fi

echo "==> Guest resize tools are present"
