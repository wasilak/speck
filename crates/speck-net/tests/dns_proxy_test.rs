//! Network-free DNS proxy path tests.
//!
//! Uses `mockall::mock!` inside this integration test file because
//! `#[cfg_attr(test, mockall::automock)]` only generates `MockResolver` when
//! compiling the library with `--cfg test` (unit tests), but integration tests
//! compile the crate as a normal dependency where `#[cfg(test)]` is inactive.
//!
//! Tests verify the routing behaviour of `speck_net::dns::resolve_with_table`:
//!
//! - NXDOMAIN:  RCODE 3 returned when the mocked resolver reports NXDOMAIN
//! - SERVFAIL:  RCODE 2 returned when the mocked resolver returns None
//! - VPN-scoped: `direct_query` is called, `resolve` never called
//! - Default:    `resolve` is called, `direct_query` never called

use std::net::IpAddr;

use speck_net::resolver_table::ResolverTable;

// ---------------------------------------------------------------------------
// Mock resolver — local mockall mock implementing `speck_net::dns::Resolver`
// so we never open real sockets in these tests.
// ---------------------------------------------------------------------------

mockall::mock! {
    pub ProxyResolver {}
    impl speck_net::dns::Resolver for ProxyResolver {
        fn resolve(&self, domain: &str, query: &[u8]) -> Option<Vec<u8>>;
        fn direct_query(&self, nameserver: IpAddr, query: &[u8]) -> Option<Vec<u8>>;
    }
}

// ---------------------------------------------------------------------------
// DNS fixture helpers — minimal query / response bytes that exercise the
// resolver routing without relying on private build_* helpers in dns.rs.
// ---------------------------------------------------------------------------

/// Build a minimal DNS A-record query for `domain`.
fn make_query(domain: &str) -> Vec<u8> {
    let mut buf = Vec::with_capacity(512);
    buf.extend_from_slice(&[0x12, 0x34]); // ID
    buf.extend_from_slice(&[0x01, 0x00]); // flags: RD=1
    buf.extend_from_slice(&[0x00, 0x01]); // QDCOUNT = 1
    buf.extend_from_slice(&[0x00, 0x00]); // ANCOUNT = 0
    buf.extend_from_slice(&[0x00, 0x00]); // NSCOUNT = 0
    buf.extend_from_slice(&[0x00, 0x00]); // ARCOUNT = 0
    // QNAME — encoded label sequence
    for label in domain.split('.') {
        buf.push(label.len() as u8);
        buf.extend_from_slice(label.as_bytes());
    }
    buf.push(0x00); // null terminator
    buf.extend_from_slice(&[0x00, 0x01]); // QTYPE = A
    buf.extend_from_slice(&[0x00, 0x01]); // QCLASS = IN
    buf
}

/// Build a minimal NXDOMAIN response (RCODE 3) echoing the question from `query`.
fn make_nxdomain_response(query: &[u8]) -> Vec<u8> {
    let id = &query[..2];
    let question = &query[12..];
    let mut resp = Vec::with_capacity(512);
    resp.extend_from_slice(id);
    resp.extend_from_slice(&[0x81, 0x83]); // QR=1, RD=1, RA=1, RCODE=3
    resp.extend_from_slice(&[0x00, 0x01]); // QDCOUNT = 1
    resp.extend_from_slice(&[0x00, 0x00]); // ANCOUNT = 0
    resp.extend_from_slice(&[0x00, 0x00]); // NSCOUNT = 0
    resp.extend_from_slice(&[0x00, 0x00]); // ARCOUNT = 0
    resp.extend_from_slice(question);
    resp
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn nxdomain_path_returns_rcode_3() {
    let query = make_query("nxdomain-test.example");
    let _nx_resp = make_nxdomain_response(&query);

    let mut mock = MockProxyResolver::new();
    mock.expect_resolve()
        .with(
            mockall::predicate::eq("nxdomain-test.example"),
            mockall::predicate::always(),
        )
        .times(1)
        .returning(move |_, q| Some(make_nxdomain_response(q)));

    // Set the expectation: direct_query should never be reached
    // (resolve_with_table will call resolve on the default path).
    mock.expect_direct_query().never();

    let table = ResolverTable::default();
    let result = speck_net::dns::resolve_with_table(&mock, &table, "nxdomain-test.example", &query);

    // NXDOMAIN on the default path should be preserved (RCODE 3).
    assert_eq!(result[3] & 0x0f, 3, "expected NXDOMAIN (RCODE 3)");
}

#[test]
fn servfail_path_returns_rcode_2_when_resolve_returns_none() {
    let query = make_query("servfail-test.example");

    let mut mock = MockProxyResolver::new();
    mock.expect_resolve()
        .with(
            mockall::predicate::eq("servfail-test.example"),
            mockall::predicate::always(),
        )
        .times(1)
        .returning(|_, _| None);
    mock.expect_direct_query().never();

    let table = ResolverTable::default();
    let result = speck_net::dns::resolve_with_table(&mock, &table, "servfail-test.example", &query);

    // When resolve returns None the function builds a SERVFAIL (RCODE 2).
    assert_eq!(result[3] & 0x0f, 2, "expected SERVFAIL (RCODE 2)");
}

#[test]
fn vpn_scoped_path_calls_direct_query_not_resolve() {
    let query = make_query("host.corp.example");
    let vpn_ns: IpAddr = "10.0.0.1".parse().unwrap();

    let mut mock = MockProxyResolver::new();
    // direct_query MUST be called exactly once
    mock.expect_direct_query()
        .with(mockall::predicate::eq(vpn_ns), mockall::predicate::always())
        .times(1)
        .returning(|_, q| Some(q.to_vec()));
    // resolve MUST NOT be called
    mock.expect_resolve().never();

    let mut table = ResolverTable::default();
    table.add_entry("corp.example".to_string(), vec![vpn_ns]);
    let _result = speck_net::dns::resolve_with_table(&mock, &table, "host.corp.example", &query);

    // Expectations verified by mockall on drop — direct_query was called, resolve was not.
}

#[test]
fn default_path_calls_resolve_not_direct_query() {
    let query = make_query("example.com");

    let mut mock = MockProxyResolver::new();
    mock.expect_resolve()
        .with(
            mockall::predicate::eq("example.com"),
            mockall::predicate::always(),
        )
        .times(1)
        .returning(|_, q| Some(q.to_vec()));
    mock.expect_direct_query().never();

    let table = ResolverTable::default();
    let _result = speck_net::dns::resolve_with_table(&mock, &table, "example.com", &query);

    // Expectations verified by mockall on drop.
}
