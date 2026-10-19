//! How much a sequence decoder reserves before it has decoded the elements, and where
//! the declared count meets the unread input.
//!
//! A counting global allocator records the allocations of the decoding thread while a
//! measurement is armed. Other threads of the test binary do not count.

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
}

struct Counting;

fn record(realloc: bool) {
    // Both cells are const-initialized and have no destructor, so this does not allocate.
    if ARMED.try_with(Cell::get).unwrap_or(false) {
        let _ = COUNTS.try_with(|c| {
            let (a, r) = c.get();
            c.set(if realloc { (a, r + 1) } else { (a + 1, r) });
        });
    }
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(false);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record(true);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// Run `f` and return its result with the (allocations, reallocations) this thread made.
fn measure<R>(f: impl FnOnce() -> R) -> (R, (usize, usize)) {
    COUNTS.with(|c| c.set((0, 0)));
    ARMED.with(|a| a.set(true));
    let r = f();
    ARMED.with(|a| a.set(false));
    (r, COUNTS.with(Cell::get))
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
    let (v, (allocs, _)) = measure(|| Vec::<u64>::with_capacity(1000));
    assert_eq!(allocs, 1, "counter_sees_a_direct_reservation: allocations");
    drop(v);
}

/// An honest sequence reserves its full count at once and never reallocates.
#[test]
fn honest_sequence_reserves_once() {
    const N: u32 = 1000;
    let input = u64_body(N, N);
    let (decoded, (allocs, reallocs)) = measure(|| {
        let mut r = CdrReader::<LittleEndian>::new(&input);
        Vec::<u64>::cdr_deserialize(&mut r)
    });
    assert_eq!(
        decoded
            .expect("honest_sequence_reserves_once: decode")
            .len(),
        N as usize
    );
    assert_eq!(reallocs, 0, "honest_sequence_reserves_once: reallocations");
    assert_eq!(allocs, 1, "honest_sequence_reserves_once: allocations");
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
