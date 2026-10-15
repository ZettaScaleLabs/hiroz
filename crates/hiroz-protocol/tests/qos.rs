//! `QosProfile::decode`/`encode` against `rmw_zenoh_cpp`'s compact wire
//! format, including the SYSTEM_DEFAULT omission rmw_zenoh_cpp uses for QoS
//! sub-fields it did not set explicitly.

use hiroz_protocol::qos::{
    QosDecodeError, QosDurability, QosDuration, QosHistory, QosLiveliness, QosProfile,
    QosReliability, RMW_ZENOH_DEFAULT_HISTORY_DEPTH,
};

#[test]
fn decode_rmw_compact_qos_corpus() {
    // An omitted or zero-valued depth decodes to rmw_zenoh_cpp's own wire
    // default (42), not hiroz's unrelated built-in default (`QosProfile::
    // default()`'s depth of 10) -- see `RMW_ZENOH_DEFAULT_HISTORY_DEPTH`.
    let wire_default = QosProfile {
        history: QosHistory::KeepLast(RMW_ZENOH_DEFAULT_HISTORY_DEPTH),
        ..QosProfile::default()
    };

    let cases = [
        ("::,:,:,:,,", wire_default),
        (":::,:,:,,", wire_default),
        ("::1,:,:,:,,", wire_default),
        ("::,10:,:,:,,", QosProfile::default()),
        (
            "::2,:,:,:,,",
            QosProfile {
                history: QosHistory::KeepAll,
                ..QosProfile::default()
            },
        ),
        (
            "1:1:,5:,:,:,,",
            QosProfile {
                durability: QosDurability::TransientLocal,
                history: QosHistory::KeepLast(5),
                ..QosProfile::default()
            },
        ),
        (
            "2::,1:,:,:,,",
            QosProfile {
                reliability: QosReliability::BestEffort,
                history: QosHistory::KeepLast(1),
                ..QosProfile::default()
            },
        ),
        ("0:0:0,0:,:,:,,", wire_default),
    ];

    for (encoded, expected) in cases {
        assert_eq!(QosProfile::decode(encoded), Ok(expected), "{encoded}");
    }
}

#[test]
fn qos_round_trip() {
    let profiles = [
        QosProfile::default(),
        QosProfile {
            reliability: QosReliability::BestEffort,
            durability: QosDurability::TransientLocal,
            history: QosHistory::KeepLast(5),
            ..QosProfile::default()
        },
        QosProfile {
            history: QosHistory::KeepAll,
            ..QosProfile::default()
        },
        QosProfile {
            deadline: QosDuration {
                sec: 1,
                nsec: 500_000,
            },
            lifespan: QosDuration { sec: 2, nsec: 0 },
            liveliness: QosLiveliness::ManualByTopic,
            liveliness_lease_duration: QosDuration { sec: 3, nsec: 250 },
            ..QosProfile::default()
        },
        // Every field non-default at once.
        QosProfile {
            reliability: QosReliability::BestEffort,
            durability: QosDurability::TransientLocal,
            history: QosHistory::KeepAll,
            deadline: QosDuration { sec: 10, nsec: 1 },
            lifespan: QosDuration { sec: 20, nsec: 2 },
            liveliness: QosLiveliness::ManualByNode,
            liveliness_lease_duration: QosDuration { sec: 30, nsec: 3 },
        },
    ];

    for profile in profiles {
        assert_eq!(QosProfile::decode(&profile.encode()), Ok(profile));
    }
}

#[test]
fn reject_invalid_history() {
    for encoded in ["::3,1:,:,:,,", "::1,-1:,:,:,,", "::"] {
        assert_eq!(
            QosProfile::decode(encoded),
            Err(QosDecodeError::InvalidHistory),
            "{encoded}"
        );
    }
}

#[test]
fn reject_invalid_duration_and_liveliness() {
    assert_eq!(
        QosProfile::decode("::,:notanumber,:,,"),
        Err(QosDecodeError::InvalidDuration),
    );
    assert_eq!(
        QosProfile::decode("::,:,:,:9,,"),
        Err(QosDecodeError::InvalidLiveliness),
    );
}

