mod baseline {
struct RingBuffer {
    bytes: Vec<u8>,
    cap: usize,
}

impl RingBuffer {
    fn new(cap: usize) -> Self {
        Self { bytes: Vec::new(), cap }
    }
    fn push(&mut self, chunk: &[u8]) {
        if chunk.len() >= self.cap {
            // Single chunk bigger than the cap — keep only the tail.
            let start = chunk.len() - self.cap;
            self.bytes.clear();
            self.bytes.extend_from_slice(&chunk[start..]);
            return;
        }
        let total = self.bytes.len() + chunk.len();
        if total > self.cap {
            let drop_count = total - self.cap;
            self.bytes.drain(0..drop_count);
        }
        self.bytes.extend_from_slice(chunk);
    }
    fn snapshot(&self) -> Vec<u8> {
        self.bytes.clone()
    }
    fn snapshot_tail(&self, max: usize) -> Vec<u8> {
        if self.bytes.len() <= max {
            return self.bytes.clone();
        }
        let start = self.bytes.len() - max;
        self.bytes[start..].to_vec()
    }
}

pub fn run() -> u128 {
    let chunk = vec![b'x'; 8192];
    let mut buffer = RingBuffer::new(1024 * 1024);
    buffer.push(&vec![0; 1024 * 1024]);
    let started = std::time::Instant::now();
    for _ in 0..16384 {
        buffer.push(std::hint::black_box(&chunk));
        std::hint::black_box(&buffer);
    }
    let elapsed = started.elapsed();
    assert_eq!(buffer.snapshot(), vec![b'x'; 1024 * 1024]);
    assert_eq!(buffer.snapshot_tail(3), b"xxx");
    elapsed.as_micros()
}

}
mod optimized {
use std::collections::VecDeque;
struct RingBuffer {
    bytes: VecDeque<u8>,
    cap: usize,
}

impl RingBuffer {
    fn new(cap: usize) -> Self {
        Self { bytes: VecDeque::new(), cap }
    }
    fn push(&mut self, chunk: &[u8]) {
        if chunk.len() >= self.cap {
            // Single chunk bigger than the cap — keep only the tail.
            let start = chunk.len() - self.cap;
            self.bytes.clear();
            self.bytes.extend(&chunk[start..]);
            return;
        }
        let total = self.bytes.len() + chunk.len();
        if total > self.cap {
            let drop_count = total - self.cap;
            self.bytes.drain(0..drop_count);
        }
        self.bytes.extend(chunk);
    }
    fn snapshot(&self) -> Vec<u8> {
        self.bytes.iter().copied().collect()
    }
    fn snapshot_tail(&self, max: usize) -> Vec<u8> {
        if self.bytes.len() <= max {
            return self.snapshot();
        }
        let start = self.bytes.len() - max;
        self.bytes.range(start..).copied().collect()
    }
}

pub fn run() -> u128 {
    let chunk = vec![b'x'; 8192];
    let mut buffer = RingBuffer::new(1024 * 1024);
    buffer.push(&vec![0; 1024 * 1024]);
    let started = std::time::Instant::now();
    for _ in 0..16384 {
        buffer.push(std::hint::black_box(&chunk));
        std::hint::black_box(&buffer);
    }
    let elapsed = started.elapsed();
    assert_eq!(buffer.snapshot(), vec![b'x'; 1024 * 1024]);
    assert_eq!(buffer.snapshot_tail(3), b"xxx");
    elapsed.as_micros()
}

#[test]
fn varied_chunks_match_reference_output() {
    for capacity in [0, 1, 8, 31, 8192] {
        let mut ring = RingBuffer::new(capacity);
        let mut reference = Vec::new();
        let mut state = 17u64;
        for _ in 0..10000 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let length = (state >> 32) as usize % (2 * capacity + 81);
            let bytes = vec![(state >> 8) as u8; length];
            reference.extend_from_slice(&bytes);
            let excess = reference.len().saturating_sub(capacity);
            reference.drain(..excess);
            ring.push(&bytes);
            assert_eq!(ring.snapshot(), reference);
            for limit in [0, 1, 7, capacity, capacity + 1] {
                assert_eq!(ring.snapshot_tail(limit), reference[reference.len().saturating_sub(limit)..]);
            }
        }
    }
}

}
fn main() { let implementation = std::env::args().nth(1).unwrap(); println!("{}", match implementation.as_str() { "baseline" => baseline::run(), "optimized" => optimized::run(), _ => panic!("expected baseline or optimized") }); }
