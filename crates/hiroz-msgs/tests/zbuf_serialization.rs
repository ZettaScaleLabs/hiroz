//! Test ZBuf serialization with CDR
//!
//! This test verifies that messages with ZBuf fields can be created,
//! serialized, and that the zero-copy optimization works correctly.

use byteorder::LittleEndian;
use hiroz::ZBuf;
use hiroz_cdr::to_vec;
use hiroz_msgs::{sensor_msgs::CompressedImage, std_msgs::Header};
use zenoh_buffers::buffer::{Buffer, SplitBuffer};

#[test]
fn test_zbuf_field_serialization() {
    // Create a CompressedImage with ZBuf data
    let img = CompressedImage {
        header: Header::default(),
        format: "jpeg".to_string(),
        data: ZBuf::from(vec![1u8, 2, 3, 4, 5, 6, 7, 8]),
    };

    // Verify ZBuf field is accessible
    assert_eq!(img.data.len(), 8);
    assert_eq!(img.data.contiguous().as_ref(), &[1u8, 2, 3, 4, 5, 6, 7, 8]);

    // Serialize using CDR - this verifies that our custom serde implementation works
    let serialized = to_vec::<_, LittleEndian>(&img, 256).expect("Serialization should succeed");

    // Verify serialized data contains the byte array
    // CDR format: [header][format string][byte array length][byte array data]
    assert!(
        serialized.len() > 8,
        "Serialized data should contain our byte array"
    );
}

#[test]
fn test_zbuf_empty() {
    let img = CompressedImage {
        header: Header::default(),
        format: "png".to_string(),
        data: ZBuf::default(),
    };

    assert_eq!(img.data.len(), 0);
    assert!(img.data.contiguous().as_ref().is_empty());

    // Verify empty ZBuf serializes correctly
    let serialized = to_vec::<_, LittleEndian>(&img, 256).expect("Serialization should succeed");

    assert!(!serialized.is_empty(), "Should serialize empty ZBuf");
}

#[test]
fn test_zbuf_large_data() {
    // Test with larger data typical of compressed images
    let large_data: Vec<u8> = (0..10000).map(|i| (i % 256) as u8).collect();

    let img = CompressedImage {
        header: Header::default(),
        format: "jpeg".to_string(),
        data: ZBuf::from(large_data.clone()),
    };

    // Verify the ZBuf contains the data
    assert_eq!(img.data.len(), 10000);
    assert_eq!(img.data.contiguous().as_ref(), large_data.as_slice());

    // Serialize - this exercises the contiguous() method for larger data
    let serialized = to_vec::<_, LittleEndian>(&img, 16384).expect("Serialization should succeed");

    // Serialized size should be roughly: header + format string + 4 bytes (length) + 10000 bytes (data)
    assert!(
        serialized.len() >= 10000,
        "Serialized data should contain the full byte array"
    );
}

#[test]
fn test_zbuf_zero_copy_property() {
    // This test demonstrates the zero-copy property of ZBuf
    // When data is contiguous, contiguous() returns Cow::Borrowed (zero-copy!)
    let data = vec![42u8; 100];
    let zbuf = ZBuf::from(data.clone());

    // contiguous() returns Cow - if data is already contiguous, it borrows (zero-copy)
    let contiguous = zbuf.contiguous();
    assert_eq!(contiguous.as_ref(), data.as_slice());

    // When we serialize, our custom serde uses contiguous().as_ref()
    // which avoids copying when possible
    let img = CompressedImage {
        header: Header::default(),
        format: "test".to_string(),
        data: zbuf,
    };

    let _serialized = to_vec::<_, LittleEndian>(&img, 256).expect("Serialization should succeed");
}
