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
        "R1ASSERT qos_chunk_equals_rmw_zenoh_for_every_grid_profile: {} of {} rows differ from rmw_zenoh; first 3 (row, hiroz, rmw_zenoh): {:?}",
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
/// rmw_zenoh also omits the depth at 42, so on the 96 depth-42 keep-last rows
/// of the grid the history field is `,` and not `,42`. The fixture must keep
/// those rows, and the encoder must write `,` on them. An encoder that always
/// writes the depth writes `,42` and fails here, as it fails the grid pin.
#[test]
fn always_writing_the_depth_differs_from_rmw_zenoh_on_the_depth_42_rows() {
    let history_field = |chunk: &str| chunk.split(':').nth(2).map(str::to_string);
    let grid = rows(GRID);
    let depth_42: Vec<&Row> = grid
        .iter()
        .filter(|r| r.hist == 1 && r.depth == 42)
        .collect();
    assert_eq!(
        depth_42.len(),
        96,
        "the depth-42 keep-last rows of the grid"
    );
    for r in depth_42 {
        let always_depth = Some(",42".to_string());
        assert_ne!(
            history_field(r.chunk),
            always_depth,
            "fixture row {:?} lost its depth omission",
            r.chunk
        );
        assert_eq!(
            history_field(&profile_of(r).encode()),
            history_field(r.chunk),
            "encoder history field on depth-42 row {:?}",
            r.chunk
        );
    }
}

const DECODE: &str = include_str!("fixtures/rmw_zenoh_qos/qos_decode.tsv");

/// When hiroz decodes a chunk that rmw_zenoh parses, the fields equal the ones
/// rmw_zenoh returns.
///
/// The fixture also holds chunks that rmw_zenoh rejects (`NULL`): `:::,:,:,,`
/// and the liveliness key 2. hiroz accepts the first on purpose, so no
/// rejection is asserted and those rows are not compared. The rejected set
/// is pinned by name, so a new `NULL` row needs a decision.
#[test]
fn qos_chunk_decodes_like_rmw_zenoh() {
    let mut compared = 0;
    let mut rejected = Vec::new();
    for line in DECODE.lines() {
        let (chunk, fields) = line.split_once('\t').expect("fixture row has a tab");
        if fields == "NULL" {
            rejected.push(chunk);
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
    assert_eq!(compared, 17, "19 fixture chunks, 2 rejected by rmw_zenoh");
    assert_eq!(rejected, [":::,:,:,,", "::,:,:,:2,,"]);
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
        "R1ASSERT qos_chunk_equals_rmw_zenoh_for_depths_0_to_100: {} of {} keep-last rows differ from rmw_zenoh; first 3 (row, hiroz, rmw_zenoh): {:?}",
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

/// `ManualByNode` is a hiroz-only liveliness value. The encoder writes key 2,
/// and rmw_zenoh_cpp has no such key: its parser rejects the whole chunk
/// (`NULL` in the decode fixture), so a peer running rmw_zenoh_cpp ignores the
/// entity. This test pins that known limitation; it is not a conformance claim.
#[test]
fn manual_by_node_is_written_as_a_chunk_rmw_zenoh_rejects() {
    let chunk = QosProfile {
        history: QosHistory::KeepLast(RMW_ZENOH_DEFAULT_HISTORY_DEPTH),
        liveliness: QosLiveliness::ManualByNode,
        ..QosProfile::default()
    }
    .encode();
    assert_eq!(chunk, "::,:,:,:2,,");
    let rejected: Vec<&str> = DECODE
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .filter(|(_, fields)| *fields == "NULL")
        .map(|(chunk, _)| chunk)
        .collect();
    assert!(
        rejected.contains(&chunk.as_str()),
        "rmw_zenoh no longer rejects {chunk:?}; rejected chunks: {rejected:?}"
    );
}

const DURATIONS: &str = include_str!("fixtures/rmw_zenoh_qos/qos_durations.tsv");

/// When exactly one component of a duration is at its default, the chunk
/// omits that component and writes the other. rmw_zenoh decides per
/// component, so a rule that omits the whole duration, or writes both parts,
/// differs from the writer on these rows. The grid has no such row: there both
/// parts are at the default or both are not.
#[test]
fn qos_chunk_equals_rmw_zenoh_when_one_duration_component_is_default() {
    let durations = rows(DURATIONS);
    assert_eq!(
        durations.len(),
        12,
        "4 component pairs for each of deadline, lifespan and lease"
    );
    let bad = chunk_mismatches(&durations);
    assert!(
        bad.is_empty(),
        "{} of {} rows differ from rmw_zenoh; first 3 (row, hiroz, rmw_zenoh): {:?}",
        bad.len(),
        durations.len(),
        &bad[..bad.len().min(3)]
    );
}
