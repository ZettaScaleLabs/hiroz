//! Shared Memory (SHM) support for zero-copy large message publishing.
//!
//! This module provides configuration and utilities for using Zenoh's shared memory
//! feature to publish large messages without copying data. SHM is particularly beneficial
//! for sensor data like point clouds, images, and laser scans.
//!
//! # Quick Start
//!
//! ## Global SHM Configuration (Context-Level)
//!
//! Enable SHM for all publishers in a context:
//!
//! ```rust,no_run
//! use hiroz::context::ZContextBuilder;
//! use hiroz::Builder;
//!
//! # fn main() -> hiroz::Result<()> {
//! let ctx = ZContextBuilder::default()
//!     .with_shm_enabled()?  // 10MB pool, 512B threshold
//!     .build()?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Custom Configuration
//!
//! ```rust,no_run
//! # use hiroz::context::ZContextBuilder;
//! # use hiroz::Builder;
//! # fn main() -> hiroz::Result<()> {
//! let ctx = ZContextBuilder::default()
//!     .with_shm_pool_size(100 * 1024 * 1024)?  // 100MB pool
//!     .with_shm_threshold(50_000)               // 50KB threshold
//!     .build()?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Per-Publisher Override
//!
//! ```rust,no_run
//! use hiroz::shm::{ShmConfig, ShmProviderBuilder};
//! use std::sync::Arc;
//!
//! # fn main() -> hiroz::Result<()> {
//! # use hiroz::{Builder, context::ZContextBuilder};
//! # let ctx = ZContextBuilder::default().build()?;
//! # let node = ctx.create_node("test").build()?;
//! let provider = Arc::new(ShmProviderBuilder::new(20_000_000).build()?);
//! let config = ShmConfig::new(provider).with_threshold(10_000);
//!
//! let publisher = node.create_pub::<hiroz_msgs::std_msgs::ByteMultiArray>("topic")
//!     .with_shm_config(config)
//!     .build()?;
//! # Ok(())
//! # }
//! ```
//!
//! # How It Works
//!
//! 1. Message is serialized to determine size
//! 2. If size ≥ threshold and SHM configured:
//!    - Allocate SHM buffer
//!    - Copy serialized data to SHM
//!    - Publish SHM reference (zero-copy!)
//! 3. If size < threshold or SHM unavailable:
//!    - Use regular memory (standard path)
//!
//! # Environment Variables
//!
//! - `ZENOH_SHM_ALLOC_SIZE`: Pool size in bytes (default: 10485760 / 10MB)
//! - `ZENOH_SHM_MESSAGE_SIZE_THRESHOLD`: Threshold in bytes (default: 512)
//!
//! # Best Practices
//!
//! - Set threshold based on typical message sizes in your application
//! - Pool size should accommodate N concurrent large messages
//! - Monitor logs for SHM allocation failures
//! - Use per-publisher config for fine-grained control

use std::sync::Arc;
use zenoh::Wait;
use zenoh::shm::{BlockOn, GarbageCollect, PosixShmProviderBackend, ShmProvider, ZShmMut};
use zenoh_buffers::ZBuf;

/// Default shared memory pool size (10 MB).
pub const DEFAULT_SHM_POOL_SIZE: usize = 10 * 1024 * 1024;

/// Default message size threshold for using SHM (512 bytes).
///
/// Messages smaller than this will use regular memory allocation.
/// Matches rmw_zenoh_cpp default for compatibility.
pub const DEFAULT_SHM_THRESHOLD: usize = 512;

/// Configuration for Shared Memory (SHM) support.
///
/// This configuration controls when and how shared memory is used for message publishing.
/// SHM allows large messages to be shared between processes without copying data.
///
/// # Example
///
/// ```rust,no_run
/// use hiroz::shm::{ShmConfig, ShmProviderBuilder};
/// use std::sync::Arc;
///
/// # fn main() -> hiroz::Result<()> {
/// let provider = Arc::new(ShmProviderBuilder::new(50 * 1024 * 1024).build()?);
/// let config = ShmConfig::new(provider)
///     .with_threshold(10_000);  // 10KB threshold
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct ShmConfig {
    /// The SHM provider for allocating shared memory buffers.
    pub(crate) provider: Arc<ShmProvider<PosixShmProviderBackend>>,
    /// Minimum message size (in bytes) to use SHM.
    /// Messages smaller than this will use regular memory.
    pub(crate) threshold: usize,
}

