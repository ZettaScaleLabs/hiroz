//! How much a sequence decoder reserves before it has decoded the elements.
//!
//! A counting global allocator records the allocations of the decoding thread
//! while a measurement is armed. Other threads of the test binary do not count.
//!
//! The decoder may reserve the declared count only as far as the unread bytes
//! allow, and an honest sequence must not grow by reallocation.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

use byteorder::LittleEndian;
use hiroz_cdr::{CdrDeserialize, CdrReader};

#[derive(Clone, Copy, Default)]
struct Counts {
    allocs: usize,
    reallocs: usize,
    largest: usize,
}

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static COUNTS: Cell<Counts> = const { Cell::new(Counts { allocs: 0, reallocs: 0, largest: 0 }) };
}

fn record(size: usize, realloc: bool) {
    // Both cells are const-initialized and have no destructor, so this does not allocate.
    if ARMED.try_with(Cell::get).unwrap_or(false) {
        let _ = COUNTS.try_with(|c| {
            let mut v = c.get();
            if realloc {
                v.reallocs += 1;
            } else {
                v.allocs += 1;
            }
            v.largest = v.largest.max(size);
            c.set(v);
        });
    }
}

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size(), false);
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size(), false);
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record(new_size, true);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// Run `f` and return its result with the allocations made by this thread during it.
fn measure<R>(f: impl FnOnce() -> R) -> (R, Counts) {
    COUNTS.with(|c| c.set(Counts::default()));
    ARMED.with(|a| a.set(true));
    let r = f();
    ARMED.with(|a| a.set(false));
    (r, COUNTS.with(Cell::get))
}

/// One element of 32 bytes on the wire and in memory, with no heap data of its own.
#[derive(Debug)]
struct Big([u64; 4]);

impl CdrDeserialize for Big {
    fn cdr_deserialize<'de, BO: byteorder::ByteOrder>(
        reader: &mut CdrReader<'de, BO>,
    ) -> hiroz_cdr::Result<Self> {
        Ok(Self([
            reader.read_u64()?,
            reader.read_u64()?,
            reader.read_u64()?,
            reader.read_u64()?,
        ]))
    }
}

/// A body for `Vec<Big>`: the declared count, then `elements` whole elements.
fn body(declared: u32, elements: u32) -> Vec<u8> {
    let mut v = declared.to_le_bytes().to_vec();
    v.extend_from_slice(&[0; 4]); // `u64` elements start on an 8-byte boundary
    v.resize(v.len() + elements as usize * size_of::<Big>(), 0);
    v
}

/// Positive control: the counter sees a reservation made directly.
#[test]
fn counter_sees_a_direct_reservation() {
    let (v, counts) = measure(|| Vec::<Big>::with_capacity(100_000));
    assert_eq!(
        counts.allocs, 1,
        "counter_sees_a_direct_reservation: allocations"
    );
    assert_eq!(
        counts.largest,
        100_000 * size_of::<Big>(),
        "counter_sees_a_direct_reservation: largest request"
    );
    drop(v);
}

/// An honest sequence reserves its full count at once and never reallocates.
#[test]
fn honest_sequence_reserves_once() {
    const N: u32 = 1000;
    let input = body(N, N);
    let (decoded, counts) = measure(|| {
        let mut r = CdrReader::<LittleEndian>::new(&input);
        Vec::<Big>::cdr_deserialize(&mut r)
    });
    let decoded = decoded.expect("honest_sequence_reserves_once: decode");
    assert_eq!(decoded.len(), N as usize);
    assert_eq!(
        decoded[0].0, [0; 4],
        "honest_sequence_reserves_once: first element"
    );
    assert_eq!(
        counts.reallocs, 0,
        "honest_sequence_reserves_once: reallocations while decoding {N} elements"
    );
    assert_eq!(
        counts.allocs, 1,
        "honest_sequence_reserves_once: allocations while decoding {N} elements"
    );
}

/// A huge declared count with a short body reserves at most one element per unread byte.
#[test]
fn hostile_count_reserves_at_most_the_body_size() {
    const DECLARED: u32 = 99_999_999;
    const ELEMENTS: u32 = 100;
    let input = body(DECLARED, ELEMENTS);
    let unread = input.len() - size_of::<u32>();
    let (decoded, counts) = measure(|| {
        let mut r = CdrReader::<LittleEndian>::new(&input);
        Vec::<Big>::cdr_deserialize(&mut r)
    });
    assert!(
        decoded.is_err(),
        "hostile_count_reserves_at_most_the_body_size: the body is short, decoding must fail"
    );
    assert!(
        counts.largest <= unread * size_of::<Big>(),
        "hostile_count_reserves_at_most_the_body_size: largest request {} bytes, bound {} bytes",
        counts.largest,
        unread * size_of::<Big>()
    );
    assert!(
        counts.largest < DECLARED as usize * size_of::<Big>() / 1000,
        "hostile_count_reserves_at_most_the_body_size: the request follows the declared count"
    );
}
