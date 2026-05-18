//! Integration tests for hiroz-cdr

use byteorder::{BigEndian, LittleEndian};
use hiroz_cdr::{CdrBuffer, CdrDeserializer, ZBufWriter, from_bytes, to_vec, to_vec_reuse};
use serde::{Deserialize, Serialize};
use zenoh_buffers::{ZSlice, buffer::SplitBuffer};

// ============================================================================
// Buffer tests
// ============================================================================

#[test]
fn test_vec_cdr_buffer() {
    let mut buf: Vec<u8> = Vec::new();

    // Use CdrBuffer trait methods explicitly
    CdrBuffer::extend_from_slice(&mut buf, &[1, 2, 3]);
    assert_eq!(CdrBuffer::len(&buf), 3);

    CdrBuffer::push(&mut buf, 4);
    assert_eq!(CdrBuffer::len(&buf), 4);

    assert!(!CdrBuffer::is_empty(&buf));

    CdrBuffer::clear(&mut buf);
    assert!(CdrBuffer::is_empty(&buf));
}

// ============================================================================
// ZBufWriter tests
// ============================================================================

#[test]
fn test_zbuf_writer_basic() {
    let mut writer = ZBufWriter::new();

    writer.extend_from_slice(&[1, 2, 3]);
    assert_eq!(writer.len(), 3);

    writer.push(4);
    assert_eq!(writer.len(), 4);

    assert!(!writer.is_empty());
}

#[test]
fn test_zbuf_writer_into_zbuf() {
    let mut writer = ZBufWriter::with_capacity(16);
    writer.extend_from_slice(&[10, 20, 30, 40]);

    let zbuf = writer.into_zbuf();
    let bytes = zbuf.contiguous();
    assert_eq!(&*bytes, &[10, 20, 30, 40]);
}

#[test]
fn test_zbuf_writer_append_zslice() {
    let mut writer = ZBufWriter::new();

    writer.extend_from_slice(&[0xAA, 0xBB]);

    let data: ZSlice = vec![1u8, 2, 3, 4, 5].into();
    writer.append_zslice(data);

    writer.extend_from_slice(&[0xCC, 0xDD]);

    let zbuf = writer.into_zbuf();
    let bytes = zbuf.contiguous();
    assert_eq!(&*bytes, &[0xAA, 0xBB, 1, 2, 3, 4, 5, 0xCC, 0xDD]);
}

// ============================================================================
// Serializer tests
// ============================================================================

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct Example {
    a: u32,
    b: [u8; 4],
}

#[test]
fn test_serializer_basic() {
    let o = Example {
        a: 1,
        b: [b'a', b'b', b'c', b'd'],
    };

    let expected: Vec<u8> = vec![0x01, 0x00, 0x00, 0x00, 0x61, 0x62, 0x63, 0x64];

    let serialized = to_vec::<_, LittleEndian>(&o, 16).unwrap();
    assert_eq!(serialized, expected);
}

#[test]
fn test_serializer_bytes() {
    let data = vec![0u8, 1, 2, 3, 4, 5];
    let serialized = to_vec::<_, LittleEndian>(&data, 16).unwrap();

    assert_eq!(serialized.len(), 4 + 6);
    assert_eq!(&serialized[0..4], &[6, 0, 0, 0]);
    assert_eq!(&serialized[4..], &[0, 1, 2, 3, 4, 5]);
}

#[test]
fn test_buffer_reuse() {
    let data1 = vec![1u8, 2, 3];
    let data2 = vec![4u8, 5, 6, 7, 8];

    let mut buffer = Vec::new();

    to_vec_reuse::<_, LittleEndian>(&data1, &mut buffer).unwrap();
    let len1 = buffer.len();
    assert!(len1 > 0);

    to_vec_reuse::<_, LittleEndian>(&data2, &mut buffer).unwrap();
    let len2 = buffer.len();
    assert!(len2 > len1);
}

// ============================================================================
// Deserializer tests
// ============================================================================

fn deserialize_from_little_endian<'de, T>(s: &'de [u8]) -> hiroz_cdr::Result<T>
where
    T: serde::Deserialize<'de>,
{
    let mut deserializer = CdrDeserializer::<LittleEndian>::new(s);
    T::deserialize(&mut deserializer)
}

fn deserialize_from_big_endian<'de, T>(s: &'de [u8]) -> hiroz_cdr::Result<T>
where
    T: serde::Deserialize<'de>,
{
    let mut deserializer = CdrDeserializer::<BigEndian>::new(s);
    T::deserialize(&mut deserializer)
}

#[test]
fn test_basic_types() {
    // u8
    let data: &[u8] = &[42];
    let val: u8 = deserialize_from_little_endian(data).unwrap();
    assert_eq!(val, 42);

    // i32 with alignment
    let data: &[u8] = &[0x78, 0x56, 0x34, 0x12];
    let val: i32 = deserialize_from_little_endian(data).unwrap();
    assert_eq!(val, 0x12345678);

    // bool
    let data: &[u8] = &[1];
    let val: bool = deserialize_from_little_endian(data).unwrap();
    assert!(val);

    let data: &[u8] = &[0];
    let val: bool = deserialize_from_little_endian(data).unwrap();
    assert!(!val);
}

