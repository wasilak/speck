//! Host-first TCP connects for guest SYNs.
//!
//! The device holds every guest SYN back. The connector dials the destination
//! from the host first and only then lets smoltcp answer the SYN:
//! - connect succeeds → a listener is opened for the exact destination, the SYN
//!   is released and smoltcp completes the handshake; the connected stream is
//!   handed to the re-origination bridge.
//! - connect fails or times out → the SYN is released with no listener, so
//!   smoltcp answers with RST and the guest sees "connection refused".
//!
//! This keeps port probes (`nc -z`, wait-for-it, testcontainers) truthful:
//! the guest never sees an accepted connection the host could not open.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::sync::oneshot;

use crate::device::SynInfo;

/// Upper bound for a host connect before the guest gets a reset.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Result of a finished host connect for one held SYN.
pub(crate) enum Outcome {
    Connected {
        syn: SynInfo,
        frame: Vec<u8>,
        stream: TcpStream,
    },
    Failed {
        frame: Vec<u8>,
    },
}

struct InFlight {
    frame: Vec<u8>,
    rx: oneshot::Receiver<std::io::Result<TcpStream>>,
}

#[derive(Default)]
pub(crate) struct Connector {
    inflight: HashMap<SynInfo, InFlight>,
    failed_now: Vec<Vec<u8>>,
}

impl Connector {
    /// Start a host connect for a held SYN. Retransmitted SYNs for a connect
    /// already in flight are dropped; the first frame is released later.
    pub fn submit(&mut self, syn: SynInfo, frame: Vec<u8>) {
        if self.inflight.contains_key(&syn) {
            return;
        }
        if is_unroutable(*syn.dst.ip()) {
            self.failed_now.push(frame);
            return;
        }
        let (tx, rx) = oneshot::channel();
        let dst = syn.dst;
        tokio::spawn(async move {
            let result = match tokio::time::timeout(CONNECT_TIMEOUT, connect_host(dst)).await {
                Ok(r) => r,
                Err(_) => Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "host connect timed out",
                )),
            };
            let _ = tx.send(result);
        });
        self.inflight.insert(syn, InFlight { frame, rx });
    }

    /// Collect connects that finished since the last call.
    pub fn poll(&mut self) -> Vec<Outcome> {
        let mut out: Vec<Outcome> = self
            .failed_now
            .drain(..)
            .map(|frame| Outcome::Failed { frame })
            .collect();

        // try_recv consumes the result, so read each receiver exactly once.
        let done: Vec<(SynInfo, Option<TcpStream>)> = self
            .inflight
            .iter_mut()
            .filter_map(|(syn, f)| match f.rx.try_recv() {
                Err(oneshot::error::TryRecvError::Empty) => None,
                Ok(Ok(stream)) => Some((*syn, Some(stream))),
                Ok(Err(e)) => {
                    tracing::debug!(dst = %syn.dst, error = %e, "host connect failed; resetting guest SYN");
                    Some((*syn, None))
                }
                Err(oneshot::error::TryRecvError::Closed) => Some((*syn, None)),
            })
            .collect();

        for (syn, stream) in done {
            let frame = self.inflight.remove(&syn).expect("listed above").frame;
            out.push(match stream {
                Some(stream) => Outcome::Connected { syn, frame, stream },
                None => Outcome::Failed { frame },
            });
        }
        out
    }

    /// Whether host connects are still in flight.
    pub fn is_active(&self) -> bool {
        !self.inflight.is_empty() || !self.failed_now.is_empty()
    }

    #[cfg(test)]
    fn inflight_count(&self) -> usize {
        self.inflight.len()
    }
}

/// Destinations that must never be dialed from the host on the guest's
/// behalf: loopback and unspecified would hit host-local services, and the
/// netstack gateway (172.16.0.1/.2) is Speck itself.
pub(crate) fn is_unroutable(ip: Ipv4Addr) -> bool {
    ip.is_loopback()
        || ip.is_unspecified()
        || ip == Ipv4Addr::new(172, 16, 0, 1)
        || ip == Ipv4Addr::new(172, 16, 0, 2)
}

/// Connect to `dst` from the host network stack (live host routing/DNS).
pub(crate) async fn connect_host(dst: SocketAddrV4) -> std::io::Result<TcpStream> {
    let stream = TcpStream::connect(dst).await?;
    // Enable TCP keepalive so WARP/NAT doesn't kill long-lived
    // connections mid-transfer (e.g. large blob downloads).
    let std_stream = stream.into_std()?;
    let sock: socket2::Socket = std_stream.into();
    let ka = socket2::TcpKeepalive::new()
        .with_time(Duration::from_secs(30))
        .with_interval(Duration::from_secs(10));
    let _ = sock.set_tcp_keepalive(&ka);
    let std_stream: std::net::TcpStream = sock.into();
    std_stream.set_nonblocking(true)?;
    TcpStream::from_std(std_stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn syn(dst: [u8; 4], port: u16) -> SynInfo {
        SynInfo {
            src: SocketAddrV4::new(Ipv4Addr::new(172, 17, 0, 2), 40000),
            dst: SocketAddrV4::new(Ipv4Addr::from(dst), port),
        }
    }

    #[test]
    fn unroutable_covers_loopback_unspecified_and_gateway() {
        assert!(is_unroutable(Ipv4Addr::new(127, 0, 0, 1)));
        assert!(is_unroutable(Ipv4Addr::UNSPECIFIED));
        assert!(is_unroutable(Ipv4Addr::new(172, 16, 0, 1)));
        assert!(!is_unroutable(Ipv4Addr::new(140, 82, 121, 4)));
    }

    #[tokio::test]
    async fn unroutable_destination_fails_without_dialing() {
        let mut c = Connector::default();
        c.submit(syn([127, 0, 0, 1], 22), vec![1]);
        assert_eq!(c.inflight_count(), 0);
        let out = c.poll();
        assert!(matches!(out.as_slice(), [Outcome::Failed { frame, .. }] if frame == &vec![1]));
    }

    /// Regression: poll() once read each receiver twice, so the stream of a
    /// successful connect was consumed and every SYN was reset.
    #[tokio::test]
    async fn successful_connect_is_reported_as_connected() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (tx, rx) = oneshot::channel();
        tx.send(Ok(stream)).unwrap();

        let mut c = Connector::default();
        let s = syn([140, 82, 121, 4], 22);
        c.inflight.insert(s, InFlight { frame: vec![7], rx });

        let out = c.poll();
        assert!(
            matches!(out.as_slice(), [Outcome::Connected { syn, frame, .. }] if *syn == s && frame == &vec![7])
        );
        assert_eq!(c.inflight_count(), 0);
    }

    #[tokio::test]
    async fn retransmitted_syn_is_deduplicated() {
        let mut c = Connector::default();
        // TEST-NET-1: never answers, so the connect stays in flight.
        c.submit(syn([192, 0, 2, 1], 9), vec![1]);
        c.submit(syn([192, 0, 2, 1], 9), vec![2]);
        assert_eq!(c.inflight_count(), 1);
    }
}
