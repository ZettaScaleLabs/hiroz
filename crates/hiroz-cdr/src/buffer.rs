//! Buffer abstraction for CDR serialization output.

use zenoh_buffers::ZBuf;

/// 4KB alignment mask for buffer growth (like CycloneDDS).
const ALIGN_4K: usize = 0xfff;

/// Trait for types that can receive CDR-serialized bytes.
pub trait CdrBuffer {
    /// Append bytes to the buffer.
    fn extend_from_slice(&mut self, data: &[u8]);

    /// Append a single byte.
    fn push(&mut self, byte: u8);

    /// Current length of buffered data.
    fn len(&self) -> usize;

    /// Check if buffer is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Reserve capacity for at least `additional` more bytes.
    fn reserve(&mut self, _additional: usize) {}

    /// Reserve capacity with 4KB-aligned growth granularity.
    ///
    /// This reduces reallocation frequency for growing buffers by
    /// always allocating on 4KB boundaries (like CycloneDDS).
    fn reserve_4k(&mut self, needed_total: usize) {
        // Default implementation just reserves the exact amount
        let current = self.len();
        if needed_total > current {
            self.reserve(needed_total - current);
        }
    }

    /// Clear the buffer for reuse.
    fn clear(&mut self);

    /// Append all ZSlices from a ZBuf directly without copying.
    /// Default implementation copies the data.
    fn append_zbuf(&mut self, zbuf: &ZBuf) {
        use zenoh_buffers::buffer::SplitBuffer;
        let bytes = zbuf.contiguous();
        self.extend_from_slice(&bytes);
    }
}

impl CdrBuffer for Vec<u8> {
    #[inline(always)]
    fn extend_from_slice(&mut self, data: &[u8]) {
        Vec::extend_from_slice(self, data)
    }

    #[inline(always)]
    fn push(&mut self, byte: u8) {
        Vec::push(self, byte)
    }

    #[inline(always)]
    fn len(&self) -> usize {
        Vec::len(self)
    }

    #[inline(always)]
    fn reserve(&mut self, additional: usize) {
        Vec::reserve(self, additional)
    }

    /// Reserve with 4KB-aligned growth for reduced reallocation frequency.
    #[inline]
    fn reserve_4k(&mut self, needed_total: usize) {
        let current_cap = self.capacity();
        if needed_total > current_cap {
            // Round up to next 4KB boundary
            let new_cap = (needed_total + ALIGN_4K) & !ALIGN_4K;
            self.reserve(new_cap - self.len());
        }
    }

    #[inline(always)]
    fn clear(&mut self) {
        Vec::clear(self)
    }
}
