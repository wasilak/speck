//! Small DNS wire-format helpers for the guest forwarder (no I/O, testable).

/// EDNS UDP payload size advertised for queries that arrived over TCP.
pub const TCP_EDNS_PAYLOAD: u16 = u16::MAX;

/// Add an EDNS OPT record advertising `payload` to `query` unless it already
/// carries additional records.
///
/// The host resolver only returns a full answer when it fits the size the
/// query advertises (EDNS, else 512 bytes), because it cannot tell whether
/// the client asked over UDP or TCP. Queries that came in over TCP can take
/// any size, so the forwarder advertises that explicitly.
pub fn with_edns(query: &[u8], payload: u16) -> Vec<u8> {
    let mut out = query.to_vec();
    if query.len() < 12 || u16::from_be_bytes([query[10], query[11]]) != 0 {
        return out;
    }
    out[11] = 1; // ARCOUNT = 1
    out.push(0); // root name
    out.extend_from_slice(&41u16.to_be_bytes()); // TYPE = OPT
    out.extend_from_slice(&payload.to_be_bytes()); // CLASS = UDP payload size
    out.extend_from_slice(&[0, 0, 0, 0]); // extended RCODE, version, flags
    out.extend_from_slice(&[0, 0]); // RDLENGTH = 0
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(arcount: u8) -> Vec<u8> {
        let mut q = vec![0x12, 0x34, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, arcount];
        q.extend_from_slice(&[3, b'f', b'o', b'o', 0, 0, 16, 0, 1]);
        q
    }

    #[test]
    fn adds_opt_record_to_plain_query() {
        let q = query(0);
        let out = with_edns(&q, TCP_EDNS_PAYLOAD);
        assert_eq!(out[11], 1);
        assert_eq!(&out[..11], &q[..11]);
        assert_eq!(&out[q.len()..], &[0, 0, 41, 0xff, 0xff, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn leaves_queries_with_additional_records_alone() {
        let q = query(1);
        assert_eq!(with_edns(&q, TCP_EDNS_PAYLOAD), q);
        assert_eq!(with_edns(&[1, 2, 3], TCP_EDNS_PAYLOAD), vec![1, 2, 3]);
    }
}
