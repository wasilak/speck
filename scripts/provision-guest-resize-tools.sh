#!/usr/bin/env bash
set -euo pipefail

SPECK_HOME="${SPECK_HOME:-$HOME/.local/share/speck}"
INITRD="${SPECK_HOME}/initrd/kata-alpine-3.22.initrd"
BACKUP="${INITRD}.pre-resize-tools.bak"
REQUIRED_TOOLS=("/sbin/resize2fs" "/sbin/mke2fs" "/sbin/blkid")

echo "==> Speck: provisioning guest resize tools in ${INITRD}"

if [ ! -f "${INITRD}" ]; then
    echo "ERROR: initrd not found: ${INITRD}; cannot provision /sbin/resize2fs"
    exit 1
fi

if ! command -v docker >/dev/null 2>&1; then
    echo "ERROR: docker is required to provision guest /sbin/resize2fs"
    exit 1
fi
RUNTIME="docker"

mkdir -p "${SPECK_HOME}/tmp"
TMPDIR="$(mktemp -d "${SPECK_HOME}/tmp/speck-resize-tools-provision.XXXXXX")"
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

compression_kind() {
    local archive="$1"
    case "$(LC_ALL=C od -An -tx1 -N4 "${archive}" | tr -d ' \n')" in
        1f8b*) echo "gzip" ;;
        28b52ffd*) echo "zstd" ;;
        *) echo "plain" ;;
    esac
}

pack_initrd_to_file() {
    local kind="$1"
    local output="$2"
    local pack_cmd
    case "${kind}" in
        gzip) pack_cmd='find . -print | cpio -o -H newc --quiet | gzip -9 > /out/repacked.initrd' ;;
        zstd) pack_cmd='find . -print | cpio -o -H newc --quiet | zstd -19 -q > /out/repacked.initrd' ;;
        plain) pack_cmd='find . -print | cpio -o -H newc --quiet > /out/repacked.initrd' ;;
        *) echo "ERROR: unsupported initrd compression kind: ${kind}" >&2; return 1 ;;
    esac
    "${RUNTIME}" run --rm --platform linux/arm64 \
        -v "${TMPDIR}/root:/work" \
        -v "${TMPDIR}:/out" \
        alpine:3.22 \
        sh -eu -c "apk add --no-cache cpio gzip zstd >/dev/null; cd /work; ${pack_cmd}"
    mv -f "${TMPDIR}/repacked.initrd" "${output}"
}

mkdir -p "${TMPDIR}/root" "${TMPDIR}/overlay"
(
    cd "${TMPDIR}/root"
    unpack_initrd "${INITRD}" | cpio -idm --quiet 2>"${TMPDIR}/cpio.err" || true
)

missing=()
for tool in "${REQUIRED_TOOLS[@]}"; do
    if [ ! -e "${TMPDIR}/root${tool}" ]; then
        missing+=("${tool}")
    fi
done

if [ "${#missing[@]}" -eq 0 ]; then
    echo "==> Required tools already present; re-running inspection"
    bash "$(dirname "$0")/check-guest-resize-tools.sh"
    exit 0
fi

echo "    Missing tools: ${missing[*]}"
echo "    Using ${RUNTIME} with alpine:3.22 arm64 to source e2fsprogs/util-linux artifacts"

"${RUNTIME}" run --rm --platform linux/arm64 \
    -v "${TMPDIR}/overlay:/overlay" \
    alpine:3.22 \
    sh -eu -c '
        apk add --no-cache e2fsprogs e2fsprogs-extra util-linux >/dev/null
        mkdir -p /overlay/sbin /overlay/lib /overlay/usr/lib
        for tool in resize2fs mke2fs blkid; do
            src="$(command -v "$tool" || true)"
            if [ -z "$src" ] || [ ! -x "$src" ]; then
                echo "ERROR: provisioning source missing /sbin/$tool" >&2
                exit 1
            fi
            cp -L "$src" "/overlay/sbin/$tool"
        done
        for lib in /lib/ld-musl-*.so.1 /lib/lib*.so* /usr/lib/lib*.so*; do
            [ -e "$lib" ] || continue
            mkdir -p "/overlay$(dirname "$lib")"
            cp -L "$lib" "/overlay$lib"
        done
    '

for tool in "${REQUIRED_TOOLS[@]}"; do
    if [ ! -e "${TMPDIR}/overlay${tool}" ]; then
        echo "ERROR: provisioning source did not provide ${tool}; cannot provision /sbin/resize2fs"
        exit 1
    fi
    rm -f "${TMPDIR}/root${tool}"
done

cp -Rp "${TMPDIR}/overlay/." "${TMPDIR}/root/"

if [ ! -f "${BACKUP}" ]; then
    cp -p "${INITRD}" "${BACKUP}"
    echo "    Backup: kata-alpine-3.22.initrd.pre-resize-tools.bak"
else
    echo "    Backup already exists: kata-alpine-3.22.initrd.pre-resize-tools.bak"
fi

KIND="$(compression_kind "${INITRD}")"
TMP_INITRD="${INITRD}.resize-tools.tmp"
pack_initrd_to_file "${KIND}" "${TMP_INITRD}"
mv -f "${TMP_INITRD}" "${INITRD}"

echo "==> Re-running guest resize tool inspection"
bash "$(dirname "$0")/check-guest-resize-tools.sh"
