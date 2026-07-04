#!/usr/bin/env bash
set -euo pipefail

ROOTFS_VERSION="${1:-0.1.0}"
BACKEND="${2:-moby}"
SPECK_HOME="${SPECK_HOME:-$HOME/.local/share/speck}"
ROOTFS_DEST="${SPECK_HOME}"  # CR-02: match spk up defaults
RELEASE_BASE="https://github.com/wasilak/speck/releases/download"
ROOTFS_IMG="${ROOTFS_DEST}/rootfs.img"
ROOTFS_SUM="${ROOTFS_DEST}/rootfs.img.sha256"
DATA_IMG="${ROOTFS_DEST}/data.img"

echo "==> Speck: fetching rootfs ${ROOTFS_VERSION} (${BACKEND}, arm64)"

mkdir -p "$ROOTFS_DEST"

# Step 1 — Download checksum for the REQUESTED version/backend (CR-01)
RELEASE_TAG="rootfs-${ROOTFS_VERSION}"
CHECKSUM_FILE="speck-rootfs-${ROOTFS_VERSION}-${BACKEND}-arm64.img.sha256"
echo "    Downloading checksum: ${RELEASE_BASE}/${RELEASE_TAG}/${CHECKSUM_FILE}"
curl -fsSL "${RELEASE_BASE}/${RELEASE_TAG}/${CHECKSUM_FILE}" -o "${ROOTFS_SUM}.requested"

# Step 2 — Compare existing image against the REQUESTED checksum
REQUESTED_SUM=$(awk '{print $1}' "${ROOTFS_SUM}.requested")
if [ -f "${ROOTFS_IMG}" ]; then
    ACTUAL=$(shasum -a 256 "${ROOTFS_IMG}" | awk '{print $1}')
    if [ "${REQUESTED_SUM}" = "${ACTUAL}" ]; then
        mv -f "${ROOTFS_SUM}.requested" "${ROOTFS_SUM}"
        echo "==> rootfs image matches ${ROOTFS_VERSION}/${BACKEND}, skipping"
        exit 0
    fi
    echo "==> existing rootfs does not match requested version/backend; re-downloading"
    rm -f "${ROOTFS_IMG}" "${ROOTFS_SUM}" "${ROOTFS_IMG}.tmp"
fi
mv -f "${ROOTFS_SUM}.requested" "${ROOTFS_SUM}"

# Step 3 — Download and decompress rootfs image
rm -f "${ROOTFS_IMG}.tmp"
IMAGE_FILE="speck-rootfs-${ROOTFS_VERSION}-${BACKEND}-arm64.img.gz"
echo "    Downloading image: ${RELEASE_BASE}/${RELEASE_TAG}/${IMAGE_FILE}"
curl -fsSL "${RELEASE_BASE}/${RELEASE_TAG}/${IMAGE_FILE}" | gunzip -c > "${ROOTFS_IMG}.tmp"
mv -f "${ROOTFS_IMG}.tmp" "${ROOTFS_IMG}"

# Step 4 — Verify SHA256 against the downloaded checksum
EXPECTED=$(awk '{print $1}' "${ROOTFS_SUM}")
ACTUAL=$(shasum -a 256 "${ROOTFS_IMG}" | awk '{print $1}')
if [ "${EXPECTED}" != "${ACTUAL}" ]; then
    echo "ERROR: SHA256 mismatch for rootfs.img"
    echo "  Expected: ${EXPECTED}"
    echo "  Actual:   ${ACTUAL}"
    rm -f "${ROOTFS_IMG}" "${ROOTFS_SUM}"
    exit 1
fi
echo "    SHA256 checksum verified"

# Step 5 — Create data disk stub atomically (WR-01)
if [ ! -f "${DATA_IMG}" ]; then
    echo "    Creating data disk stub: ${DATA_IMG}"
    rm -f "${DATA_IMG}.tmp"
    dd if=/dev/zero of="${DATA_IMG}.tmp" bs=1M count=512 status=progress
    mv -f "${DATA_IMG}.tmp" "${DATA_IMG}"
fi

# Step 6 — Done message
echo ""
echo "==> Done!"
echo "    Rootfs: ${ROOTFS_IMG} ($(du -h "${ROOTFS_IMG}" | cut -f1))"
echo "    Data disk stub: ${DATA_IMG} ($(du -h "${DATA_IMG}" | cut -f1))"
