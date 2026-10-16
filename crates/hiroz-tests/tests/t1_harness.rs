//! The counter of the T1 harness needs no ROS: it reads a payload sequence number and a sentinel.

#[path = "common/t1_harness.rs"]
mod t1_harness;

use t1_harness::{SENTINEL, SeqCounter, seq_payload};

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

/// A reader that hands out one byte vector in fixed small pieces, so a marker can straddle reads.
struct Pieces<'a>(&'a [u8], usize);

impl std::io::Read for Pieces<'_> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = self.0.len().min(self.1).min(out.len());
        out[..n].copy_from_slice(&self.0[..n]);
        self.0 = &self.0[n..];
        Ok(n)
    }
}

#[test]
fn scan_finds_a_marker_that_straddles_two_chunks() {
    let mut data = vec![b'x'; (1 << 20) - 3];
    data.extend_from_slice(b"_buf_cpu");
    data.extend_from_slice(&[b'y'; 100]);
    assert!(t1_harness::stream_contains(&data[..], b"_buf_cpu", u64::MAX).unwrap());
    assert!(t1_harness::stream_contains(Pieces(b"aa_buf_cpu", 3), b"_buf_cpu", u64::MAX).unwrap());
    assert!(!t1_harness::stream_contains(Pieces(b"aa_buf_cp", 2), b"_buf_cpu", u64::MAX).unwrap());
}

#[test]
fn scan_is_bounded_by_its_limit() {
    let data = vec![b'x'; 4 << 20];
    assert!(t1_harness::stream_contains(&data[..], b"_buf_cpu", 2 << 20).is_err());
    assert!(!t1_harness::stream_contains(&data[..], b"_buf_cpu", u64::MAX).unwrap());
}

#[test]
fn guard_names_each_reason_and_accepts_a_buffer_aware_library() {
    use t1_harness::RmwGuardError;
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("rmw_guard");
    std::fs::create_dir_all(&dir).unwrap();

    assert_eq!(t1_harness::find_rmw_library(&[dir.join("absent")]), None);
    assert_eq!(
        RmwGuardError::NotFound.to_string(),
        "could not find librmw_zenoh_cpp.so"
    );

    let lib = dir.join("librmw_zenoh_cpp.so");
    std::fs::write(&lib, b"\x7fELF backends only").unwrap();
    assert_eq!(
        t1_harness::find_rmw_library(std::slice::from_ref(&dir)),
        Some(lib.clone())
    );
    let err = t1_harness::check_rmw_library(&lib).unwrap_err();
    let text = err.to_string();
    assert!(
        text.starts_with("the rmw_zenoh_cpp in this environment is not buffer-aware (")
            && text.contains("version not found")
            && text.ends_with("the released 0.10.6 or later"),
        "{text}"
    );

    std::fs::write(&lib, b"\x7fELF xx_buf_cpu xx").unwrap();
    assert_eq!(t1_harness::check_rmw_library(&lib), Ok(()));

    let missing = t1_harness::check_rmw_library(&dir.join("gone.so")).unwrap_err();
    assert!(
        matches!(missing, RmwGuardError::Unreadable { .. }),
        "{missing:?}"
    );
}
