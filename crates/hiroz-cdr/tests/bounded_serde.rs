//! A serde field declared as a bounded sequence rejects a count over its maximum, as soon as the
//! count is known.

use byteorder::LittleEndian;
use hiroz_cdr::{from_bytes, to_vec};
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct Wire {
    values: Vec<u32>,
}

#[derive(Debug, Deserialize)]
struct Bounded {
    #[serde(deserialize_with = "hiroz_cdr::deserialize_bounded::<_, _, 2>")]
    values: Vec<u32>,
}

fn decode(bytes: &[u8]) -> hiroz_cdr::Result<Bounded> {
    from_bytes::<Bounded, LittleEndian>(bytes).map(|(value, _)| value)
}

fn encode(values: &[u32]) -> Vec<u8> {
    let wire = Wire {
        values: values.to_vec(),
    };
    to_vec::<_, LittleEndian>(&wire, 64).expect("serialize")
}

#[test]
fn a_count_at_the_maximum_decodes() {
    let decoded = decode(&encode(&[1, 2])).expect("a count at the maximum decodes");
    assert_eq!(decoded.values, [1, 2]);
}

#[test]
fn a_count_over_the_maximum_is_an_error() {
    let err = decode(&encode(&[1, 2, 3])).expect_err("a count over the maximum");
    assert!(err.to_string().contains("maximum"), "{err}");
}

/// The count is known before any element. A truncated sample with a count over the maximum
/// reports the maximum, not the missing bytes.
#[test]
fn a_truncated_sample_over_the_maximum_names_the_maximum() {
    let mut bytes = 9u32.to_le_bytes().to_vec();
    bytes.extend_from_slice(&1u32.to_le_bytes());
    let err = decode(&bytes).expect_err("a count over the maximum");
    assert!(err.to_string().contains("maximum"), "{err}");
}