// ---------------------------------------------------------------------------
// Conformance with the chunk rmw_zenoh_cpp writes. The expected text comes
// from fixtures captured from the real C++ code (see
// `fixtures/rmw_zenoh_qos/README.md`); nothing here recomputes it.
// ---------------------------------------------------------------------------

const GRID: &str = include_str!("fixtures/rmw_zenoh_qos/qos_grid.tsv");
const DEPTHS: &str = include_str!("fixtures/rmw_zenoh_qos/qos_depths.tsv");

/// One fixture row: the rmw integers the C++ writer was given, and the chunk it wrote.
struct Row {
    rel: u32,
    dur: u32,
    hist: u32,
    depth: usize,
    deadline: (u64, u64),
    lifespan: (u64, u64),
    lv: u32,
    lease: (u64, u64),
    chunk: &'static str,
}

fn rows(fixture: &'static str) -> Vec<Row> {
    fixture
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|l| {
            let (fields, chunk) = l.split_once('\t').expect("fixture row has a tab");
            let n: Vec<u64> = fields
                .split(' ')
                .map(|v| v.parse().expect("fixture field is an integer"))
                .collect();
            assert_eq!(n.len(), 11, "{l}");
            Row {
                rel: n[0] as u32,
                dur: n[1] as u32,
                hist: n[2] as u32,
                depth: n[3] as usize,
                deadline: (n[4], n[5]),
                lifespan: (n[6], n[7]),
                lv: n[8] as u32,
                lease: (n[9], n[10]),
                chunk,
            }
        })
        .collect()
}

fn profile_of(r: &Row) -> QosProfile {
    let dur = |(sec, nsec)| QosDuration { sec, nsec };
    QosProfile {
        reliability: match r.rel {
            1 => QosReliability::Reliable,
            2 => QosReliability::BestEffort,
            v => panic!("reliability {v}"),
        },
        durability: match r.dur {
            1 => QosDurability::TransientLocal,
            2 => QosDurability::Volatile,
            v => panic!("durability {v}"),
        },
        history: match r.hist {
            1 => QosHistory::KeepLast(r.depth),
            2 => QosHistory::KeepAll,
            v => panic!("history {v}"),
        },
        deadline: dur(r.deadline),
        lifespan: dur(r.lifespan),
        liveliness: match r.lv {
            1 => QosLiveliness::Automatic,
            3 => QosLiveliness::ManualByTopic,
            v => panic!("liveliness {v}"),
        },
        liveliness_lease_duration: dur(r.lease),
    }
}

/// `(row number, got, expected)` for every row whose chunk differs.
fn chunk_mismatches(rows: &[Row]) -> Vec<(usize, String, &'static str)> {
    rows.iter()
        .enumerate()
        .filter_map(|(i, r)| {
            let got = profile_of(r).encode();
            (got != r.chunk).then_some((i + 1, got, r.chunk))
        })
        .collect()
}

#[test]
fn qos_chunk_equals_rmw_zenoh_for_every_grid_profile() {
    let grid = rows(GRID);
    assert_eq!(grid.len(), 576, "fixture is the whole grid");
    let bad = chunk_mismatches(&grid);
    assert!(
        bad.is_empty(),
        "{} of {} rows differ from rmw_zenoh; first 3 (row, hiroz, rmw_zenoh): {:?}",
        bad.len(),
        grid.len(),
        &bad[..bad.len().min(3)]
    );
}

/// Control: the rows hiroz already encoded correctly (depth 10, the hiroz
/// default, and keep-all) stay correct. Counting them keeps the check from
/// passing over an empty set.
#[test]
fn grid_rows_that_matched_before_the_fix_still_match() {
    let kept: Vec<Row> = rows(GRID)
        .into_iter()
        .filter(|r| r.hist == 2 || r.depth == 10)
        .collect();
    assert_eq!(kept.len(), 192, "96 depth-10 rows and 96 keep-all rows");
    let bad = chunk_mismatches(&kept);
    assert!(
        bad.is_empty(),
        "{} rows regressed: {:?}",
        bad.len(),
        &bad[..bad.len().min(3)]
    );
}

