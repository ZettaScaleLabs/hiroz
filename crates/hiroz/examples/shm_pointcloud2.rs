//! PointCloud2 example demonstrating user-managed SHM for zero-copy point clouds.
//!
//! This example shows how to create large sensor messages with data stored directly
//! in shared memory, avoiding any intermediate copies.
//!
//! # Three SHM Patterns Demonstrated:
//!
//! 1. **User-Managed SHM** (Primary): Allocate SHM buffer, write points, create message
//! 2. **Automatic SHM** (Context-level): Enable SHM globally, automatic threshold-based usage
//! 3. **Per-Publisher SHM**: Override SHM config for specific publisher
//!
//! # Usage:
//! ```bash
//! cargo run --example shm_pointcloud2
//! ```

use std::{sync::Arc, time::Instant};

use hiroz::{
    Builder, ZBuf,
    context::ZContextBuilder,
    shm::{ShmConfig, ShmProviderBuilder},
};
use hiroz_msgs::{
    sensor_msgs::{PointCloud2, PointField},
    std_msgs::Header,
};
use zenoh::{
    Wait,
    shm::{BlockOn, GarbageCollect, ShmProvider},
};
use zenoh_buffers::buffer::Buffer;

fn main() -> zenoh::Result<()> {
    println!("=== PointCloud2 with SHM Example ===\n");

    // Pattern 1: User-managed SHM (maximum performance, full control)
    println!("1. User-Managed SHM Pattern:");
    demo_user_managed_shm()?;

    println!("\n2. Automatic SHM Pattern (Context-level):");
    demo_automatic_shm()?;

    println!("\n3. Per-Publisher SHM Override:");
    demo_publisher_shm_override()?;

    println!("\n=== All patterns completed successfully ===");
    Ok(())
}

/// Pattern 1: User creates SHM buffer, writes points, constructs PointCloud2
fn demo_user_managed_shm() -> zenoh::Result<()> {
    // Step 1: Initialize SHM provider
    let provider = ShmProviderBuilder::new(50 * 1024 * 1024).build()?;
    println!("  ✓ Created SHM provider with 50MB pool");

    // Step 2: Generate point cloud with SHM-backed data
    let start = Instant::now();
    let cloud = generate_pointcloud_with_shm(100_000, &provider)?;
    let gen_time = start.elapsed();

    println!(
        "  ✓ Generated 100k point cloud ({} KB) in {:?}",
        cloud.data.len() / 1024,
        gen_time
    );
    println!("    Points stored directly in SHM (zero-copy!)");

    // Step 3: Create node and publisher
    let ctx = ZContextBuilder::default().build()?;
    let node = ctx.create_node("pointcloud_publisher").build()?;
    let publisher = node
        .create_pub::<PointCloud2>("cloud/user_managed")
        .build()?;

    // Step 4: Publish (data field is already in SHM)
    let start = Instant::now();
    publisher.publish(&cloud)?;
    let pub_time = start.elapsed();

    println!(
        "  ✓ Published in {:?} (data already in SHM, only metadata serialized)",
        pub_time
    );

    Ok(())
}

/// Pattern 2: Enable SHM at context level, automatic for large messages
fn demo_automatic_shm() -> zenoh::Result<()> {
    // Enable SHM globally
    let ctx = ZContextBuilder::default()
        .with_shm_pool_size(50 * 1024 * 1024)?
        .with_shm_threshold(10_000) // 10KB threshold
        .build()?;
    println!("  ✓ Context configured with automatic SHM (threshold: 10KB)");

    let node = ctx.create_node("pointcloud_publisher").build()?;
    let publisher = node.create_pub::<PointCloud2>("cloud/automatic").build()?;

    // Generate point cloud normally (using Vec<u8>)
    let start = Instant::now();
    let cloud = generate_pointcloud_normal(50_000);
    let gen_time = start.elapsed();

    println!(
        "  ✓ Generated 50k point cloud ({} KB) in {:?}",
        cloud.data.len() / 1024,
        gen_time
    );

    // Publish - automatically uses SHM because message > threshold
    let start = Instant::now();
    publisher.publish(&cloud)?;
    let pub_time = start.elapsed();

    println!(
        "  ✓ Published in {:?} (serialized ~600KB > 10KB, automatically used SHM)",
        pub_time
    );

    Ok(())
}

