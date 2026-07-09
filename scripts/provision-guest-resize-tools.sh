#!/usr/bin/env bash
set -euo pipefail

SPECK_HOME="${SPECK_HOME:-$HOME/.speck}"
INITRD="${SPECK_HOME}/initrd/initrd.cpio.gz"
BACKUP="${INITRD}.pre-resize-tools.bak"
REQUIRED_TOOLS=("/sbin/resize2fs" "/sbin/mke2fs" "/sbin/blkid")

echo "==> Speck: provisioning guest resize tools in ${INITRD}"

if [ ! -f "${INITRD}" ]; then
    echo "ERROR: initrd not found: ${INITRD}; cannot provision /sbin/resize2fs"
    exit 1
fi

ALPINE_IMAGE="docker://docker.io/library/alpine:3.22"
ALPINE_REPO="https://dl-cdn.alpinelinux.org/alpine/v3.22/main/aarch64"
REQUIRED_APKS=(blkid e2fsprogs e2fsprogs-extra e2fsprogs-libs libblkid libcom_err libuuid)

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
        gzip) pack_cmd='find . -print | cpio -o -H newc --quiet | gzip -9 > "$1"' ;;
        zstd) pack_cmd='find . -print | cpio -o -H newc --quiet | zstd -19 -q > "$1"' ;;
        plain) pack_cmd='find . -print | cpio -o -H newc --quiet > "$1"' ;;
        *) echo "ERROR: unsupported initrd compression kind: ${kind}" >&2; return 1 ;;
    esac
    (
        cd "${TMPDIR}/root"
        sh -eu -c "${pack_cmd}" sh "${output}"
    )
}

apk_version() {
    local pkg="$1"
    python3 - <<'PY' "${TMPDIR}" "$pkg"
from pathlib import Path
import sys

tmpdir = Path(sys.argv[1])
pkg = sys.argv[2]
text = (tmpdir / "apk-index" / "APKINDEX").read_text()
for block in text.split("\n\n"):
    name = version = None
    for line in block.splitlines():
        if line.startswith("P:"):
            name = line[2:]
        elif line.startswith("V:"):
            version = line[2:]
    if name == pkg and version:
        print(version)
        raise SystemExit(0)
raise SystemExit(f"package {pkg} not found in APKINDEX")
PY
}

populate_overlay_from_apks() {
    local oci_dir="${TMPDIR}/alpine-oci"
    local layer_digest

    mkdir -p "${TMPDIR}/apk-index"
    curl -fsSL "${ALPINE_REPO}/APKINDEX.tar.gz" -o "${TMPDIR}/APKINDEX.tar.gz"
    tar -xzf "${TMPDIR}/APKINDEX.tar.gz" -C "${TMPDIR}/apk-index"

    skopeo copy --insecure-policy --override-os linux --override-arch arm64 \
        "${ALPINE_IMAGE}" "oci:${oci_dir}:latest" >/dev/null

    layer_digest="$(python3 - <<'PY' "${oci_dir}"
from pathlib import Path
import json
import sys

oci = Path(sys.argv[1])
index = json.loads((oci / "index.json").read_text())
manifest_digest = index["manifests"][0]["digest"].split(":", 1)[1]
manifest = json.loads((oci / "blobs" / "sha256" / manifest_digest).read_text())
print(manifest["layers"][0]["digest"].split(":", 1)[1])
PY
)"
    tar -xzf "${oci_dir}/blobs/sha256/${layer_digest}" -C "${TMPDIR}/overlay"

    for pkg in "${REQUIRED_APKS[@]}"; do
        local version
        version="$(apk_version "$pkg")"
        curl -fsSL "${ALPINE_REPO}/${pkg}-${version}.apk" -o "${TMPDIR}/${pkg}.apk"
        tar -xzf "${TMPDIR}/${pkg}.apk" -C "${TMPDIR}/overlay"
    done

    mkdir -p "${TMPDIR}/overlay/sbin"
    for tool in resize2fs mke2fs blkid; do
        local src=""
        for candidate in \
            "${TMPDIR}/overlay/sbin/${tool}" \
            "${TMPDIR}/overlay/usr/sbin/${tool}" \
            "${TMPDIR}/overlay/bin/${tool}" \
            "${TMPDIR}/overlay/usr/bin/${tool}"; do
            if [ -e "${candidate}" ]; then
                src="${candidate}"
                break
            fi
        done
        if [ -z "${src}" ]; then
            echo "ERROR: provisioning source missing /sbin/${tool}" >&2
            exit 1
        fi
        if [ "${src}" != "${TMPDIR}/overlay/sbin/${tool}" ]; then
            cp -L "${src}" "${TMPDIR}/overlay/sbin/${tool}"
        fi
    done
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
if command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
    echo "    Using docker with alpine:3.22 arm64 to source e2fsprogs/util-linux artifacts"
    docker run --rm --platform linux/arm64 \
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
else
    if ! command -v skopeo >/dev/null 2>&1; then
        echo "ERROR: docker is unavailable and skopeo is not installed; cannot provision guest /sbin/resize2fs" >&2
        exit 1
    fi
    echo "    Docker daemon unavailable; using skopeo + Alpine APKs to source arm64 Linux tooling"
    populate_overlay_from_apks
fi

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
    echo "    Backup: ${BACKUP}"
else
    echo "    Backup already exists: ${BACKUP}"
fi

KIND="$(compression_kind "${INITRD}")"
TMP_INITRD="${INITRD}.resize-tools.tmp"
pack_initrd_to_file "${KIND}" "${TMP_INITRD}"
mv -f "${TMP_INITRD}" "${INITRD}"

echo "==> Re-running guest resize tool inspection"
bash "$(dirname "$0")/check-guest-resize-tools.sh"