/// Control: the rule "omit the kind, always write the depth" is not enough.
/// rmw_zenoh also omits the depth at 42, so this model of the rejected rule
/// must differ from the writer on every depth-42 keep-last row (96 of 576).
/// If this fails, the fixture lost its depth-42 rows and the pin in
/// `qos_chunk_equals_rmw_zenoh_for_every_grid_profile` no longer proves it.
#[test]
fn always_writing_the_depth_differs_from_rmw_zenoh_on_the_depth_42_rows() {
    let differs: Vec<usize> = rows(GRID)
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            let history_field = match r.hist {
                1 => format!(",{}", r.depth),
                _ => "2,".to_string(),
            };
            r.chunk.split(':').nth(2) != Some(history_field.as_str())
        })
        .map(|(i, _)| i + 1)
        .collect();
    assert_eq!(differs.len(), 96);
    assert!(differs
        .iter()
        .all(|&n| rows(GRID)[n - 1].hist == 1 && rows(GRID)[n - 1].depth == 42));
}

const DECODE: &str = include_str!("fixtures/rmw_zenoh_qos/qos_decode.tsv");

/// When hiroz decodes a chunk that rmw_zenoh parses, the fields equal the ones
/// rmw_zenoh returns.
///
/// The fixture also holds the chunk `:::,:,:,,`, which rmw_zenoh rejects
/// (`NULL`) and hiroz accepts. hiroz stays lenient on purpose, so that row is
/// not compared and no rejection is asserted.
#[test]
fn qos_chunk_decodes_like_rmw_zenoh() {
    let mut compared = 0;
    for line in DECODE.lines() {
        let (chunk, fields) = line.split_once('\t').expect("fixture row has a tab");
        if fields == "NULL" {
            continue;
        }
        let row_text = format!("{fields}\t{chunk}");
        let row = rows(Box::leak(row_text.into_boxed_str()))
            .pop()
            .expect("one row");
        assert_eq!(
            QosProfile::decode(chunk),
            Ok(profile_of(&row)),
            "chunk {chunk:?}, rmw_zenoh fields {fields:?}"
        );
        compared += 1;
    }
    assert_eq!(compared, 7, "8 fixture chunks, 1 rejected by rmw_zenoh");
}

/// When hiroz encodes depth 0 to 100 for each history kind, each chunk equals
/// the writer's.
///
/// hiroz `KeepAll` carries no depth, but rmw_zenoh writes `2,` for depth 42
/// and `2,0` for depth 0. One text cannot match both, so for keep-all the
/// text is compared on the depth-42 row (in the grid) and here the chunk must
/// decode back to `KeepAll`.
#[test]
fn qos_chunk_equals_rmw_zenoh_for_depths_0_to_100() {
    let depths = rows(DEPTHS);
    assert_eq!(
        depths.len(),
        408,
        "4 reliability/durability pairs, 102 rows each"
    );

    let keep_last: Vec<Row> = depths.into_iter().filter(|r| r.hist == 1).collect();
    assert_eq!(keep_last.len(), 404, "depths 0 to 100, 4 pairs");
    let bad = chunk_mismatches(&keep_last);
    assert!(
        bad.is_empty(),
        "{} of {} keep-last rows differ from rmw_zenoh; first 3 (row, hiroz, rmw_zenoh): {:?}",
        bad.len(),
        keep_last.len(),
        &bad[..bad.len().min(3)]
    );

    let keep_all: Vec<Row> = rows(DEPTHS).into_iter().filter(|r| r.hist == 2).collect();
    assert_eq!(keep_all.len(), 4, "keep-all with depth 0, 4 pairs");
    for r in &keep_all {
        assert_eq!(
            QosProfile::decode(r.chunk),
            Ok(profile_of(r)),
            "keep-all chunk {:?} does not round-trip",
            r.chunk
        );
    }
}