#[test]
fn test_string() {
    // "abc" with null terminator, length = 4
    let data: &[u8] = &[0x04, 0x00, 0x00, 0x00, 0x61, 0x62, 0x63, 0x00];
    let val: String = deserialize_from_little_endian(data).unwrap();
    assert_eq!(val, "abc");
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct SimpleStruct {
    x: i32,
    y: i32,
}

#[test]
fn test_struct() {
    let data: &[u8] = &[
        0x01, 0x00, 0x00, 0x00, // x = 1
        0x02, 0x00, 0x00, 0x00, // y = 2
    ];
    let val: SimpleStruct = deserialize_from_little_endian(data).unwrap();
    assert_eq!(val, SimpleStruct { x: 1, y: 2 });
}

#[test]
fn test_vec() {
    // Vec<i32> with 3 elements [1, 2, 3]
    let data: &[u8] = &[
        0x03, 0x00, 0x00, 0x00, // length = 3
        0x01, 0x00, 0x00, 0x00, // 1
        0x02, 0x00, 0x00, 0x00, // 2
        0x03, 0x00, 0x00, 0x00, // 3
    ];
    let val: Vec<i32> = deserialize_from_little_endian(data).unwrap();
    assert_eq!(val, vec![1, 2, 3]);
}

#[test]
fn test_round_trip() {
    let original = SimpleStruct { x: 42, y: -100 };
    let serialized = to_vec::<_, LittleEndian>(&original, 64).unwrap();
    let (deserialized, bytes_consumed): (SimpleStruct, usize) =
        from_bytes::<SimpleStruct, LittleEndian>(&serialized).unwrap();
    assert_eq!(original, deserialized);
    assert_eq!(serialized.len(), bytes_consumed);
}

#[test]
fn test_endianness() {
    let val: i32 = 0x12345678;
    let le_bytes = to_vec::<_, LittleEndian>(&val, 16).unwrap();
    let be_bytes = to_vec::<_, BigEndian>(&val, 16).unwrap();

    let le_result: i32 = deserialize_from_little_endian(&le_bytes).unwrap();
    let be_result: i32 = deserialize_from_big_endian(&be_bytes).unwrap();

    assert_eq!(val, le_result);
    assert_eq!(val, be_result);
}

// ============================================================================
// Proptest: CDR roundtrip coverage (bulk POD path + serde path)
// ============================================================================

use hiroz_cdr::{CdrPlain, CdrReader, CdrWriter};
use proptest::prelude::*;

/// Write a slice via `write_pod_slice` and read it back via `read_pod_slice`.
#[cfg(target_endian = "little")]
fn pod_slice_roundtrip<T>(values: &[T]) -> Vec<T>
where
    T: CdrPlain + bytemuck::Pod + PartialEq + std::fmt::Debug,
{
    // Serialize
    let mut buffer: Vec<u8> = Vec::new();
    {
        let mut writer = CdrWriter::<LittleEndian, _>::new(&mut buffer);
        // write_pod_slice asserts !slice.is_empty, so we need to handle empty specially
        if !values.is_empty() {
            writer.write_pod_slice(values);
        }
    }

    // Deserialize
    let mut reader = CdrReader::<LittleEndian>::new(&buffer);
    if values.is_empty() {
        vec![]
    } else {
        reader
            .read_pod_slice::<T>(values.len())
            .expect("read_pod_slice")
    }
}

proptest! {
    // ── Bulk-copy path: numeric types ────────────────────────────────────────

    #[test]
    fn prop_pod_slice_i8(values in proptest::collection::vec(any::<i8>(), 1..=64)) {
        #[cfg(target_endian = "little")]
        {
            let result = pod_slice_roundtrip(&values);
            prop_assert_eq!(result, values);
        }
    }

    #[test]
    fn prop_pod_slice_u8(values in proptest::collection::vec(any::<u8>(), 1..=64)) {
        #[cfg(target_endian = "little")]
        {
            let result = pod_slice_roundtrip(&values);
            prop_assert_eq!(result, values);
        }
    }

    #[test]
    fn prop_pod_slice_i16(values in proptest::collection::vec(any::<i16>(), 1..=64)) {
        #[cfg(target_endian = "little")]
        {
            let result = pod_slice_roundtrip(&values);
            prop_assert_eq!(result, values);
        }
    }

    #[test]
    fn prop_pod_slice_u16(values in proptest::collection::vec(any::<u16>(), 1..=64)) {
        #[cfg(target_endian = "little")]
        {
            let result = pod_slice_roundtrip(&values);
            prop_assert_eq!(result, values);
        }
    }

    #[test]
    fn prop_pod_slice_i32(values in proptest::collection::vec(any::<i32>(), 1..=64)) {
        #[cfg(target_endian = "little")]
        {
            let result = pod_slice_roundtrip(&values);
            prop_assert_eq!(result, values);
        }
    }

    #[test]
    fn prop_pod_slice_u32(values in proptest::collection::vec(any::<u32>(), 1..=64)) {
        #[cfg(target_endian = "little")]
        {
            let result = pod_slice_roundtrip(&values);
            prop_assert_eq!(result, values);
        }
    }

    #[test]
    fn prop_pod_slice_i64(values in proptest::collection::vec(any::<i64>(), 1..=64)) {
        #[cfg(target_endian = "little")]
        {
            let result = pod_slice_roundtrip(&values);
            prop_assert_eq!(result, values);
        }
    }

    #[test]
    fn prop_pod_slice_u64(values in proptest::collection::vec(any::<u64>(), 1..=64)) {
        #[cfg(target_endian = "little")]
        {
            let result = pod_slice_roundtrip(&values);
            prop_assert_eq!(result, values);
        }
    }

    #[test]
    fn prop_pod_slice_f32(values in proptest::collection::vec(any::<f32>(), 1..=64)) {
        #[cfg(target_endian = "little")]
        {
            let result = pod_slice_roundtrip(&values);
            // f32::NaN != NaN, so compare bits instead
            for (a, b) in result.iter().zip(values.iter()) {
                prop_assert_eq!(a.to_bits(), b.to_bits());
            }
        }
    }

    #[test]
    fn prop_pod_slice_f64(values in proptest::collection::vec(any::<f64>(), 1..=64)) {
        #[cfg(target_endian = "little")]
        {
            let result = pod_slice_roundtrip(&values);
            for (a, b) in result.iter().zip(values.iter()) {
                prop_assert_eq!(a.to_bits(), b.to_bits());
            }
        }
    }

    // ── Bulk-copy path: empty slice (len=0) handled correctly ─────────────

    #[test]
    fn prop_pod_slice_empty_i32(_unused in 0_u8..=0) {
        #[cfg(target_endian = "little")]
        {
            let empty: Vec<i32> = vec![];
            let result = pod_slice_roundtrip(&empty);
            prop_assert_eq!(result, empty);
        }
    }

    // ── Bulk-copy path: mid-stream alignment (u8 before slice) ───────────

    #[test]
    fn prop_pod_slice_mid_stream_alignment(
        prefix in any::<u8>(),
        values in proptest::collection::vec(any::<i32>(), 1..=32),
    ) {
        #[cfg(target_endian = "little")]
        {
            let mut buffer: Vec<u8> = Vec::new();
            {
                let mut writer = CdrWriter::<LittleEndian, _>::new(&mut buffer);
                // Write a u8 first to force non-zero stream position
                writer.write_u8(prefix);
                writer.write_pod_slice(&values);
            }

            let mut reader = CdrReader::<LittleEndian>::new(&buffer);
            let read_prefix = reader.read_u8().expect("read prefix");
            let read_values = reader.read_pod_slice::<i32>(values.len()).expect("read slice");

            prop_assert_eq!(read_prefix, prefix);
            prop_assert_eq!(read_values, values);
        }
    }

    // ── Serde path: Vec<i32> roundtrip ───────────────────────────────────

    #[test]
    fn prop_serde_vec_i32(values in proptest::collection::vec(any::<i32>(), 0..=64)) {
        let serialized = to_vec::<_, LittleEndian>(&values, 256).expect("serialize");
        let (deserialized, _): (Vec<i32>, _) = from_bytes::<Vec<i32>, LittleEndian>(&serialized).expect("deserialize");
        prop_assert_eq!(deserialized, values);
    }

    #[test]
    fn prop_serde_vec_f64(values in proptest::collection::vec(any::<f64>(), 0..=64)) {
        let serialized = to_vec::<_, LittleEndian>(&values, 256).expect("serialize");
        let (deserialized, _): (Vec<f64>, _) = from_bytes::<Vec<f64>, LittleEndian>(&serialized).expect("deserialize");
        for (a, b) in deserialized.iter().zip(values.iter()) {
            prop_assert_eq!(a.to_bits(), b.to_bits());
        }
    }

    // ── Serde path: Vec<String> roundtrip ────────────────────────────────

    #[test]
    fn prop_serde_vec_string(
        values in proptest::collection::vec(
            "[a-zA-Z0-9 !@#$%^&*()_+\\-=\\[\\]{}|;':\",./<>?]{0,64}",
            0..=16,
        ),
    ) {
        let serialized = to_vec::<_, LittleEndian>(&values, 512).expect("serialize");
        let (deserialized, _): (Vec<String>, _) = from_bytes::<Vec<String>, LittleEndian>(&serialized).expect("deserialize");
        prop_assert_eq!(deserialized, values);
    }

    // ── Serde path: String with unicode ──────────────────────────────────

    #[test]
    fn prop_serde_string_unicode(s in "\\PC{0,100}") {
        let serialized = to_vec::<_, LittleEndian>(&s, 256).expect("serialize");
        let (deserialized, _): (String, _) = from_bytes::<String, LittleEndian>(&serialized).expect("deserialize");
        prop_assert_eq!(deserialized, s);
    }
}
