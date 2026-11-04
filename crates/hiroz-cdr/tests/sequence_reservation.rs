//! How much a sequence decoder reserves before it has decoded the elements, and where
//! the declared count meets the unread input.
//!
//! A counting global allocator records the allocations of the decoding thread while a
//! measurement is armed, and the size of the largest request. Other threads of the test binary
//! do not count.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

use byteorder::LittleEndian;
use hiroz_cdr::{CdrDeserialize, CdrReader};

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    /// Allocations and reallocations of the armed thread.
    static COUNTS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
    /// Bytes of the largest single request of the armed thread.
    static LARGEST: Cell<usize> = const { Cell::new(0) };
}

struct Counting;

fn record(realloc: bool, bytes: usize) {
    // The cells are const-initialized and have no destructor, so this does not allocate.
    if ARMED.try_with(Cell::get).unwrap_or(false) {
        let _ = COUNTS.try_with(|c| {
            let (a, r) = c.get();
            c.set(if realloc { (a, r + 1) } else { (a + 1, r) });
        });
        let _ = LARGEST.try_with(|l| l.set(l.get().max(bytes)));
    }
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(false, layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record(true, new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// What the armed thread requested from the allocator.
#[derive(Debug)]
struct Stats {
    allocs: usize,
    reallocs: usize,
    /// Bytes of the largest single request.
    largest: usize,
}

/// Run `f` and return its result with the requests this thread made.
fn measure<R>(f: impl FnOnce() -> R) -> (R, Stats) {
    COUNTS.with(|c| c.set((0, 0)));
    LARGEST.with(|l| l.set(0));
    ARMED.with(|a| a.set(true));
    let r = f();
    ARMED.with(|a| a.set(false));
    let (allocs, reallocs) = COUNTS.with(Cell::get);
    let largest = LARGEST.with(Cell::get);
    (
        r,
        Stats {
            allocs,
            reallocs,
            largest,
        },
    )
}

/// A body for `Vec<u64>`: the declared count, alignment padding, then `elements` whole elements.
fn u64_body(declared: u32, elements: u32) -> Vec<u8> {
    let mut v = declared.to_le_bytes().to_vec();
    v.extend_from_slice(&[0; 4]);
    v.resize(v.len() + elements as usize * 8, 0);
    v
}

/// Positive control: the counter sees a reservation made directly.
#[test]
fn counter_sees_a_direct_reservation() {
    let (v, stats) = measure(|| Vec::<u64>::with_capacity(1000));
    assert_eq!(
        stats.allocs, 1,
        "counter_sees_a_direct_reservation: allocations"
    );
    assert_eq!(
        stats.largest, 8000,
        "counter_sees_a_direct_reservation: largest request"
    );
    drop(v);
}

/// An honest sequence reserves its full count at once and never reallocates.
#[test]
fn honest_sequence_reserves_once() {
    const N: u32 = 1000;
    let input = u64_body(N, N);
    let (decoded, stats) = measure(|| {
        let mut r = CdrReader::<LittleEndian>::new(&input);
        Vec::<u64>::cdr_deserialize(&mut r)
    });
    assert_eq!(
        decoded
            .expect("honest_sequence_reserves_once: decode")
            .len(),
        N as usize
    );
    assert_eq!(
        stats.reallocs, 0,
        "honest_sequence_reserves_once: reallocations"
    );
    assert_eq!(
        stats.allocs, 1,
        "honest_sequence_reserves_once: allocations"
    );
    assert_eq!(
        stats.largest,
        N as usize * size_of::<u64>(),
        "honest_sequence_reserves_once: reserved bytes"
    );
}

/// A count above the unread input reserves at most one element per unread byte.
#[test]
fn truncated_sequence_reserves_at_most_the_unread_input() {
    const NAME: &str = "truncated_sequence_reserves_at_most_the_unread_input";
    // The count is declared as 100_000; two whole elements follow the alignment padding.
    let input = u64_body(100_000, 2);
    let unread = input.len() - 4;
    let (decoded, stats) = measure(|| {
        let mut r = CdrReader::<LittleEndian>::new(&input);
        Vec::<u64>::cdr_deserialize(&mut r)
    });
    assert!(decoded.is_err(), "{NAME}: decode must fail");
    assert!(
        stats.allocs >= 1,
        "{NAME}: the decoder must reserve, or the bound is not measured: {stats:?}"
    );
    assert!(
        stats.largest <= unread * size_of::<u64>(),
        "{NAME}: reserved {} bytes for {unread} unread bytes",
        stats.largest
    );
}

/// A count equal to the unread bytes decodes (one byte per element); one more is an error.
#[test]
fn typed_decoder_accepts_a_count_at_the_boundary() {
    let decode = |count: u32, payload: usize| {
        let mut body = count.to_le_bytes().to_vec();
        body.extend(std::iter::repeat_n(7u8, payload));
        Vec::<u8>::cdr_deserialize(&mut CdrReader::<LittleEndian>::new(&body))
    };
    const NAME: &str = "typed_decoder_accepts_a_count_at_the_boundary";
    assert_eq!(
        decode(4, 4).ok(),
        Some(vec![7; 4]),
        "{NAME}: count equals unread bytes"
    );
    assert!(decode(5, 4).is_err(), "{NAME}: count above unread bytes");
}

/// An element that takes no bytes on the wire and is not a serde type.
#[derive(Debug, PartialEq)]
struct ZeroBytes;

impl CdrDeserialize for ZeroBytes {
    fn cdr_deserialize<'de, BO: byteorder::ByteOrder>(
        _reader: &mut CdrReader<'de, BO>,
    ) -> hiroz_cdr::Result<Self> {
        Ok(Self)
    }
}

/// The generic `Vec<T>` decoder accepts a count above the unread bytes when elements take none.
#[test]
fn generic_vec_decodes_zero_byte_elements() {
    let body = 3u32.to_le_bytes();
    let mut r = CdrReader::<LittleEndian>::new(&body);
    let v = Vec::<ZeroBytes>::cdr_deserialize(&mut r).expect("zero-byte elements must decode");
    assert_eq!(v, [ZeroBytes, ZeroBytes, ZeroBytes]);
}
