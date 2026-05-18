//! CDR round-trip tests for test_interface_files message types.
//!
//! Hand-written Rust structs mirror the generated structs, following the same
//! pattern as hiroz-cdr/tests/cdr_tests.rs. This avoids depending on build-time
//! codegen output at test-compile-time.

use hiroz_cdr::{LittleEndian, from_bytes, to_vec};
use serde::{Deserialize, Serialize};

// ============================================================================
// Helper
// ============================================================================

fn roundtrip<T>(value: &T) -> T
where
    T: Serialize + for<'de> Deserialize<'de> + std::fmt::Debug + PartialEq,
{
    let bytes = to_vec::<_, LittleEndian>(value, 64).expect("serialize failed");
    let (decoded, _) = from_bytes::<T, LittleEndian>(&bytes).expect("deserialize failed");
    decoded
}

// ============================================================================
// BasicTypes
// ============================================================================

#[derive(Debug, PartialEq, Serialize, Deserialize, Default)]
struct BasicTypes {
    bool_value: bool,
    byte_value: u8,
    char_value: u8,
    float32_value: f32,
    float64_value: f64,
    int8_value: i8,
    uint8_value: u8,
    int16_value: i16,
    uint16_value: u16,
    int32_value: i32,
    uint32_value: u32,
    int64_value: i64,
    uint64_value: u64,
    string_value: String,
}

#[test]
fn roundtrip_basic_types_default() {
    let msg = BasicTypes::default();
    assert_eq!(roundtrip(&msg), msg);
}

#[test]
fn roundtrip_basic_types_values() {
    let msg = BasicTypes {
        bool_value: true,
        byte_value: 0xFF,
        char_value: 0x41, // 'A'
        float32_value: 1.5_f32,
        float64_value: -1.5_f64,
        int8_value: -42,
        uint8_value: 200,
        int16_value: -1000,
        uint16_value: 2000,
        int32_value: -100_000,
        uint32_value: 200_000,
        int64_value: i64::MIN / 2,
        uint64_value: u64::MAX / 2,
        string_value: "hello world".to_string(),
    };
    assert_eq!(roundtrip(&msg), msg);
}

// ============================================================================
// Arrays (fixed-size, using tuples/arrays)
// ============================================================================

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Arrays {
    bool_values: [bool; 3],
    byte_values: [u8; 3],
    char_values: [u8; 3],
    float32_values: [f32; 3],
    float64_values: [f64; 3],
    int8_values: [i8; 3],
    uint8_values: [u8; 3],
    int16_values: [i16; 3],
    uint16_values: [u16; 3],
    int32_values: [i32; 3],
    uint32_values: [u32; 3],
    int64_values: [i64; 3],
    uint64_values: [u64; 3],
    string_values: [String; 3],
    basic_types_values: [BasicTypes; 3],
}

impl Default for Arrays {
    fn default() -> Self {
        Self {
            bool_values: [false; 3],
            byte_values: [0; 3],
            char_values: [0; 3],
            float32_values: [0.0; 3],
            float64_values: [0.0; 3],
            int8_values: [0; 3],
            uint8_values: [0; 3],
            int16_values: [0; 3],
            uint16_values: [0; 3],
            int32_values: [0; 3],
            uint32_values: [0; 3],
            int64_values: [0; 3],
            uint64_values: [0; 3],
            string_values: [String::new(), String::new(), String::new()],
            basic_types_values: [
                BasicTypes::default(),
                BasicTypes::default(),
                BasicTypes::default(),
            ],
        }
    }
}

#[test]
fn roundtrip_arrays_default() {
    let msg = Arrays::default();
    assert_eq!(roundtrip(&msg), msg);
}

#[test]
fn roundtrip_arrays_values() {
    let msg = Arrays {
        bool_values: [true, false, true],
        byte_values: [1, 2, 3],
        char_values: [b'a', b'b', b'c'],
        float32_values: [1.0, 2.0, 3.0],
        float64_values: [1.0, -2.0, 3.5],
        int8_values: [-1, 0, 1],
        uint8_values: [0, 128, 255],
        int16_values: [-1000, 0, 1000],
        uint16_values: [0, 1000, 65535],
        int32_values: [-100_000, 0, 100_000],
        uint32_values: [0, 50_000, 100_000],
        int64_values: [-1_000_000, 0, 1_000_000],
        uint64_values: [0, 500_000, 1_000_000],
        string_values: ["a".into(), "bb".into(), "ccc".into()],
        basic_types_values: [
            BasicTypes {
                uint8_value: 1,
                ..Default::default()
            },
            BasicTypes {
                uint8_value: 2,
                ..Default::default()
            },
            BasicTypes {
                uint8_value: 3,
                ..Default::default()
            },
        ],
    };
    assert_eq!(roundtrip(&msg), msg);
}

// ============================================================================
// BoundedSequences (using Vec — CDR serializes same as unbounded)
// ============================================================================

