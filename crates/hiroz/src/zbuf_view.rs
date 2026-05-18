//! Zero-copy ZBuf view for Python buffer protocol.
//!
//! This provides a way to expose ZBuf data to Python without copying,
//! by implementing Python's buffer protocol.

// PyO3 0.22 generates code that calls unsafe functions in edition 2024.
// This is safe because PyO3's generated code handles the safety invariants.
#![allow(unsafe_op_in_unsafe_fn)]

use pyo3::exceptions::{PyBufferError, PyIndexError, PyTypeError};
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::PySlice;
use std::borrow::Cow;
use std::ffi::{CString, c_int, c_void};
use std::ptr;
use zenoh_buffers::buffer::SplitBuffer;

/// Stores either a borrowed pointer or owned Vec
enum BufferData {
    /// Zero-copy: pointer to contiguous ZBuf memory
    Borrowed { ptr: *const u8, len: usize },
    /// Fallback: owned Vec for non-contiguous buffers
    Owned(Vec<u8>),
}

unsafe impl Send for BufferData {}
unsafe impl Sync for BufferData {}

/// Zero-copy view of a ZBuf via Python buffer protocol.
///
/// This class holds a ZBuf and exposes its bytes through Python's buffer protocol.
/// Python code can access the data without copying via memoryview():
///
/// ```python
/// view = ZBufView(zbuf)
/// mv = memoryview(view)  # Zero-copy!
/// data = bytes(mv)  # Only copies when needed
/// ```
#[pyclass(name = "ZBufView")]
pub struct ZBufView {
    /// The ZBuf that owns the memory. Must not be dropped while buffer is in use.
    #[allow(dead_code)]
    zbuf: crate::ZBuf,
    /// Cached pointer/length or owned copy of the bytes
    data: BufferData,
}

impl ZBufView {
    /// Create a new ZBufView from a ZBuf.
    ///
    /// If the ZBuf is contiguous, this stores a pointer (zero-copy).
    /// If fragmented, it copies to an owned Vec.
    pub fn new(zbuf: crate::ZBuf) -> Self {
        let cow: Cow<[u8]> = zbuf.0.contiguous();
        let data = match cow {
            Cow::Borrowed(b) => BufferData::Borrowed {
                ptr: b.as_ptr(),
                len: b.len(),
            },
            Cow::Owned(v) => BufferData::Owned(v),
        };
        Self { zbuf, data }
    }

    /// Get the data as a byte slice.
    ///
    /// # Safety
    /// The returned slice is valid as long as `self` is alive.
    pub fn as_slice(&self) -> &[u8] {
        match &self.data {
            BufferData::Borrowed { ptr, len } => {
                // SAFETY: ptr is valid as long as zbuf is alive, and we hold zbuf
                unsafe { std::slice::from_raw_parts(*ptr, *len) }
            }
            BufferData::Owned(v) => v.as_slice(),
        }
    }

    /// Check if this view is zero-copy (borrowed) or had to copy (owned).
    pub fn is_zero_copy(&self) -> bool {
        matches!(self.data, BufferData::Borrowed { .. })
    }

    /// Get a reference to the inner ZBuf.
    /// Clone is cheap (ref-counted ZSlices), enabling zero-copy re-publish.
    pub fn zbuf(&self) -> &crate::ZBuf {
        &self.zbuf
    }
}

#[pymethods]
impl ZBufView {
    /// Return the length in bytes.
    fn __len__(&self) -> usize {
        self.as_slice().len()
    }

    /// Return True if not empty.
    fn __bool__(&self) -> bool {
        !self.as_slice().is_empty()
    }

    /// Check if this view achieved zero-copy.
    #[getter]
    fn is_zero_copy_py(&self) -> bool {
        self.is_zero_copy()
    }

    /// Support subscript access: view[i] and view[start:stop]
    fn __getitem__(&self, py: Python, key: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let slice = self.as_slice();
        let len = slice.len() as isize;

        if let Ok(py_slice) = key.downcast::<PySlice>() {
            let indices = py_slice.indices(len)?;
            let start = indices.start as usize;
            let stop = indices.stop as usize;
            let step = indices.step;
            if step == 1 {
                Ok(pyo3::types::PyBytes::new_bound(py, &slice[start..stop])
                    .into_any()
                    .unbind())
            } else {
                let result: Vec<u8> = (0..indices.slicelength)
                    .map(|i| slice[(start as isize + i as isize * step) as usize])
                    .collect();
                Ok(pyo3::types::PyBytes::new_bound(py, &result)
                    .into_any()
                    .unbind())
            }
        } else if let Ok(idx) = key.extract::<isize>() {
            let actual = if idx < 0 { len + idx } else { idx };
            if actual < 0 || actual >= len {
                Err(PyIndexError::new_err("index out of range"))
            } else {
                Ok(slice[actual as usize].into_py(py))
            }
        } else {
            Err(PyTypeError::new_err("indices must be integers or slices"))
        }
    }

    /// Implement Python buffer protocol - get buffer.
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
            (*view).obj = slf.into_any().into_ptr();
            (*view).buf = bytes.as_ptr() as *mut c_void;
            (*view).len = bytes.len() as isize;
            (*view).readonly = 1;
            (*view).itemsize = 1;

            (*view).format = if (flags & ffi::PyBUF_FORMAT) == ffi::PyBUF_FORMAT {
                CString::new("B").unwrap().into_raw()
            } else {
                ptr::null_mut()
            };

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
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe fn __releasebuffer__(&self, view: *mut ffi::Py_buffer) {
        unsafe {
            if !(*view).format.is_null() {
                drop(CString::from_raw((*view).format));
            }
        }
    }
}
