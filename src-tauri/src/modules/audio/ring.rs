//! The lock-free handoff between the mixer and the device callback.
//!
//! One producer (the fill thread) and one consumer (the device's real-time
//! callback). The callback runs on a thread the kernel will not reschedule
//! politely: if it allocates, takes a contended lock, or waits on IO, the
//! deadline is missed and the user hears a click. So it does exactly one thing
//! here — copy samples out of a fixed allocation and move an atomic index.
//!
//! ## Why the *consumer* performs the flush
//!
//! A seek has to throw away everything already mixed, and the obvious way to
//! do that is for the seeking thread to move the read cursor forward. It must
//! not: the read cursor is what tells the producer which slots are free, so
//! moving it from the outside lets the producer overwrite samples the callback
//! is in the middle of reading, and a torn `f32` is not a quiet glitch, it is
//! full-scale noise.
//!
//! Instead a flush publishes a *discard point* and bumps a generation counter.
//! The callback notices the counter changed and jumps its own cursor to that
//! point, which is the one place it is safe to do. Until it does, the producer
//! simply sees a full ring and waits — one callback period, a few
//! milliseconds. Nothing is ever overwritten while it might still be read.
//!
//! ## Underruns are silence
//!
//! [`RingConsumer::pop`] fills what it can and reports how much. It never
//! blocks and never returns junk; the caller zeroes the rest. A momentary
//! shortfall is then a few milliseconds of silence, which is what every audio
//! stack in the world does with an underrun, rather than a stall that would
//! take the whole device down with it.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

/// The shared allocation. Never resized, never reallocated.
///
/// Indices are *monotonic sample counts*, not slot numbers, and are reduced
/// modulo the capacity only when a slot is touched. That makes "how full is
/// it" a subtraction rather than a case analysis, and the classic full/empty
/// ambiguity of two wrapping pointers simply does not arise. At 96 kHz stereo
/// a `usize` counter wraps after roughly three million years.
struct SampleRing {
    slots: Box<[UnsafeCell<f32>]>,
    capacity: usize,
    /// Samples written. Owned by the producer.
    write: AtomicUsize,
    /// Samples read. Owned by the consumer.
    read: AtomicUsize,
    /// Bumped by the producer on flush; the consumer acts on the change.
    generation: AtomicU64,
    /// Where the consumer should jump to when it notices a new generation.
    discard: AtomicUsize,
    /// Samples the consumer wanted and did not get. Diagnostics.
    starved: AtomicU64,
}

/// # Safety
///
/// The producer only ever writes slots in `[write, read + capacity)` and the
/// consumer only ever reads slots in `[read, write)`. Those ranges are
/// disjoint by construction, and each index is published with a `Release`
/// store that the other side reads with `Acquire`, so the samples a reader
/// sees are exactly the ones written before the index that admitted them.
unsafe impl Sync for SampleRing {}
unsafe impl Send for SampleRing {}

impl SampleRing {
    /// A raw pointer to the sample storage.
    ///
    /// `UnsafeCell<f32>` is `repr(transparent)`, so the slot array *is* an
    /// array of `f32` as far as the copies below are concerned.
    fn base(&self) -> *mut f32 {
        self.slots.as_ptr() as *mut f32
    }
}

/// Build a ring holding `capacity` samples (not sample *frames*) and split it
/// into its two ends.
///
/// Returning two distinct handles rather than one shared object is what makes
/// the single-producer/single-consumer rule a property of the type system
/// instead of a comment somebody will eventually not read.
pub fn ring(capacity: usize) -> (RingProducer, RingConsumer) {
    let capacity = capacity.max(2);
    let slots = (0..capacity)
        .map(|_| UnsafeCell::new(0.0))
        .collect::<Vec<_>>()
        .into_boxed_slice();
    let ring = Arc::new(SampleRing {
        slots,
        capacity,
        write: AtomicUsize::new(0),
        read: AtomicUsize::new(0),
        generation: AtomicU64::new(0),
        discard: AtomicUsize::new(0),
        starved: AtomicU64::new(0),
    });
    (
        RingProducer {
            ring: Arc::clone(&ring),
        },
        RingConsumer {
            ring,
            generation: 0,
        },
    )
}

/// The mixer's end.
pub struct RingProducer {
    ring: Arc<SampleRing>,
}

impl RingProducer {
    pub fn capacity(&self) -> usize {
        self.ring.capacity
    }