#[derive(Debug, PartialEq, Serialize, Deserialize, Default)]
struct BoundedSequences {
    bool_values: Vec<bool>,
    byte_values: Vec<u8>,
    char_values: Vec<u8>,
    float32_values: Vec<f32>,
    float64_values: Vec<f64>,
    int8_values: Vec<i8>,
    uint8_values: Vec<u8>,
    int16_values: Vec<i16>,
    uint16_values: Vec<u16>,
    int32_values: Vec<i32>,
    uint32_values: Vec<u32>,
    int64_values: Vec<i64>,
    uint64_values: Vec<u64>,
    string_values: Vec<String>,
    basic_types_values: Vec<BasicTypes>,
}

#[test]
fn roundtrip_bounded_sequences_empty() {
    let msg = BoundedSequences::default();
    assert_eq!(roundtrip(&msg), msg);
}

#[test]
fn roundtrip_bounded_sequences_values() {
    let msg = BoundedSequences {
        bool_values: vec![true, false],
        byte_values: vec![1, 2, 3],
        char_values: vec![b'x'],
        float32_values: vec![1.0, 2.0],
        float64_values: vec![-1.5],
        int8_values: vec![-128, 127],
        uint8_values: vec![0, 255],
        int16_values: vec![-1, 1],
        uint16_values: vec![100],
        int32_values: vec![0, 1, -1],
        uint32_values: vec![42],
        int64_values: vec![i64::MAX],
        uint64_values: vec![u64::MAX],
        string_values: vec!["hello".into(), "world".into()],
        basic_types_values: vec![BasicTypes {
            bool_value: true,
            string_value: "nested".into(),
            ..Default::default()
        }],
    };
    assert_eq!(roundtrip(&msg), msg);
}

// ============================================================================
// UnboundedSequences (same struct shape as BoundedSequences at CDR level)
// ============================================================================

#[derive(Debug, PartialEq, Serialize, Deserialize, Default)]
struct UnboundedSequences {
    bool_values: Vec<bool>,
    byte_values: Vec<u8>,
    char_values: Vec<u8>,
    float32_values: Vec<f32>,
    float64_values: Vec<f64>,
    int8_values: Vec<i8>,
    uint8_values: Vec<u8>,
    int16_values: Vec<i16>,
    uint16_values: Vec<u16>,
    int32_values: Vec<i32>,
    uint32_values: Vec<u32>,
    int64_values: Vec<i64>,
    uint64_values: Vec<u64>,
    string_values: Vec<String>,
    basic_types_values: Vec<BasicTypes>,
}

#[test]
fn roundtrip_unbounded_sequences_empty() {
    let msg = UnboundedSequences::default();
    assert_eq!(roundtrip(&msg), msg);
}

#[test]
fn roundtrip_unbounded_sequences_values() {
    let msg = UnboundedSequences {
        bool_values: vec![true],
        byte_values: (0u8..10).collect(),
        char_values: vec![],
        float32_values: vec![0.0, f32::MAX, f32::MIN],
        float64_values: vec![std::f64::consts::PI],
        int8_values: vec![],
        uint8_values: vec![42],
        int16_values: vec![i16::MIN, i16::MAX],
        uint16_values: vec![u16::MAX],
        int32_values: vec![],
        uint32_values: vec![1, 2, 3, 4, 5],
        int64_values: vec![0],
        uint64_values: vec![],
        string_values: vec!["ros".into(), "zenoh".into()],
        basic_types_values: vec![],
    };
    assert_eq!(roundtrip(&msg), msg);
}

// ============================================================================
// Nested
// ============================================================================

#[derive(Debug, PartialEq, Serialize, Deserialize, Default)]
struct Nested {
    basic_types_value: BasicTypes,
}

#[test]
fn roundtrip_nested_default() {
    let msg = Nested::default();
    assert_eq!(roundtrip(&msg), msg);
}

#[test]
fn roundtrip_nested_values() {
    let msg = Nested {
        basic_types_value: BasicTypes {
            bool_value: true,
            int32_value: 42,
            string_value: "nested string".to_string(),
            ..Default::default()
        },
    };
    assert_eq!(roundtrip(&msg), msg);
}

// ============================================================================
// Defaults (fields with default values — same struct shape at CDR level)
// ============================================================================

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Defaults {
    bool_value: bool,
    byte_value: u8,
    char_value: u8,
    float32_value: f32,
    float64_value: f64,
    int8_value: i8,
    uint8_value: u8,
    int16_value: i16,
    uint16_value: u16,
    int32_value: i32,
    uint32_value: u32,
    int64_value: i64,
    uint64_value: u64,
    string_value: String,
}

impl Default for Defaults {
    fn default() -> Self {
        // Mirrors the default values in Defaults.msg
        Self {
            bool_value: true,
            byte_value: 50,
            char_value: 100,
            float32_value: 1.125,
            float64_value: 1.125,
            int8_value: -50,
            uint8_value: 200,
            int16_value: -1000,
            uint16_value: 2000,
            int32_value: -30000,
            uint32_value: 60000,
            int64_value: -40000000,
            uint64_value: 50000000,
            string_value: "hello world".to_string(),
        }
    }
}

#[test]
fn roundtrip_defaults() {
    let msg = Defaults::default();
    assert_eq!(roundtrip(&msg), msg);
}

#[test]
fn roundtrip_defaults_modified() {
    let msg = Defaults {
        bool_value: false,
        string_value: "modified".to_string(),
        int64_value: i64::MIN,
        ..Defaults::default()
    };
    assert_eq!(roundtrip(&msg), msg);
}
