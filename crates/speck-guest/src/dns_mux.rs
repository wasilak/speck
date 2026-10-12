//! Matching host DNS answers to guest clients by query ID.
//!
//! The guest sends every query over one vsock stream and the host answers
//! them concurrently, so answers can come back in any order. Each forwarded
//! query gets a unique ID (a tag) for the trip; the answer's ID finds the
//! client and the client's original ID is put back.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Queries the host has not answered within this time are forgotten.
pub const PENDING_TIMEOUT: Duration = Duration::from_secs(15);

/// Queries waiting for a host answer, keyed by tag.
pub struct Pending<T> {
    by_tag: HashMap<u16, (u16, T, Instant)>,
    next: u16,
}

impl<T> Default for Pending<T> {
    fn default() -> Self {
        Self {
            by_tag: HashMap::new(),
            next: 0,
        }
    }
}

impl<T> Pending<T> {
    /// Register `query` for `client`: rewrite its ID to a free tag.
    /// Returns `false` (and leaves the query alone) for a malformed query or
    /// when every tag is in use.
    pub fn register(&mut self, query: &mut [u8], client: T, now: Instant) -> bool {
        if query.len() < 12 {
            return false;
        }
        self.by_tag
            .retain(|_, (_, _, sent)| now.duration_since(*sent) < PENDING_TIMEOUT);
        let Some(tag) = (0..=u16::MAX)
            .map(|i| self.next.wrapping_add(i))
            .find(|tag| !self.by_tag.contains_key(tag))
        else {
            return false;
        };
        self.next = tag.wrapping_add(1);
        let original = u16::from_be_bytes([query[0], query[1]]);
        query[..2].copy_from_slice(&tag.to_be_bytes());
        self.by_tag.insert(tag, (original, client, now));
        true
    }

    /// Find the client of a host `answer` and restore its original ID.
    pub fn complete(&mut self, answer: &mut [u8]) -> Option<T> {
        if answer.len() < 12 {
            return None;
        }
        let tag = u16::from_be_bytes([answer[0], answer[1]]);
        let (original, client, _) = self.by_tag.remove(&tag)?;
        answer[..2].copy_from_slice(&original.to_be_bytes());
        Some(client)
    }

    /// Forget the queries of clients `gone` says are gone.
    pub fn forget(&mut self, gone: impl Fn(&T) -> bool) {
        self.by_tag.retain(|_, (_, client, _)| !gone(client));
    }

    pub fn len(&self) -> usize {
        self.by_tag.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_tag.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(id: u16) -> Vec<u8> {
        let mut q = vec![0u8; 12];
        q[..2].copy_from_slice(&id.to_be_bytes());
        q
    }

    #[test]
    fn answers_out_of_order_reach_their_clients_with_original_ids() {
        let now = Instant::now();
        let mut pending = Pending::default();
        // Two clients happen to use the same query ID.
        let mut a = query(7);
        let mut b = query(7);
        assert!(pending.register(&mut a, "a", now));
        assert!(pending.register(&mut b, "b", now));
        assert_ne!(a[..2], b[..2], "each trip gets its own tag");

        let mut answer_b = b.clone();
        let mut answer_a = a.clone();
        assert_eq!(pending.complete(&mut answer_b), Some("b"));
        assert_eq!(pending.complete(&mut answer_a), Some("a"));
        assert_eq!(answer_a[..2], 7u16.to_be_bytes());
        assert_eq!(answer_b[..2], 7u16.to_be_bytes());
        assert!(pending.is_empty());
    }

    #[test]
    fn unknown_or_repeated_answers_are_dropped() {
        let mut pending = Pending::default();
        let mut q = query(1);
        assert!(pending.register(&mut q, 1, Instant::now()));
        let mut answer = q.clone();
        assert_eq!(pending.complete(&mut answer.clone()), Some(1));
        assert_eq!(pending.complete(&mut answer), None);
        assert_eq!(pending.complete(&mut [0u8; 4]), None);
    }

    #[test]
    fn stale_queries_expire_and_gone_clients_are_forgotten() {
        let start = Instant::now();
        let mut pending = Pending::default();
        assert!(pending.register(&mut query(1), 1, start));
        assert!(pending.register(&mut query(2), 2, start + Duration::from_secs(1)));
        assert_eq!(pending.len(), 2);
        assert!(pending.register(&mut query(3), 3, start + PENDING_TIMEOUT * 2));
        assert_eq!(pending.len(), 1, "queries 1 and 2 expired");
        pending.forget(|client| *client == 3);
        assert!(pending.is_empty());
    }

    #[test]
    fn malformed_queries_are_rejected() {
        let mut pending = Pending::default();
        assert!(!pending.register(&mut [0u8; 5], (), Instant::now()));
    }
}
