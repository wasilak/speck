# 04-06: Guest DNS Forwarder

**Status:** ✅ Complete  
**Plans:** 1 plan (04-06-PLAN.md)  
**Commits:** *(included in Wave 3 commit)*

## Summary

Implemented a DNS forwarder running inside the guest micro-VM. Listens on `AF_VSOCK` on the configured port, accepts length-prefixed DNS wire queries, resolves them by forwarding to the real DNS resolver via UDP:53, and writes back wire responses. Integrated into the `vminitd` binary via cmdline argument `dns_vsock_port`.

## Files Created

| File | Description |
|------|-------------|
| `crates/speck-guest/src/dns_forwarder.rs` | AF_VSOCK listener + UDP:53 poll loop, length-prefixed wire format |

## Files Modified

| File | Change |
|------|--------|
| `crates/speck-guest/src/lib.rs` | Added `pub mod dns_forwarder;` |
| `crates/speck-guest/src/bin/vminitd.rs` | Added `parse_cmdline_dns_port()` + DNS forwarder spawn thread |

## Design Decisions

- Uses blocking I/O (spawned as `std::thread`) since speck-guest targets musl Linux with no async runtime
- Length-prefixed wire format matches `dns::spawn_dns_proxy` protocol contract
- UDP dgram socket created via raw libc socket/BindToAddress/fcntl for non-blocking poll loop
- `#[cfg(target_os = "linux")]` guarded modules ensure cross-compile safety on macOS

## Verification

- `cargo build -p speck-guest --target aarch64-unknown-linux-musl --release` ✅
- Initrd updated at `/tmp/speck-initrd.cpio.gz` (204K)
- Both `dns_forwarder::serve` and `vsock_echo::serve` confirmed linked via `nm`

## Blockers

None. Cross-compiles cleanly. Requires codesigned host binary and boot artifacts for end-to-end testing.