    /// Samples written but not yet read.
    pub fn len(&self) -> usize {
        let write = self.ring.write.load(Ordering::Relaxed);
        let read = self.ring.read.load(Ordering::Acquire);
        write.saturating_sub(read)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Room for this many more samples.
    pub fn free(&self) -> usize {
        self.ring.capacity - self.len()
    }

    /// Write as much of `samples` as fits, and report how much that was.
    ///
    /// A short write is normal and not an error: the ring being full is the
    /// back-pressure that keeps the mixer from running arbitrarily far ahead
    /// of the playhead, which is the same reason the renderer has a read-ahead
    /// limit.
    pub fn push(&mut self, samples: &[f32]) -> usize {
        let write = self.ring.write.load(Ordering::Relaxed);
        let count = samples.len().min(self.free());
        if count == 0 {
            return 0;
        }

        let capacity = self.ring.capacity;
        let offset = write % capacity;
        let first = count.min(capacity - offset);
        // Safety: `[write, write + count)` is inside the free region computed
        // above, so no slot here can be one the consumer is reading.
        unsafe {
            let base = self.ring.base();
            std::ptr::copy_nonoverlapping(samples.as_ptr(), base.add(offset), first);
            if count > first {
                std::ptr::copy_nonoverlapping(samples.as_ptr().add(first), base, count - first);
            }
        }
        self.ring.write.store(write + count, Ordering::Release);
        count
    }

    /// Throw away everything unread.
    ///
    /// Publishes the discard point *before* the generation so a consumer that
    /// sees the new generation is guaranteed to see the point that goes with
    /// it. The samples are not actually gone until the callback next runs;
    /// what matters is that nothing after this call is heard from before it.
    pub fn flush(&mut self) {
        let write = self.ring.write.load(Ordering::Relaxed);
        self.ring.discard.store(write, Ordering::Relaxed);
        self.ring.generation.fetch_add(1, Ordering::Release);
    }
}

/// The device callback's end. Every method here is real-time safe.
pub struct RingConsumer {
    ring: Arc<SampleRing>,
    /// The generation this consumer has already acted on.
    generation: u64,
}

impl RingConsumer {
    pub fn capacity(&self) -> usize {
        self.ring.capacity
    }

