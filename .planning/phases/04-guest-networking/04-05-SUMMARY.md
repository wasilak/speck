# 04-05: Integration Tests

**Status:** ✅ Complete (manual execution pending)  
**Plans:** 1 plan (04-05-PLAN.md)  
**Commits:** *(included in Wave 3 commit)*

## Summary

Created integration test file for the networking stack with three `#[ignore]`d tests that require a codesigned binary and boot artifacts to run. Made `FdDevice` and `SmoltcpInterface` publicly visible for test usage.

## Files Created

| File | Description |
|------|-------------|
| `crates/speck-vz/tests/networking_integration_test.rs` | Three `#[ignore]`d integration tests |

## Files Modified

| File | Change |
|------|--------|
| `crates/speck-net/src/device.rs` | Made `FdDevice`, `FdRxToken`, `FdTxToken` `pub` |
| `crates/speck-net/src/interface.rs` | Made `SmoltcpInterface` `pub` |
| `crates/speck-vz/src/guest.rs` | Added `netstack_fd()`, `dns_vsock_fd()` public methods |
| `crates/speck-vz/src/vsock.rs` | Added `AsRawFd` impl for `VzSocket` |
| `crates/speck-vz/Cargo.toml` | Added `speck-net` + `smoltcp` dev-dependencies |

## Tests

| Test | Status | Purpose |
|------|--------|---------|
| `test_netstack_arp` | `#[ignore]d` | ARP resolution inside the VM |
| `test_netstack_no_fd_before_start` | `#[ignore]d` | Netstack FDs are only valid after VM start |
| `test_dns_proxy_vsock` | `#[ignore]d` | DNS proxy can handle queries over vsock |

## Verification

- `cargo check -p speck-vz --tests` ✅

## Blockers

- Tests require codesigned binary (`com.apple.security.virtualization`) and boot artifacts (kernel + initrd)
- Manual execution deferred to later development phase
