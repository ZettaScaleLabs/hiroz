//! Integration tests for accurate size estimation with SHM serialization.
//!
//! This test suite validates that:
//! 1. Messages with accurate size estimation can be serialized to SHM without panicking
//! 2. The estimates are close to actual sizes (within 5% over-allocation)
//! 3. Large messages (PointCloud2, Image, etc.) work correctly

use std::sync::Arc;

use hiroz::{
    ZBuf,
    msg::{ZMessage, ZSerializer},
    shm::ShmProviderBuilder,
};
use zenoh_buffers::buffer::Buffer;

#[test]
fn test_pointcloud2_shm_serialization_with_accurate_estimate() {
    use hiroz_msgs::{
        builtin_interfaces::Time,
        sensor_msgs::{PointCloud2, PointField},
        std_msgs::Header,
    };

    // Create a large point cloud (1MB)
    let data = vec![0u8; 1_000_000];
    let cloud = PointCloud2 {
        header: Header {
            stamp: Time { sec: 0, nanosec: 0 },
            frame_id: "laser".to_string(),
        },
        height: 1,
        width: 250_000,
        fields: vec![
            PointField {
                name: "x".to_string(),
                offset: 0,
                datatype: 7,
                count: 1,
            },
            PointField {
                name: "y".to_string(),
                offset: 4,
                datatype: 7,
                count: 1,
            },
            PointField {
                name: "z".to_string(),
                offset: 8,
                datatype: 7,
                count: 1,
            },
        ],
        is_bigendian: false,
        point_step: 16,
        row_step: 4_000_000,
        data: ZBuf::from(data),
        is_dense: true,
    };

    // Get accurate estimate
    let estimated = cloud.estimated_serialized_size();
    println!("PointCloud2 estimated size: {} bytes", estimated);

    // Estimate should be reasonable for 1MB data
    assert!(
        estimated >= 1_000_000,
        "Estimate should account for data: {}",
        estimated
    );
    assert!(
        estimated <= 1_050_000,
        "Estimate should not over-allocate excessively: {}",
        estimated
    );

    // Create SHM provider
    let provider = Arc::new(
        ShmProviderBuilder::new(10 * 1024 * 1024)
            .build()
            .expect("Failed to create SHM provider"),
    );

    // Serialize to SHM - should not panic!
    let result = <PointCloud2 as ZMessage>::Serdes::serialize_to_shm(&cloud, estimated, &provider);

    assert!(result.is_ok(), "SHM serialization should succeed");

    let (zbuf, actual_size) = result.unwrap();
    println!("PointCloud2 actual size: {} bytes", actual_size);

    // Verify actual size is within estimate
    assert!(
        actual_size <= estimated,
        "Actual size should be within estimate: {} vs {}",
        actual_size,
        estimated
    );

    // Verify estimate is not too wasteful (within 5% over-allocation)
    let waste_percent = ((estimated as f64 - actual_size as f64) / actual_size as f64) * 100.0;
    println!("Over-allocation: {:.2}%", waste_percent);
    assert!(
        waste_percent < 5.0,
        "Estimate should not waste >5% (wasted: {:.2}%)",
        waste_percent
    );

    // Verify ZBuf has data
    assert!(zbuf.len() > 0, "ZBuf should contain data");
}

#[test]
fn test_image_shm_serialization_with_accurate_estimate() {
    use hiroz_msgs::{builtin_interfaces::Time, sensor_msgs::Image, std_msgs::Header};

    // Create a 640x480 RGB image
    let data = vec![0u8; 640 * 480 * 3];
    let image = Image {
        header: Header {
            stamp: Time { sec: 0, nanosec: 0 },
            frame_id: "camera".to_string(),
        },
        height: 480,
        width: 640,
        encoding: "rgb8".to_string(),
        is_bigendian: 0,
        step: 1920,
        data: ZBuf::from(data),
    };

    let estimated = image.estimated_serialized_size();
    println!("Image estimated size: {} bytes", estimated);

    // Estimate should account for ~921KB data
    assert!(
        estimated >= 921_600,
        "Estimate should account for image data"
    );
    assert!(
        estimated <= 970_000,
        "Estimate should not over-allocate excessively"
    );

    let provider = Arc::new(
        ShmProviderBuilder::new(10 * 1024 * 1024)
            .build()
            .expect("Failed to create SHM provider"),
    );

    let result = <Image as ZMessage>::Serdes::serialize_to_shm(&image, estimated, &provider);
    assert!(result.is_ok(), "SHM serialization should succeed");

    let (zbuf, actual_size) = result.unwrap();
    println!("Image actual size: {} bytes", actual_size);

    assert!(
        actual_size <= estimated,
        "Actual size should be within estimate"
    );

    let waste_percent = ((estimated as f64 - actual_size as f64) / actual_size as f64) * 100.0;
    println!("Over-allocation: {:.2}%", waste_percent);
    assert!(
        waste_percent < 5.0,
        "Estimate should not waste >5%: {:.2}%",
        waste_percent
    );

    assert!(zbuf.len() > 0, "ZBuf should contain data");
}