/// Pattern 3: Per-publisher SHM configuration
fn demo_publisher_shm_override() -> zenoh::Result<()> {
    // Context has no SHM, but publisher has its own config
    let ctx = ZContextBuilder::default().build()?;
    let node = ctx.create_node("pointcloud_publisher").build()?;

    // Create SHM provider for this publisher only
    let provider = Arc::new(ShmProviderBuilder::new(30 * 1024 * 1024).build()?);
    let shm_config = ShmConfig::new(provider).with_threshold(5_000); // 5KB threshold

    let publisher = node
        .create_pub::<PointCloud2>("cloud/per_publisher")
        .with_shm_config(shm_config)
        .build()?;

    println!("  ✓ Publisher configured with custom SHM (threshold: 5KB)");

    let cloud = generate_pointcloud_normal(30_000);
    println!(
        "  ✓ Generated 30k point cloud ({} KB)",
        cloud.data.len() / 1024
    );

    let start = Instant::now();
    publisher.publish(&cloud)?;
    let pub_time = start.elapsed();

    println!(
        "  ✓ Published in {:?} (used publisher's SHM config)",
        pub_time
    );

    Ok(())
}

/// Generate point cloud with user-managed SHM (Pattern 1: zero-copy)
fn generate_pointcloud_with_shm(
    num_points: usize,
    provider: &ShmProvider<zenoh::shm::PosixShmProviderBackend>,
) -> zenoh::Result<PointCloud2> {
    let point_step = 12; // x, y, z as f32 (4 bytes each)
    let data_size = num_points * point_step;

    // Allocate SHM buffer for point data
    let mut shm_buf = provider
        .alloc(data_size)
        .with_policy::<BlockOn<GarbageCollect>>()
        .wait()?;

    // Write point coordinates directly into SHM buffer
    for i in 0..num_points {
        let offset = i * point_step;
        let angle = (i as f32) * 0.01;
        let radius = 5.0 + (angle * 0.1).sin();

        // Calculate x, y, z
        let x = radius * angle.cos();
        let y = radius * angle.sin();
        let z = (i as f32) * 0.001;

        // Write directly to SHM (no intermediate Vec<u8>)
        shm_buf[offset..offset + 4].copy_from_slice(&x.to_le_bytes());
        shm_buf[offset + 4..offset + 8].copy_from_slice(&y.to_le_bytes());
        shm_buf[offset + 8..offset + 12].copy_from_slice(&z.to_le_bytes());
    }

    // Create ZBuf from SHM buffer (zero-copy conversion!)
    let data_zbuf = ZBuf::from(shm_buf);

    // Construct PointCloud2 with SHM-backed ZBuf
    Ok(PointCloud2 {
        header: Header {
            frame_id: "map".into(),
            ..Default::default()
        },
        height: 1,
        width: num_points as u32,
        fields: vec![
            PointField {
                name: "x".into(),
                offset: 0,
                datatype: 7, // FLOAT32
                count: 1,
            },
            PointField {
                name: "y".into(),
                offset: 4,
                datatype: 7,
                count: 1,
            },
            PointField {
                name: "z".into(),
                offset: 8,
                datatype: 7,
                count: 1,
            },
        ],
        is_bigendian: false,
        point_step: point_step as u32,
        row_step: (num_points * point_step) as u32,
        data: data_zbuf, // SHM-backed data!
        is_dense: true,
    })
}

/// Generate point cloud normally (Pattern 2 & 3: uses Vec<u8>, then automatic SHM)
fn generate_pointcloud_normal(num_points: usize) -> PointCloud2 {
    let point_step = 12;
    let mut data = Vec::with_capacity(num_points * point_step);

    for i in 0..num_points {
        let angle = (i as f32) * 0.01;
        let radius = 5.0 + (angle * 0.1).sin();

        let x = radius * angle.cos();
        let y = radius * angle.sin();
        let z = (i as f32) * 0.001;

        data.extend_from_slice(&x.to_le_bytes());
        data.extend_from_slice(&y.to_le_bytes());
        data.extend_from_slice(&z.to_le_bytes());
    }

    PointCloud2 {
        header: Header {
            frame_id: "map".into(),
            ..Default::default()
        },
        height: 1,
        width: num_points as u32,
        fields: vec![
            PointField {
                name: "x".into(),
                offset: 0,
                datatype: 7,
                count: 1,
            },
            PointField {
                name: "y".into(),
                offset: 4,
                datatype: 7,
                count: 1,
            },
            PointField {
                name: "z".into(),
                offset: 8,
                datatype: 7,
                count: 1,
            },
        ],
        is_bigendian: false,
        point_step: point_step as u32,
        row_step: (num_points * point_step) as u32,
        data: ZBuf::from(data),
        is_dense: true,
    }
}