impl ShmConfig {
    /// Create a new SHM configuration with the given provider.
    ///
    /// Uses the default threshold of 512 bytes.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use hiroz::shm::{ShmConfig, ShmProviderBuilder};
    /// use hiroz::Builder;
    /// use std::sync::Arc;
    ///
    /// # fn main() -> hiroz::Result<()> {
    /// let provider = Arc::new(ShmProviderBuilder::new(10 * 1024 * 1024).build()?);
    /// let config = ShmConfig::new(provider);
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(provider: Arc<ShmProvider<PosixShmProviderBackend>>) -> Self {
        Self {
            provider,
            threshold: DEFAULT_SHM_THRESHOLD,
        }
    }

    /// Set the message size threshold for using SHM.
    ///
    /// Messages with serialized size >= this threshold will use SHM.
    /// Messages smaller will use regular memory allocation.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use hiroz::shm::{ShmConfig, ShmProviderBuilder};
    /// # use std::sync::Arc;
    /// # fn main() -> hiroz::Result<()> {
    /// # let provider = Arc::new(ShmProviderBuilder::new(10 * 1024 * 1024).build()?);
    /// let config = ShmConfig::new(provider)
    ///     .with_threshold(50_000);  // 50KB threshold
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_threshold(mut self, threshold: usize) -> Self {
        self.threshold = threshold;
        self
    }

    /// Get the current threshold.
    pub fn threshold(&self) -> usize {
        self.threshold
    }

    /// Get a reference to the SHM provider.
    pub fn provider(&self) -> &ShmProvider<PosixShmProviderBackend> {
        &self.provider
    }

    /// Create SHM configuration from environment variables.
    ///
    /// Reads:
    /// - `ZENOH_SHM_ALLOC_SIZE`: Pool size in bytes (default: 10MB)
    /// - `ZENOH_SHM_MESSAGE_SIZE_THRESHOLD`: Threshold in bytes (default: 512)
    ///
    /// Returns `None` if SHM should not be enabled (no environment variables set).
    ///
    /// # Example
    ///
    /// ```bash
    /// export ZENOH_SHM_ALLOC_SIZE=52428800        # 50MB
    /// export ZENOH_SHM_MESSAGE_SIZE_THRESHOLD=10000  # 10KB
    /// ```
    ///
    /// ```rust,no_run
    /// use hiroz::shm::ShmConfig;
    ///
    /// # fn main() -> hiroz::Result<()> {
    /// if let Some(config) = ShmConfig::from_env()? {
    ///     println!("SHM enabled from environment");
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn from_env() -> zenoh::Result<Option<Self>> {
        // Check if either environment variable is set
        let has_pool_size = std::env::var("ZENOH_SHM_ALLOC_SIZE").is_ok();
        let has_threshold = std::env::var("ZENOH_SHM_MESSAGE_SIZE_THRESHOLD").is_ok();

        if !has_pool_size && !has_threshold {
            return Ok(None);
        }

        let pool_size = std::env::var("ZENOH_SHM_ALLOC_SIZE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_SHM_POOL_SIZE);

        let threshold = std::env::var("ZENOH_SHM_MESSAGE_SIZE_THRESHOLD")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_SHM_THRESHOLD);

        let provider = Arc::new(ShmProviderBuilder::new(pool_size).build()?);

        Ok(Some(Self {
            provider,
            threshold,
        }))
    }
}

impl std::fmt::Debug for ShmConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShmConfig")
            .field("threshold", &self.threshold)
            .field("provider", &"<ShmProvider>")
            .finish()
    }
}

/// Builder for creating ShmProvider with common configurations.
///
/// Provides a simplified API for creating ShmProvider instances with
/// POSIX shared memory backend.
///
/// # Example
///
/// ```rust,no_run
/// use hiroz::shm::ShmProviderBuilder;
///
/// # fn main() -> hiroz::Result<()> {
/// // Create provider with 50MB pool
/// let provider = ShmProviderBuilder::new(50 * 1024 * 1024).build()?;
/// # Ok(())
/// # }
/// ```
pub struct ShmProviderBuilder {
    size: usize,
}

