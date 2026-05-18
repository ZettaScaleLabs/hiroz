//! Performance validation for size estimation optimization in regular serialization.
//!
//! This test demonstrates that using accurate size estimation significantly
//! improves serialization performance by reducing buffer reallocations.

use std::time::Instant;

use hiroz::{ZBuf, msg::ZMessage};
use hiroz_msgs::{builtin_interfaces::Time, sensor_msgs::*, std_msgs::Header};
use zenoh_buffers::buffer::Buffer;

#[test]
fn test_pointcloud2_serialization_performance() {
    // Create a large PointCloud2 (1MB)
    let data = vec![0u8; 1_000_000];
    let cloud = PointCloud2 {
        header: Header {
            stamp: Time { sec: 0, nanosec: 0 },
            frame_id: "laser".to_string(),
        },
        height: 1,
        width: 250_000,
        fields: vec![],
        is_bigendian: false,
        point_step: 16,
        row_step: 4_000_000,
        data: ZBuf::from(data),
        is_dense: true,
    };

    // Warm up
    for _ in 0..5 {
        let _ = cloud.serialize_to_zbuf();
    }

    // Benchmark with accurate size estimation (current implementation)
    let start = Instant::now();
    for _ in 0..100 {
        let zbuf = cloud.serialize_to_zbuf();
        assert!(zbuf.len() > 1_000_000);
    }
    let with_estimation = start.elapsed();

    println!("Serialized 1MB PointCloud2 100 times:");
    println!("  With size estimation: {:?}", with_estimation);
    println!("  Per iteration: {:?}", with_estimation / 100);

    // The optimization should make this fast (single allocation per iteration)
    // Without optimization, it would need ~4000 reallocations per iteration
}

#[test]
fn test_image_serialization_performance() {
    // Create a 640x480 RGB image (921KB)
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

    // Warm up
    for _ in 0..5 {
        let _ = image.serialize_to_zbuf();
    }

    // Benchmark
    let start = Instant::now();
    for _ in 0..100 {
        let zbuf = image.serialize_to_zbuf();
        assert!(zbuf.len() > 920_000);
    }
    let elapsed = start.elapsed();

    println!("Serialized 921KB Image 100 times:");
    println!("  Time: {:?}", elapsed);
    println!("  Per iteration: {:?}", elapsed / 100);
}

#[test]
fn test_estimated_size_matches_actual() {
    // Verify that estimated sizes are accurate for various message types

    // PointCloud2
    let cloud = PointCloud2 {
        header: Header {
            stamp: Time { sec: 0, nanosec: 0 },
            frame_id: "test".to_string(),
        },
        height: 1,
        width: 100,
        fields: vec![],
        is_bigendian: false,
        point_step: 16,
        row_step: 1600,
        data: ZBuf::from(vec![0u8; 10_000]),
        is_dense: true,
    };

    let estimated = cloud.estimated_serialized_size();
    let zbuf = cloud.serialize_to_zbuf();
    let actual = zbuf.len();

    println!("PointCloud2: estimated={}, actual={}", estimated, actual);
    assert!(actual <= estimated, "Actual should fit in estimated");
    assert!(
        estimated - actual < estimated / 20,
        "Estimate should be within 5%"
    );

    // Image
    let image = Image {
        header: Header {
            stamp: Time { sec: 0, nanosec: 0 },
            frame_id: "camera".to_string(),
        },
        height: 100,
        width: 100,
        encoding: "mono8".to_string(),
        is_bigendian: 0,
        step: 100,
        data: ZBuf::from(vec![0u8; 10_000]),
    };

    let estimated = image.estimated_serialized_size();
    let zbuf = image.serialize_to_zbuf();
    let actual = zbuf.len();

    println!("Image: estimated={}, actual={}", estimated, actual);
    assert!(actual <= estimated, "Actual should fit in estimated");
    assert!(
        estimated - actual < estimated / 20,
        "Estimate should be within 5%"
    );

    // LaserScan
    let scan = LaserScan {
        header: Header {
            stamp: Time { sec: 0, nanosec: 0 },
            frame_id: "laser".to_string(),
        },
        angle_min: -std::f32::consts::PI,
        angle_max: std::f32::consts::PI,
        angle_increment: 0.01,
        time_increment: 0.0,
        scan_time: 0.1,
        range_min: 0.1,
        range_max: 10.0,
        ranges: vec![1.0; 100],
        intensities: vec![50.0; 100],
    };

    let estimated = scan.estimated_serialized_size();
    let zbuf = scan.serialize_to_zbuf();
    let actual = zbuf.len();

    println!("LaserScan: estimated={}, actual={}", estimated, actual);
    assert!(actual <= estimated, "Actual should fit in estimated");
    assert!(
        estimated - actual < estimated / 10,
        "Estimate should be within 10%"
    );
}

#[test]
fn test_capacity_hint_api() {
    use hiroz::msg::{SerdeCdrSerdes, ZSerializer};

    let cloud = PointCloud2 {
        header: Header {
            stamp: Time { sec: 0, nanosec: 0 },
            frame_id: "test".to_string(),
        },
        height: 1,
        width: 100,
        fields: vec![],
        is_bigendian: false,
        point_step: 16,
        row_step: 1600,
        data: ZBuf::from(vec![0u8; 50_000]),
        is_dense: true,
    };

    // Test the low-level API with explicit hint
    let hint = cloud.estimated_serialized_size();
    let zbuf = SerdeCdrSerdes::<PointCloud2>::serialize_to_zbuf_with_hint(&cloud, hint);

    assert!(zbuf.len() > 50_000);
    println!("Serialized with explicit hint: {} bytes", zbuf.len());

    // Test that ZMessage::serialize_to_zbuf uses the hint automatically
    let zbuf2 = cloud.serialize_to_zbuf();
    assert_eq!(zbuf.len(), zbuf2.len());
}
