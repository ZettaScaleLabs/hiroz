//! The counter of the T1 harness needs no ROS: it reads a payload sequence number and a sentinel.

mod common;

use common::t1_harness::{SENTINEL, SeqCounter, seq_payload};

#[test]
fn seq_counter_counts_by_sequence_and_rejects_a_bad_sentinel() {
    let mut counter = SeqCounter::new();
    for seq in [0u32, 1, 2, 4] {
        assert!(counter.record(&seq_payload(seq, 16)), "new message {seq}");
    }
    assert!(!counter.record(&seq_payload(2, 16)), "a repeat is not new");
    let mut truncated = seq_payload(3, 16);
    truncated.pop();
    assert!(
        !counter.record(&truncated),
        "a truncated payload is corrupt"
    );
    let mut bad = seq_payload(3, 16);
    *bad.last_mut().unwrap() = SENTINEL ^ 1;
    assert!(!counter.record(&bad), "a wrong sentinel is corrupt");

    assert_eq!(
        counter.received_of(5),
        4,
        "0, 1, 2 and 4 arrived, 3 did not"
    );
    assert_eq!(counter.duplicates(), 1);
    assert_eq!(counter.corrupt(), 2);
    assert_eq!(
        counter.received_of(3),
        3,
        "a sequence beyond the range is not counted"
    );
}