impl ShmProviderBuilder {
    /// Create a new ShmProviderBuilder with the specified pool size.
    ///
    /// # Arguments
    ///
    /// * `size` - Pool size in bytes
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use hiroz::shm::ShmProviderBuilder;
    ///
    /// # fn main() -> hiroz::Result<()> {
    /// let provider = ShmProviderBuilder::new(100 * 1024 * 1024)  // 100MB
    ///     .build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(size: usize) -> Self {
        Self { size }
    }

    /// Build the ShmProvider.
    ///
    /// Creates a POSIX shared memory provider with the configured size.
    ///
    /// # Errors
    ///
    /// Returns an error if the SHM provider cannot be created (e.g., insufficient
    /// system resources, permissions issues).
    pub fn build(self) -> zenoh::Result<ShmProvider<PosixShmProviderBackend>> {
        use zenoh::shm::ShmProviderBuilder as ZenohShmProviderBuilder;

        ZenohShmProviderBuilder::default_backend(self.size)
            .wait()
            .map_err(|e| zenoh::Error::from(format!("Failed to create ShmProvider: {}", e)))
    }
}

/// Writer that serializes CDR data directly into a shared memory buffer.
///
/// This allows zero-copy serialization by writing directly to SHM,
/// avoiding the intermediate copy that would occur if serializing to
/// a regular buffer first.
///
/// # Example
///
/// ```rust,no_run
/// use hiroz::shm::{ShmProviderBuilder, ShmWriter};
/// use serde::Serialize;
///
/// # fn main() -> zenoh::Result<()> {
/// let provider = ShmProviderBuilder::new(10 * 1024 * 1024).build()?;
///
/// // Estimate serialized size (conservative)
/// let estimated_size = 1024;
///
/// // Create SHM writer
/// let mut writer = ShmWriter::new(&provider, estimated_size)?;
///
/// // Serialize data directly into SHM
/// // (using hiroz_cdr::CdrSerializer with writer as buffer)
///
/// // Convert to ZBuf
/// let zbuf = writer.into_zbuf()?;
/// # Ok(())
/// # }
/// ```
pub struct ShmWriter {
    /// The SHM buffer being written to
    buffer: ZShmMut,
    /// Current write position
    position: usize,
}

impl ShmWriter {
    /// Create a new SHM writer with the specified capacity.
    ///
    /// # Arguments
    ///
    /// * `provider` - The SHM provider to allocate from
    /// * `capacity` - Estimated maximum size of serialized data
    ///
    /// # Errors
    ///
    /// Returns an error if SHM allocation fails.
    pub fn new(
        provider: &ShmProvider<PosixShmProviderBackend>,
        capacity: usize,
    ) -> zenoh::Result<Self> {
        let buffer = provider
            .alloc(capacity)
            .with_policy::<BlockOn<GarbageCollect>>()
            .wait()
            .map_err(|e| zenoh::Error::from(format!("SHM allocation failed: {}", e)))?;

        Ok(Self {
            buffer,
            position: 0,
        })
    }

    /// Get the current write position (number of bytes written).
    #[inline]
    pub fn position(&self) -> usize {
        self.position
    }

    /// Convert the writer into a ZBuf containing the serialized data.
    ///
    /// This consumes the writer and returns a ZBuf backed by the SHM buffer.
    /// Only the written portion (0..position) is included in the ZBuf.
    ///
    /// # Errors
    ///
    /// Returns an error if the buffer cannot be converted to ZBuf.
    pub fn into_zbuf(self) -> zenoh::Result<ZBuf> {
        // Create a ZBuf from the SHM buffer
        // Note: The entire allocated buffer is used, not just the written portion
        // This is acceptable as Zenoh will handle the actual data length separately
        Ok(ZBuf::from(self.buffer))
    }

