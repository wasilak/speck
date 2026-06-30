#!/usr/bin/env bash
set -euo pipefail

ROOTFS_VERSION="${1:-0.1.0}"
BACKEND="${2:-podman}"
SPECK_HOME="${SPECK_HOME:-$HOME/.local/share/speck}"
ROOTFS_DEST="${SPECK_HOME}/rootfs"
RELEASE_BASE="https://github.com/speck-vm/speck/releases/download"
ROOTFS_IMG="${ROOTFS_DEST}/rootfs.img"
ROOTFS_SUM="${ROOTFS_DEST}/rootfs.img.sha256"
DATA_IMG="${ROOTFS_DEST}/data.img"

# Step 1 — Skip if already present (idempotent)
if [ -f "${ROOTFS_IMG}" ]; then
    echo "==> rootfs image already present, skipping"
    exit 0
fi

echo "==> Speck: fetching rootfs ${ROOTFS_VERSION} (${BACKEND}, arm64)"

# Step 2 — Create directory
mkdir -p "$ROOTFS_DEST"

# Step 3 — Download SHA256 checksum file first
RELEASE_TAG="rootfs-${ROOTFS_VERSION}"
CHECKSUM_FILE="speck-rootfs-${ROOTFS_VERSION}-${BACKEND}-arm64.img.sha256"
echo "    Downloading checksum: ${RELEASE_BASE}/${RELEASE_TAG}/${CHECKSUM_FILE}"
curl -fsSL "${RELEASE_BASE}/${RELEASE_TAG}/${CHECKSUM_FILE}" -o "${ROOTFS_SUM}"

# Step 4 — Download and decompress rootfs image
IMAGE_FILE="speck-rootfs-${ROOTFS_VERSION}-${BACKEND}-arm64.img.gz"
echo "    Downloading image: ${RELEASE_BASE}/${RELEASE_TAG}/${IMAGE_FILE}"
curl -fsSL "${RELEASE_BASE}/${RELEASE_TAG}/${IMAGE_FILE}" | gunzip -c > "${ROOTFS_IMG}"

# Step 5 — Verify SHA256
EXPECTED=$(cat "${ROOTFS_SUM}" | awk '{print $1}')
ACTUAL=$(shasum -a 256 "${ROOTFS_IMG}" | awk '{print $1}')
if [ "${EXPECTED}" != "${ACTUAL}" ]; then
    echo "ERROR: SHA256 mismatch for rootfs.img"
    echo "  Expected: ${EXPECTED}"
    echo "  Actual:   ${ACTUAL}"
    rm -f "${ROOTFS_IMG}" "${ROOTFS_SUM}"
    exit 1
fi
echo "    SHA256 checksum verified"

# Step 6 — Create data disk stub (zero-filled 512 MB raw file) if not present
if [ ! -f "${DATA_IMG}" ]; then
    echo "    Creating data disk stub: ${DATA_IMG}"
    dd if=/dev/zero of="${DATA_IMG}" bs=1M count=512 status=progress 2>&1 || true
fi

# Step 7 — Done message
echo ""
echo "==> Done!"
echo "    Rootfs: ${ROOTFS_IMG} ($(du -h "${ROOTFS_IMG}" | cut -f1))"
echo "    Data disk stub: ${DATA_IMG} ($(du -h "${DATA_IMG}" | cut -f1))"
