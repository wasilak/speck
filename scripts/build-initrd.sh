#!/usr/bin/env bash
# Build the speck guest initrd: an Alpine base with the disk tools vminitd
# runs before switch_root (/sbin/mke2fs and /sbin/blkid to format a blank
# data disk, /sbin/resize2fs to grow it), plus vminitd as /init.
#
# Used by .github/workflows/build-initrd.yml and for local verification, so
# the released initrd and the one tested locally are built the same way.
#
# Usage: scripts/build-initrd.sh <vminitd binary> <output .cpio.gz>
# Needs a Docker engine that can run linux/arm64 images (spk up works).
set -euo pipefail

if [ "$#" -ne 2 ]; then
    echo "usage: $0 <vminitd binary> <output .cpio.gz>" >&2
    exit 2
fi
VMINITD="$1"
OUT="$2"
ALPINE_IMAGE="${ALPINE_IMAGE:-alpine:3.22}"
REQUIRED_TOOLS=(mke2fs resize2fs blkid)

WORK="$(mktemp -d)"
cid=""
cleanup() {
    [ -n "${cid}" ] && docker rm -f "${cid}" >/dev/null 2>&1 || true
    rm -rf "${WORK}"
}
trap cleanup EXIT

echo "==> Installing disk tools into ${ALPINE_IMAGE}"
# util-linux's blkid, not busybox's: vminitd relies on its exit codes to
# prove a disk is blank before formatting it.
cid="$(docker create --platform linux/arm64 "${ALPINE_IMAGE}" \
    sh -euc 'apk add --no-cache e2fsprogs e2fsprogs-extra blkid >/dev/null')"
docker start -a "${cid}"
docker export "${cid}" > "${WORK}/rootfs.tar"

mkdir -p "${WORK}/root"
# Device nodes cannot be created without root; the kernel provides /dev.
tar -xf "${WORK}/rootfs.tar" -C "${WORK}/root" --exclude 'dev/*'

for tool in "${REQUIRED_TOOLS[@]}"; do
    if [ ! -e "${WORK}/root/sbin/${tool}" ]; then
        if [ -e "${WORK}/root/usr/sbin/${tool}" ]; then
            cp -L "${WORK}/root/usr/sbin/${tool}" "${WORK}/root/sbin/${tool}"
        else
            echo "ERROR: ${tool} missing from ${ALPINE_IMAGE} after apk add" >&2
            exit 1
        fi
    fi
done

mkdir -p "${WORK}/root"/{proc,sys,dev,rootfs,var/lib/containerd,run/speck}
cp "${VMINITD}" "${WORK}/root/init"
chmod 0755 "${WORK}/root/init"

echo "==> Packing ${OUT}"
(
    cd "${WORK}/root"
    find . -print | cpio -o -H newc -R 0:0 --quiet | gzip -9
) > "${OUT}"

echo "==> initrd: $(du -h "${OUT}" | cut -f1), $(gzip -dc "${OUT}" | cpio -t --quiet 2>/dev/null | wc -l | tr -d ' ') entries"
for tool in /init "${REQUIRED_TOOLS[@]/#//sbin/}"; do
    gzip -dc "${OUT}" | cpio -t --quiet 2>/dev/null | grep -qx ".${tool}" \
        || { echo "ERROR: ${tool} missing from ${OUT}" >&2; exit 1; }
done
echo "==> OK: /init and /sbin/{mke2fs,resize2fs,blkid} present"