#[test]
fn test_laserscan_shm_serialization_with_accurate_estimate() {
    use hiroz_msgs::{builtin_interfaces::Time, sensor_msgs::LaserScan, std_msgs::Header};

    let scan = LaserScan {
        header: Header {
            stamp: Time { sec: 0, nanosec: 0 },
            frame_id: "laser".to_string(),
        },
        angle_min: -std::f32::consts::PI,
        angle_max: std::f32::consts::PI,
        angle_increment: 0.01,
        time_increment: 0.0001,
        scan_time: 0.1,
        range_min: 0.1,
        range_max: 30.0,
        ranges: vec![1.5; 628], // ~628 measurements
        intensities: vec![100.0; 628],
    };

    let estimated = scan.estimated_serialized_size();
    println!("LaserScan estimated size: {} bytes", estimated);

    // 628 ranges + 628 intensities = 1256 floats = 5024 bytes
    assert!(
        estimated >= 5024,
        "Estimate should account for float vectors"
    );

    let provider = Arc::new(
        ShmProviderBuilder::new(1024 * 1024)
            .build()
            .expect("Failed to create SHM provider"),
    );

    let result = <LaserScan as ZMessage>::Serdes::serialize_to_shm(&scan, estimated, &provider);
    assert!(result.is_ok(), "SHM serialization should succeed");

    let (zbuf, actual_size) = result.unwrap();
    println!("LaserScan actual size: {} bytes", actual_size);

    assert!(
        actual_size <= estimated,
        "Actual size should be within estimate"
    );

    let waste_percent = ((estimated as f64 - actual_size as f64) / actual_size as f64) * 100.0;
    println!("Over-allocation: {:.2}%", waste_percent);
    assert!(
        waste_percent < 10.0,
        "Estimate should not waste >10%: {:.2}%",
        waste_percent
    );

    assert!(zbuf.len() > 0, "ZBuf should contain data");
}

#[test]
fn test_compressed_image_shm_serialization() {
    use hiroz_msgs::{builtin_interfaces::Time, sensor_msgs::CompressedImage, std_msgs::Header};

    // Compressed JPEG data (much smaller than raw)
    let data = vec![0xFF, 0xD8, 0xFF, 0xE0]; // JPEG header + some data
    let data = [data, vec![0u8; 50_000]].concat(); // ~50KB

    let img = CompressedImage {
        header: Header {
            stamp: Time { sec: 0, nanosec: 0 },
            frame_id: "camera".to_string(),
        },
        format: "jpeg".to_string(),
        data: ZBuf::from(data),
    };

    let estimated = img.estimated_serialized_size();
    println!("CompressedImage estimated size: {} bytes", estimated);

    assert!(estimated >= 50_000, "Should account for compressed data");

    let provider = Arc::new(
        ShmProviderBuilder::new(1024 * 1024)
            .build()
            .expect("Failed to create SHM provider"),
    );

    let result =
        <CompressedImage as ZMessage>::Serdes::serialize_to_shm(&img, estimated, &provider);
    assert!(result.is_ok(), "SHM serialization should succeed");

    let (zbuf, actual_size) = result.unwrap();
    println!("CompressedImage actual size: {} bytes", actual_size);

    assert!(
        actual_size <= estimated,
        "Actual size should be within estimate"
    );

    assert!(zbuf.len() > 0, "ZBuf should contain data");
}

#[test]
fn test_multiple_messages_share_shm_pool() {
    use hiroz_msgs::{builtin_interfaces::Time, sensor_msgs::Image, std_msgs::Header};

    // Create shared SHM pool
    let provider = Arc::new(
        ShmProviderBuilder::new(20 * 1024 * 1024)
            .build()
            .expect("Failed to create SHM provider"),
    );

    // Serialize multiple images using the same pool
    for i in 0..5 {
        let data = vec![i as u8; 100_000]; // 100KB each
        let image = Image {
            header: Header {
                stamp: Time { sec: i, nanosec: 0 },
                frame_id: format!("camera_{}", i),
            },
            height: 100,
            width: 100,
            encoding: "rgb8".to_string(),
            is_bigendian: 0,
            step: 300,
            data: ZBuf::from(data),
        };

        let estimated = image.estimated_serialized_size();
        let result = <Image as ZMessage>::Serdes::serialize_to_shm(&image, estimated, &provider);

        assert!(
            result.is_ok(),
            "Image {} should serialize to shared pool",
            i
        );
        println!("Image {} serialized successfully", i);
    }
}

#[test]
fn test_size_estimate_comparison_with_default() {
    use hiroz_msgs::{builtin_interfaces::Time, sensor_msgs::PointCloud2, std_msgs::Header};

    let data = vec![0u8; 1_000_000];
    let cloud = PointCloud2 {
        header: Header {
            stamp: Time { sec: 0, nanosec: 0 },
            frame_id: "test".to_string(),
        },
        height: 1,
        width: 100_000,
        fields: vec![],
        is_bigendian: false,
        point_step: 16,
        row_step: 1_600_000,
        data: ZBuf::from(data),
        is_dense: true,
    };

    // Our accurate estimate
    let accurate_estimate = cloud.estimated_serialized_size();

    // Default conservative estimate (would be used without our implementation)
    let default_estimate = std::mem::size_of::<PointCloud2>() * 2 + 4;

    println!("Accurate estimate: {} bytes", accurate_estimate);
    println!("Default estimate: {} bytes", default_estimate);
    println!(
        "Improvement: {}x larger (accurate accounts for data)",
        accurate_estimate as f64 / default_estimate as f64
    );

    // Our estimate should be much larger because it accounts for the data Vec
    assert!(
        accurate_estimate > default_estimate * 1000,
        "Accurate estimate should be much larger for messages with data"
    );

    // Default estimate would cause panic with 1MB data
    assert!(
        default_estimate < 1000,
        "Default estimate would be way too small"
    );
}
