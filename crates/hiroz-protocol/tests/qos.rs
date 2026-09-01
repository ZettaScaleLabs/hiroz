//! `QosProfile::decode`/`encode` against `rmw_zenoh_cpp`'s compact wire
//! format, including the SYSTEM_DEFAULT omission rmw_zenoh_cpp uses for QoS
//! sub-fields it did not set explicitly.

use hiroz_protocol::qos::{
    QosDecodeError, QosDurability, QosHistory, QosProfile, QosReliability,
    RMW_ZENOH_DEFAULT_HISTORY_DEPTH,
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
        },
        QosProfile {
            history: QosHistory::KeepAll,
            ..QosProfile::default()
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
