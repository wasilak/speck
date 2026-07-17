#!/usr/bin/env bash
# Dev iteration helper: cross-compile vminitd and repack it into the live
# initrd at $SPECK_HOME/initrd/initrd.cpio.gz, without touching the rest of
# the initrd contents.
#
# Usage: scripts/dev-repack-vminitd.sh [--release]
#
# Per D-01 (25-CONTEXT.md): this is a greenfield project, no rollback/backup
# safety net is required — breaking/rebuilding the VM is acceptable.
set -euo pipefail

RELEASE=0
if [ "${1:-}" = "--release" ]; then
    RELEASE=1
fi

SPECK_HOME="${SPECK_HOME:-$HOME/.speck}"
INITRD="${SPECK_HOME}/initrd/initrd.cpio.gz"

if [ ! -f "${INITRD}" ]; then
    echo "ERROR: initrd not found: ${INITRD}" >&2
    exit 1
fi

# The default `rustc` on PATH may resolve to a Homebrew-managed Rust
# toolchain (no cross-target std libs installed) rather than the
# rustup-managed one that has `aarch64-unknown-linux-musl` installed, even
# when both `rustup` and a separate `rustc` are present. If the default
# rustc lacks the target's std lib, fall back to rustup's toolchain rustc
# explicitly via the RUSTC env var, which cargo respects.
CARGO_RUSTC_ENV=()
if ! rustc --target aarch64-unknown-linux-musl --print sysroot >/dev/null 2>&1 \
   || ! rustc --print sysroot 2>/dev/null | xargs -I{} test -d "{}/lib/rustlib/aarch64-unknown-linux-musl/lib"; then
    if command -v rustup >/dev/null 2>&1; then
        RUSTUP_RUSTC="$(rustup which rustc 2>/dev/null || true)"
        if [ -n "${RUSTUP_RUSTC}" ] && [ -x "${RUSTUP_RUSTC}" ]; then
            echo "==> Speck: default rustc lacks aarch64-unknown-linux-musl; using rustup's rustc (${RUSTUP_RUSTC})"
            CARGO_RUSTC_ENV=(env "RUSTC=${RUSTUP_RUSTC}")
        fi
    fi
fi

echo "==> Speck: cross-compiling vminitd (aarch64-unknown-linux-musl)"
if [ "${RELEASE}" -eq 1 ]; then
    "${CARGO_RUSTC_ENV[@]}" cargo build --target aarch64-unknown-linux-musl -p speck-guest --bin vminitd --release
    VMINITD_BIN="target/aarch64-unknown-linux-musl/release/vminitd"
else
    "${CARGO_RUSTC_ENV[@]}" cargo build --target aarch64-unknown-linux-musl -p speck-guest --bin vminitd
    VMINITD_BIN="target/aarch64-unknown-linux-musl/debug/vminitd"
fi

if [ ! -f "${VMINITD_BIN}" ]; then
    echo "ERROR: built vminitd binary not found at ${VMINITD_BIN}" >&2
    exit 1
fi

mkdir -p "${SPECK_HOME}/tmp"
TMPDIR="$(mktemp -d "${SPECK_HOME}/tmp/speck-dev-repack-vminitd.XXXXXX")"
cleanup() {
    rm -rf "${TMPDIR}"
}
trap cleanup EXIT

# --- functions copied verbatim from scripts/provision-guest-resize-tools.sh ---

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

# --- end copied functions ---

mkdir -p "${TMPDIR}/root"
(
    cd "${TMPDIR}/root"
    unpack_initrd "${INITRD}" | cpio -idm --quiet
)

cp "${VMINITD_BIN}" "${TMPDIR}/root/init"
chmod 755 "${TMPDIR}/root/init"

KIND="$(compression_kind "${INITRD}")"
pack_initrd_to_file "${KIND}" "${INITRD}.tmp"
mv -f "${INITRD}.tmp" "${INITRD}"

echo "==> Speck: repacked ${INITRD} with freshly built vminitd"