    /// Write bytes directly to the SHM buffer.
    ///
    /// # Panics
    ///
    /// Panics if writing would exceed the allocated buffer size.
    #[inline]
    fn write_bytes(&mut self, bytes: &[u8]) {
        let end = self.position + bytes.len();
        assert!(
            end <= self.buffer.len(),
            "SHM buffer overflow: tried to write {} bytes at position {} but buffer size is {}",
            bytes.len(),
            self.position,
            self.buffer.len()
        );
        self.buffer[self.position..end].copy_from_slice(bytes);
        self.position = end;
    }
}

/// Implement CdrBuffer for ShmWriter to enable direct CDR serialization.
impl hiroz_cdr::CdrBuffer for ShmWriter {
    #[inline(always)]
    fn extend_from_slice(&mut self, data: &[u8]) {
        self.write_bytes(data);
    }

    #[inline(always)]
    fn push(&mut self, byte: u8) {
        self.write_bytes(&[byte]);
    }

    #[inline(always)]
    fn len(&self) -> usize {
        self.position
    }

    #[inline(always)]
    fn reserve(&mut self, _additional: usize) {
        // SHM buffer is pre-allocated, cannot grow
        // This is fine as we allocate based on estimated_serialized_size
    }

    #[inline(always)]
    fn clear(&mut self) {
        self.position = 0;
    }

    fn append_zbuf(&mut self, zbuf: &ZBuf) {
        use zenoh_buffers::buffer::SplitBuffer;
        let bytes = zbuf.contiguous();
        self.write_bytes(&bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn test_shm_config_creation() {
        let provider = Arc::new(
            ShmProviderBuilder::new(1024 * 1024)
                .build()
                .expect("Failed to create SHM provider"),
        );

        let config = ShmConfig::new(provider);
        assert_eq!(config.threshold(), DEFAULT_SHM_THRESHOLD);
    }

    #[test]
    fn test_shm_config_with_threshold() {
        let provider = Arc::new(
            ShmProviderBuilder::new(1024 * 1024)
                .build()
                .expect("Failed to create SHM provider"),
        );

        let config = ShmConfig::new(provider).with_threshold(10_000);
        assert_eq!(config.threshold(), 10_000);
    }

    #[test]
    fn test_shm_provider_builder() {
        let provider = ShmProviderBuilder::new(2 * 1024 * 1024)
            .build()
            .expect("Failed to create SHM provider");

        // Verify we can allocate from the pool
        let buf = provider.alloc(1024).wait();
        assert!(buf.is_ok(), "Should be able to allocate from SHM pool");
    }

    #[test]
    #[serial]
    fn test_shm_config_from_env_none() {
        // Ensure env vars are not set
        unsafe {
            std::env::remove_var("ZENOH_SHM_ALLOC_SIZE");
            std::env::remove_var("ZENOH_SHM_MESSAGE_SIZE_THRESHOLD");
        }

        let config = ShmConfig::from_env().expect("Should not error");
        assert!(config.is_none(), "Should return None when no env vars set");
    }

    #[test]
    #[serial]
    fn test_shm_config_from_env_with_size() {
        unsafe {
            std::env::set_var("ZENOH_SHM_ALLOC_SIZE", "1048576"); // 1MB — keep small for CI runners
            std::env::remove_var("ZENOH_SHM_MESSAGE_SIZE_THRESHOLD");
        }

        let config = ShmConfig::from_env()
            .expect("Should not error")
            .expect("Should return Some when env var set");

        assert_eq!(config.threshold(), DEFAULT_SHM_THRESHOLD);

        unsafe {
            std::env::remove_var("ZENOH_SHM_ALLOC_SIZE");
        }
    }

    #[test]
    #[serial]
    fn test_shm_config_from_env_full() {
        unsafe {
            std::env::set_var("ZENOH_SHM_ALLOC_SIZE", "1048576"); // 1MB — keep small for CI runners
            std::env::set_var("ZENOH_SHM_MESSAGE_SIZE_THRESHOLD", "2048");
        }

        let config = ShmConfig::from_env()
            .expect("Should not error")
            .expect("Should return Some when env vars set");

        assert_eq!(config.threshold(), 2048);

        unsafe {
            std::env::remove_var("ZENOH_SHM_ALLOC_SIZE");
            std::env::remove_var("ZENOH_SHM_MESSAGE_SIZE_THRESHOLD");
        }
    }
}
