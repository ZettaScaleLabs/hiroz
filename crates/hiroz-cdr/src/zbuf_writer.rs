//! ZBuf writer for zero-copy CDR serialization output.

use zenoh_buffers::{ZBuf, ZSlice};

use crate::buffer::CdrBuffer;

/// A writer that accumulates bytes and ZSlices into a ZBuf.
pub struct ZBufWriter {
    /// Accumulated ZSlices
    zbuf: ZBuf,
    /// Current buffer for small writes (header, padding, length prefixes)
    current: Vec<u8>,
    /// Track total length for alignment calculations
    total_len: usize,
}

impl Default for ZBufWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl ZBufWriter {
    /// Create a new empty ZBufWriter.
    #[inline]
    pub fn new() -> Self {
        Self {
            zbuf: ZBuf::empty(),
            current: Vec::new(),
            total_len: 0,
        }
    }

    /// Create a new ZBufWriter with pre-allocated capacity for the current buffer.
    #[inline]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            zbuf: ZBuf::empty(),
            current: Vec::with_capacity(capacity),
            total_len: 0,
        }
    }

    /// Flush the current byte buffer to the ZBuf as a ZSlice.
    #[inline]
    fn flush_current(&mut self) {
        if !self.current.is_empty() {
            let slice = std::mem::take(&mut self.current);
            self.zbuf.push_zslice(ZSlice::from(slice));
        }
    }

    /// Append a ZSlice directly without copying.
    #[inline]
    pub fn append_zslice(&mut self, slice: ZSlice) {
        let len = slice.len();
        self.flush_current();
        self.zbuf.push_zslice(slice);
        self.total_len += len;
    }

    /// Consume the writer and produce a ZBuf.
    #[inline]
    pub fn into_zbuf(mut self) -> ZBuf {
        self.flush_current();
        self.zbuf
    }

    /// Get the current capacity of the internal byte buffer.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.current.capacity()
    }
}

impl CdrBuffer for ZBufWriter {
    #[inline(always)]
    fn extend_from_slice(&mut self, data: &[u8]) {
        self.current.extend_from_slice(data);
        self.total_len += data.len();
    }

    #[inline(always)]
    fn push(&mut self, byte: u8) {
        self.current.push(byte);
        self.total_len += 1;
    }

    #[inline(always)]
    fn len(&self) -> usize {
        self.total_len
    }

    #[inline(always)]
    fn reserve(&mut self, additional: usize) {
        self.current.reserve(additional)
    }

    #[inline(always)]
    fn clear(&mut self) {
        self.zbuf.clear();
        self.current.clear();
        self.total_len = 0;
    }

    #[inline]
    fn append_zbuf(&mut self, zbuf: &ZBuf) {
        for zslice in zbuf.zslices() {
            self.append_zslice(zslice.clone());
        }
    }
}
