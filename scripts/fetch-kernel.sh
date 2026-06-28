#!/usr/bin/env bash
set -euo pipefail

KATA_VERSION="${1:-3.32.0}"
SPECK_HOME="${SPECK_HOME:-$HOME/.local/share/speck}"
KERNEL_DEST="${2:-$SPECK_HOME/kernel}"
INITRD_DEST="${3:-$SPECK_HOME/initrd}"
KATA_BASE="https://github.com/kata-containers/kata-containers/releases/download"

echo "==> Speck: fetching Kata Containers ${KATA_VERSION} kernel + initrd (arm64)"

mkdir -p "$KERNEL_DEST" "$INITRD_DEST"

if ! command -v zstdcat &>/dev/null; then
    echo "ERROR: zstd not found. Install it: brew install zstd"
    exit 1
fi

KATA_TARBALL="${KATA_BASE}/${KATA_VERSION}/kata-static-${KATA_VERSION}-arm64.tar.zst"
echo "    Downloading: ${KATA_TARBALL}"

if ! curl -sL "$KATA_TARBALL" | zstdcat -c | tar -C "$KERNEL_DEST" --strip-components=5 -xf - \
    ./opt/kata/share/kata-containers/vmlinux-6.18.35-197 \
    ./opt/kata/share/kata-containers/kata-alpine-3.22.initrd
then
    echo "ERROR: download or extraction failed"
    exit 1
fi

# Move initrd to its destination
if [ -f "${KERNEL_DEST}/kata-alpine-3.22.initrd" ]; then
    mv "${KERNEL_DEST}/kata-alpine-3.22.initrd" "${INITRD_DEST}/"
fi

# Create a stable symlink for the default kernel path
ln -sf "vmlinux-6.18.35-197" "${KERNEL_DEST}/vmlinux"

echo ""
echo "==> Done!"
echo "    Kernel: ${KERNEL_DEST}/vmlinux-6.18.35-197 ($(du -h "${KERNEL_DEST}/vmlinux-6.18.35-197" | cut -f1))"
echo "    Initrd: ${INITRD_DEST}/kata-alpine-3.22.initrd ($(du -h "${INITRD_DEST}/kata-alpine-3.22.initrd" | cut -f1))"
echo "    Stable symlink: ${KERNEL_DEST}/vmlinux -> vmlinux-6.18.35-197"
