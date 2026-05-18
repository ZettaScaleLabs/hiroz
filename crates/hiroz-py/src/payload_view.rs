//! Zero-copy payload view for Python buffer protocol.
//!
//! This module provides `ZPayloadView`, a PyO3 class that wraps a Zenoh Sample
//! and exposes its payload via Python's buffer protocol. This enables true
//! zero-copy access from Python via `memoryview()` or `numpy.frombuffer()`.

use pyo3::exceptions::PyBufferError;
use pyo3::ffi;
use pyo3::prelude::*;
use std::borrow::Cow;
use std::ffi::{CString, c_int, c_void};
use std::ptr;
use zenoh::sample::Sample;

/// Stores either a borrowed pointer (zero-copy) or owned Vec (fallback for fragmented buffers)
enum PayloadBytes {
    /// Zero-copy: pointer into Sample's ZBuf memory
    Borrowed { ptr: *const u8, len: usize },
    /// Fallback: owned Vec for non-contiguous buffers
    Owned(Vec<u8>),
}

// SAFETY: The pointer in Borrowed variant is valid as long as the Sample is alive,
// and we always keep the Sample in the same struct.
unsafe impl Send for PayloadBytes {}
unsafe impl Sync for PayloadBytes {}

/// Zero-copy view of a Zenoh payload via Python buffer protocol.
///
/// This class holds a Zenoh Sample and exposes its payload bytes through
/// Python's buffer protocol. Python code can access the data without copying:
///
/// ```python
/// payload = subscriber.recv_raw_view()
/// mv = memoryview(payload)  # Zero-copy view!
/// data = mv[offset:offset+length]  # Slicing is also zero-copy
///
/// # Or with numpy:
/// import numpy as np
/// arr = np.frombuffer(payload, dtype=np.uint8)  # numpy views ZBuf memory!
/// ```
#[pyclass(name = "ZPayloadView")]
pub struct ZPayloadView {
    /// The Sample that owns the ZBuf memory. Must not be dropped while buffer is in use.
    #[allow(dead_code)]
    sample: Sample,
    /// Cached pointer/length or owned copy of the bytes
    bytes: PayloadBytes,
}

impl ZPayloadView {
    /// Create a new ZPayloadView from a Sample.
    ///
    /// If the payload is contiguous, this stores a pointer (zero-copy).
    /// If fragmented, it copies to an owned Vec (rare case).
    pub fn new(sample: Sample) -> Self {
        let payload = sample.payload();
        let cow = payload.to_bytes();
        let bytes = match cow {
            Cow::Borrowed(b) => PayloadBytes::Borrowed {
                ptr: b.as_ptr(),
                len: b.len(),
            },
            Cow::Owned(v) => PayloadBytes::Owned(v),
        };
        Self { sample, bytes }
    }

    /// Get the payload as a byte slice.
    ///
    /// # Safety
    /// The returned slice is valid as long as `self` is alive.
    pub fn as_slice(&self) -> &[u8] {
        match &self.bytes {
            PayloadBytes::Borrowed { ptr, len } => {
                // SAFETY: ptr is valid as long as sample is alive, and we hold sample
                unsafe { std::slice::from_raw_parts(*ptr, *len) }
            }
            PayloadBytes::Owned(v) => v.as_slice(),
        }
    }

    /// Check if this view is zero-copy (borrowed) or had to copy (owned).
    pub fn is_zero_copy(&self) -> bool {
        matches!(self.bytes, PayloadBytes::Borrowed { .. })
    }
}

#[pymethods]
impl ZPayloadView {
    /// Return the length of the payload in bytes.
    fn __len__(&self) -> usize {
        self.as_slice().len()
    }

    /// Return True if the payload is empty.
    fn __bool__(&self) -> bool {
        !self.as_slice().is_empty()
    }

    /// Check if this view achieved zero-copy.
    ///
    /// Returns True if the payload was contiguous and no copy was needed.
    /// Returns False if the payload was fragmented and had to be copied.
    #[getter]
    fn is_zero_copy_py(&self) -> bool {
        self.is_zero_copy()
    }

    /// Implement Python buffer protocol - get buffer.
    ///
    /// This is called when Python code does `memoryview(payload)` or
    /// `numpy.frombuffer(payload)`.
    ///
    /// # Safety
    /// The view pointer is valid as long as the ZPayloadView Python object is alive.
    /// We ensure this by setting `view.obj` to the Python object reference.
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe fn __getbuffer__(
        slf: Bound<'_, Self>,
        view: *mut ffi::Py_buffer,
        flags: c_int,
    ) -> PyResult<()> {
        if view.is_null() {
            return Err(PyBufferError::new_err("View is null"));
        }
        if (flags & ffi::PyBUF_WRITABLE) == ffi::PyBUF_WRITABLE {
            return Err(PyBufferError::new_err("Object is not writable"));
        }

        let binding = slf.borrow();
        let bytes = binding.as_slice();

        unsafe {
            // Set obj to keep the ZPayloadView alive while buffer is in use
            (*view).obj = slf.into_any().into_ptr();

            // Set buffer pointer and length
            (*view).buf = bytes.as_ptr() as *mut c_void;
            (*view).len = bytes.len() as isize;
            (*view).readonly = 1;
            (*view).itemsize = 1;

            // Format string: "B" for unsigned bytes
            (*view).format = if (flags & ffi::PyBUF_FORMAT) == ffi::PyBUF_FORMAT {
                CString::new("B").unwrap().into_raw()
            } else {
                ptr::null_mut()
            };

            // 1-dimensional array
            (*view).ndim = 1;
            (*view).shape = if (flags & ffi::PyBUF_ND) == ffi::PyBUF_ND {
                &mut (*view).len
            } else {
                ptr::null_mut()
            };

            (*view).strides = if (flags & ffi::PyBUF_STRIDES) == ffi::PyBUF_STRIDES {
                &mut (*view).itemsize
            } else {
                ptr::null_mut()
            };

            (*view).suboffsets = ptr::null_mut();
            (*view).internal = ptr::null_mut();
        }
        Ok(())
    }

    /// Implement Python buffer protocol - release buffer.
    ///
    /// Called when the buffer view is released. We need to free the format string
    /// that was allocated in __getbuffer__.
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe fn __releasebuffer__(&self, view: *mut ffi::Py_buffer) {
        unsafe {
            if !(*view).format.is_null() {
                drop(CString::from_raw((*view).format));
            }
        }
    }
}