    pub fn len(&self) -> usize {
        let write = self.ring.write.load(Ordering::Acquire);
        let read = self.ring.read.load(Ordering::Relaxed);
        write.saturating_sub(read)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How many samples have been asked for and were not there.
    pub fn starved(&self) -> u64 {
        self.ring.starved.load(Ordering::Relaxed)
    }

    /// Fill `out` and report how many samples were real.
    ///
    /// The caller is responsible for the tail: `out[filled..]` is left
    /// untouched, because the device callback wants to write silence into it
    /// in the sample format the device asked for, not in ours.
    pub fn pop(&mut self, out: &mut [f32]) -> usize {
        self.apply_flush();

        let read = self.ring.read.load(Ordering::Relaxed);
        let write = self.ring.write.load(Ordering::Acquire);
        let available = write - read;
        let count = out.len().min(available);
        if count < out.len() {
            self.ring
                .starved
                .fetch_add((out.len() - count) as u64, Ordering::Relaxed);
        }
        if count == 0 {
            return 0;
        }

        let capacity = self.ring.capacity;
        let offset = read % capacity;
        let first = count.min(capacity - offset);
        // Safety: `[read, read + count)` was written before `write` was
        // published, and the producer will not reuse those slots until `read`
        // moves past them below.
        unsafe {
            let base = self.ring.base();
            std::ptr::copy_nonoverlapping(base.add(offset), out.as_mut_ptr(), first);
            if count > first {
                std::ptr::copy_nonoverlapping(base, out.as_mut_ptr().add(first), count - first);
            }
        }
        self.ring.read.store(read + count, Ordering::Release);
        count
    }

    /// Act on a pending flush by jumping the read cursor to the discard point.
    ///
    /// Clamped to `write` because the producer may have written more since;
    /// jumping past it would make the ring look enormous rather than empty.
    fn apply_flush(&mut self) {
        let generation = self.ring.generation.load(Ordering::Acquire);
        if generation == self.generation {
            return;
        }
        self.generation = generation;

        let discard = self.ring.discard.load(Ordering::Relaxed);
        let write = self.ring.write.load(Ordering::Acquire);
        let read = self.ring.read.load(Ordering::Relaxed);
        let target = discard.min(write);
        if target > read {
            self.ring.read.store(target, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_goes_in_comes_out_in_order() {
        let (mut tx, mut rx) = ring(16);
        assert_eq!(tx.push(&[1.0, 2.0, 3.0]), 3);

        let mut out = [0.0; 3];
        assert_eq!(rx.pop(&mut out), 3);
        assert_eq!(out, [1.0, 2.0, 3.0]);
        assert!(rx.is_empty());
    }

    #[test]
    fn a_full_ring_accepts_a_short_write_rather_than_overwriting() {
        let (mut tx, mut rx) = ring(4);
        assert_eq!(tx.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]), 4);
        assert_eq!(tx.free(), 0);
        assert_eq!(tx.push(&[9.0]), 0);

        let mut out = [0.0; 4];
        rx.pop(&mut out);
        assert_eq!(out, [1.0, 2.0, 3.0, 4.0], "the tail was dropped, not the head");
    }

    #[test]
    fn samples_survive_the_wrap_point() {
        let (mut tx, mut rx) = ring(8);
        let mut out = [0.0; 5];

        // Walk the write cursor several times around a ring that does not
        // divide evenly into the block size, which is where an index that
        // wraps wrongly shows up.
        let mut expected = 0.0f32;
        for round in 0..7 {
            let block: Vec<f32> = (0..5).map(|i| (round * 5 + i) as f32).collect();
            assert_eq!(tx.push(&block), 5, "round {round}");
            assert_eq!(rx.pop(&mut out), 5);
            for value in out {
                assert_eq!(value, expected);
                expected += 1.0;
            }
        }
    }

    #[test]
    fn a_split_read_reassembles_across_the_seam() {
        let (mut tx, mut rx) = ring(8);
        // Park the cursors near the end of the allocation.
        tx.push(&[0.0; 6]);
        let mut sink = [0.0; 6];
        rx.pop(&mut sink);

        tx.push(&[1.0, 2.0, 3.0, 4.0]);
        let mut out = [0.0; 4];
        assert_eq!(rx.pop(&mut out), 4);
        assert_eq!(out, [1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn an_underrun_reports_what_was_missing_and_leaves_the_rest_alone() {
        let (mut tx, mut rx) = ring(16);
        tx.push(&[1.0, 2.0]);

        let mut out = [-1.0; 5];
        assert_eq!(rx.pop(&mut out), 2);
        assert_eq!(out[0], 1.0);
        assert_eq!(out[1], 2.0);
        assert_eq!(&out[2..], &[-1.0, -1.0, -1.0], "the tail is the caller's job");
        assert_eq!(rx.starved(), 3);

        // And an empty ring is not an error, it is zero samples.
        assert_eq!(rx.pop(&mut out), 0);
    }

    #[test]
    fn a_flush_drops_everything_queued() {
        let (mut tx, mut rx) = ring(16);
        tx.push(&[1.0; 10]);
        tx.flush();

        let mut out = [0.0; 4];
        assert_eq!(rx.pop(&mut out), 0, "stale audio must not be heard after a seek");

        // And the room comes back once the consumer has acted on the flush.
        assert_eq!(tx.free(), 16);
        assert_eq!(tx.push(&[2.0; 4]), 4);
        assert_eq!(rx.pop(&mut out), 4);
        assert_eq!(out, [2.0; 4]);
    }

    #[test]
    fn a_flush_the_consumer_has_not_seen_yet_does_not_free_space() {
        // The producer must not reclaim slots on its own say-so: until the
        // callback has moved its cursor, those samples are still live.
        let (mut tx, mut rx) = ring(8);
        tx.push(&[1.0; 8]);
        tx.flush();
        assert_eq!(tx.free(), 0, "the read cursor has not moved yet");

        let mut out = [0.0; 1];
        rx.pop(&mut out);
        assert_eq!(tx.free(), 8);
    }

    #[test]
    fn repeated_flushes_are_idempotent() {
        let (mut tx, mut rx) = ring(8);
        tx.push(&[1.0; 4]);
        tx.flush();
        tx.flush();
        tx.flush();

        let mut out = [0.0; 4];
        assert_eq!(rx.pop(&mut out), 0);
        tx.push(&[7.0; 2]);
        assert_eq!(rx.pop(&mut out), 2);
        assert_eq!(out[0], 7.0);
    }

    #[test]
    fn a_producer_and_a_consumer_on_two_threads_agree_on_every_sample() {
        // The ordering is the whole point of the type; a single-threaded test
        // cannot fail on it.
        const TOTAL: usize = 200_000;
        let (mut tx, mut rx) = ring(1024);

        let writer = std::thread::spawn(move || {
            let mut sent = 0usize;
            while sent < TOTAL {
                let block: Vec<f32> = (sent..(sent + 97).min(TOTAL)).map(|i| i as f32).collect();
                let mut offset = 0;
                while offset < block.len() {
                    offset += tx.push(&block[offset..]);
                    std::hint::spin_loop();
                }
                sent += block.len();
            }
        });

        let mut received = 0usize;
        let mut out = [0.0f32; 64];
        while received < TOTAL {
            let count = rx.pop(&mut out);
            for value in out.iter().take(count) {
                assert_eq!(*value, received as f32, "sample {received} arrived out of order");
                received += 1;
            }
            if count == 0 {
                std::hint::spin_loop();
            }
        }
        writer.join().unwrap();
    }
}
