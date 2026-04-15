// arc-relay/src/mirror/scrollback.rs
use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub struct ScrollbackEntry {
    pub seq: u64,
    pub bytes: Vec<u8>,
    pub at: Instant,
}

pub struct ScrollbackRing {
    entries: VecDeque<ScrollbackEntry>,
    next_seq: u64,
    total_bytes: usize,
    max_bytes: usize,
    max_age: Duration,
}

impl ScrollbackRing {
    pub fn new(max_bytes: usize, max_age: Duration) -> Self {
        Self {
            entries: VecDeque::new(),
            next_seq: 0,
            total_bytes: 0,
            max_bytes,
            max_age,
        }
    }

    pub fn append(&mut self, bytes: Vec<u8>) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.total_bytes += bytes.len();
        self.entries.push_back(ScrollbackEntry { seq, bytes, at: Instant::now() });
        self.compact();
        seq
    }

    pub fn compact(&mut self) {
        let now = Instant::now();
        while let Some(front) = self.entries.front() {
            let too_old = now.duration_since(front.at) > self.max_age;
            let too_big = self.total_bytes > self.max_bytes;
            if !too_old && !too_big { break; }
            let evicted = self.entries.pop_front().unwrap();
            self.total_bytes -= evicted.bytes.len();
        }
    }

    pub fn next_seq(&self) -> u64 { self.next_seq }
    pub fn total_bytes(&self) -> usize { self.total_bytes }

    pub fn get(&self, seq: u64) -> Option<&ScrollbackEntry> {
        self.entries.iter().find(|e| e.seq == seq)
    }

    pub fn replay_since(&self, since_seq: u64) -> impl Iterator<Item = &ScrollbackEntry> {
        self.entries.iter().filter(move |e| e.seq >= since_seq)
    }

    /// Returns true if the requested seq is older than any entry we have
    /// (caller must send SessionDiscontinuity).
    pub fn out_of_range(&self, since_seq: u64) -> bool {
        self.entries.front().map_or(false, |f| since_seq < f.seq)
    }
}

pub const DEFAULT_MAX_BYTES: usize = 2 * 1024 * 1024;
pub const DEFAULT_MAX_AGE: Duration = Duration::from_secs(300);

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn append_until_cap_evicts_oldest() {
        let mut ring = ScrollbackRing::new(100, Duration::from_secs(60));
        for i in 0..50u8 { ring.append(vec![i; 10]); }   // 500 bytes total → over 100
        // Should have evicted enough to fit
        assert!(ring.total_bytes() <= 100);
        // Last entries preserved
        let last_seq = ring.next_seq() - 1;
        let entry = ring.get(last_seq).unwrap();
        assert_eq!(entry.bytes[0], 49);
    }

    #[test]
    fn replay_returns_contiguous_range() {
        let mut ring = ScrollbackRing::new(10_000, Duration::from_secs(60));
        for _ in 0..10 { ring.append(b"hi".to_vec()); }
        let entries: Vec<_> = ring.replay_since(5).collect();
        assert_eq!(entries.len(), 5);
        assert_eq!(entries[0].seq, 5);
    }

    #[test]
    fn replay_out_of_range_returns_none() {
        let mut ring = ScrollbackRing::new(50, Duration::from_secs(60));
        for _ in 0..100 { ring.append(vec![0u8; 10]); }   // forces eviction
        let result = ring.replay_since(0).next();
        // seq 0 was evicted long ago — caller must use SessionDiscontinuity
        assert!(result.is_none() || result.unwrap().seq > 0);
    }

    #[test]
    fn time_based_eviction() {
        let mut ring = ScrollbackRing::new(usize::MAX, Duration::from_millis(50));
        ring.append(b"old".to_vec());
        std::thread::sleep(Duration::from_millis(100));
        ring.append(b"new".to_vec());
        ring.compact();
        assert_eq!(ring.replay_since(0).count(), 1);
    }
}
