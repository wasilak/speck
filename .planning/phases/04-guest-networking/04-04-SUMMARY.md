# 04-04: TCP Re-origination, DHCP, DNS Proxy, MTU Detection

**Status:** ✅ Complete  
**Plans:** 1 plan (04-04-PLAN.md)  
**Commits:** *(included in Wave 3 commit)*

## Summary

Implemented the core netstack services for the Speck micro-VM: TCP re-origination bridge (guest→host TCP connections re-originate from host), DHCP server (guest gets IP on 172.16.0.0/24), vsock-based DNS proxy (resolves queries via macOS `getaddrinfo` for live VPN/WARP compatibility), and host MTU detection (via `getifaddrs` + `sysctl` on macOS).

## Files Created

| File | Description |
|------|-------------|
| `crates/speck-net/src/reorigin.rs` | `ReoriginBridge` — poll_bridges, handle_new_connections, loopback exclusion |
| `crates/speck-net/src/dhcp.rs` | `DhcpServer` — DISCOVER→OFFER, REQUEST→ACK, binary DHCP wire format |
| `crates/speck-net/src/dns.rs` | `spawn_dns_proxy` — async vsock DNS proxy, length-prefixed wire, getaddrinfo |
| `crates/speck-net/src/mtu.rs` | `detect_host_mtu()`, `mss_for_mtu()` via getifaddrs + sysctl |

## Files Modified

| File | Change |
|------|--------|
| `crates/speck-net/src/lib.rs` | Added module declarations + SpeckNet wiring (poll loop with reorigin → DHCP → egress) |
| `crates/speck-net/Cargo.toml` | Added `"fs"` feature to tokio |

## Design Decisions

- **ReoriginBridge** uses `smoltcp::iface::SocketHandle` as HashMap key to track guest→host TCP stream pairs
- **DhcpServer** implements minimal DHCP (no lease tracking, no DORA retransmission) — sufficient for a single-guest VM
- **DNS proxy** uses length-prefixed binary wire format over vsock (2-byte BE length + raw DNS wire query)
- **MTU detection** iterates `getifaddrs` for non-loopback UP interfaces, then queries `sysctl net.interface.{name}.mtu`
- MSS clamping deferred — smoltcp 0.13 does not expose `set_mss`; handled by interface MTU negotiation

## Verification

- `cargo check -p speck-net` ✅ (0 errors, 2 expected dead-code warnings)
- `cargo check -p speck-vz --tests` ✅
- `cargo check -p speck-core` ✅
- `cargo check -p speck-cli` ✅

## Blockers

None. All modules compile cleanly.
