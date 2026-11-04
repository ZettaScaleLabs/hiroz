//! The decision of the allocator pin, driven with an injected `mallopt`.
//!
//! A glibc that rejects the mmap threshold is not available in CI, so the test replaces the call.
//! A rejected mmap threshold must leave the earlier allocator settings unchanged: the trim call
//! must not run, and a `trim_rc` of 0 means the call was not attempted.

use hiroz::malloc_pin::{MMAP_THRESHOLD, PinOutcome, TRIM_THRESHOLD, pin_with};

const M_TRIM_THRESHOLD: i32 = -1;
const M_MMAP_THRESHOLD: i32 = -3;

#[test]
fn accepted_thresholds_set_mmap_first_then_trim() {
    let mut calls = vec![];
    let out = pin_with(|p, v| {
        calls.push((p, v));
        1
    });
    assert_eq!(
        out,
        PinOutcome::Pinned {
            mmap_rc: 1,
            trim_rc: 1
        }
    );
    assert_eq!(
        calls,
        vec![
            (M_MMAP_THRESHOLD, MMAP_THRESHOLD),
            (M_TRIM_THRESHOLD, TRIM_THRESHOLD)
        ]
    );
}

#[test]
fn rejected_mmap_threshold_skips_the_trim_call() {
    let mut calls = vec![];
    let out = pin_with(|p, v| {
        calls.push((p, v));
        i32::from(p != M_MMAP_THRESHOLD)
    });
    assert_eq!(
        out,
        PinOutcome::Rejected {
            mmap_rc: 0,
            trim_rc: 0
        }
    );
    assert_eq!(calls, vec![(M_MMAP_THRESHOLD, MMAP_THRESHOLD)]);
}

#[test]
fn rejected_trim_threshold_is_reported() {
    let out = pin_with(|p, _| i32::from(p != M_TRIM_THRESHOLD));
    assert_eq!(
        out,
        PinOutcome::Rejected {
            mmap_rc: 1,
            trim_rc: 0
        }
    );
}

#[test]
fn rejected_mmap_threshold_is_described_without_a_trim_return_code() {
    let text = PinOutcome::Rejected {
        mmap_rc: 0,
        trim_rc: 0,
    }
    .to_string();
    assert!(text.contains("trim not attempted"), "{text}");
    assert!(!text.contains("trim=0"), "{text}");
}

#[test]
fn rejected_trim_threshold_is_described_with_its_return_code() {
    // Positive control of the test above: a trim call that ran and failed is reported as such.
    let text = PinOutcome::Rejected {
        mmap_rc: 1,
        trim_rc: 0,
    }
    .to_string();
    assert!(text.contains("trim=0"), "{text}");
    assert!(!text.contains("not attempted"), "{text}");
}
