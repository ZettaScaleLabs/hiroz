//! Python bridge traits for ROS message conversion
//!
//! These traits provide a type-safe interface for converting between Python
//! msgspec objects and Rust ROS message types. They are designed to work with
//! the derive macros in `hiroz-derive`.
//!
//! # Example
//!
//! ```ignore
//! use hiroz::python_bridge::{FromPyMessage, IntoPyMessage};
//! use pyo3::prelude::*;
//!
//! // Extract Rust message from Python object
//! let rust_msg = MyMessage::from_py(&py_obj)?;
//!
//! // Convert Rust message to Python object
//! let py_obj = rust_msg.into_py_message(py)?;
//! ```

#[cfg(feature = "python")]
use pyo3::prelude::*;

/// Trait for extracting Rust messages from Python objects
///
/// This trait is automatically implemented by the `#[derive(FromPyMessage)]` macro.
/// It handles arbitrary nesting depth by recursively calling `from_py()` for nested types.
#[cfg(feature = "python")]
pub trait FromPyMessage: Sized {
    /// Extract a Rust message from a Python object
    ///
    /// # Arguments
    /// * `obj` - Reference to a Python object (typically a msgspec.Struct)
    ///
    /// # Returns
    /// * `Ok(Self)` - Successfully extracted Rust message
    /// * `Err(PyErr)` - Extraction failed (type mismatch, missing field, etc.)
    fn from_py(obj: &Bound<'_, PyAny>) -> PyResult<Self>;
}

/// Trait for constructing Python objects from Rust messages
///
/// This trait is automatically implemented by the `#[derive(IntoPyMessage)]` macro.
/// It handles arbitrary nesting depth by recursively calling `into_py_message()` for nested types.
#[cfg(feature = "python")]
#[allow(clippy::wrong_self_convention)]
pub trait IntoPyMessage {
    /// Convert a Rust message to a Python object
    ///
    /// # Arguments
    /// * `py` - Python interpreter handle
    ///
    /// # Returns
    /// * `Ok(PyObject)` - Successfully constructed Python object
    /// * `Err(PyErr)` - Construction failed (module not found, etc.)
    fn into_py_message(&self, py: Python) -> PyResult<PyObject>;
}

// Implement FromPyMessage for primitive types
#[cfg(feature = "python")]
macro_rules! impl_from_py_primitive {
    ($($ty:ty),*) => {
        $(
            impl FromPyMessage for $ty {
                fn from_py(obj: &Bound<'_, PyAny>) -> PyResult<Self> {
                    obj.extract()
                }
            }
        )*
    };
}

#[cfg(feature = "python")]
impl_from_py_primitive!(bool, i8, u8, i16, u16, i32, u32, i64, u64, f32, f64);

#[cfg(feature = "python")]
impl FromPyMessage for String {
    fn from_py(obj: &Bound<'_, PyAny>) -> PyResult<Self> {
        obj.extract()
    }
}

// Specialized implementation for Vec<u8> using buffer protocol (zero-copy when possible)
#[cfg(feature = "python")]
impl FromPyMessage for Vec<u8> {
    fn from_py(obj: &Bound<'_, PyAny>) -> PyResult<Self> {
        // Try bytes/bytearray first (fast path - buffer protocol)
        if let Ok(bytes) = obj.downcast::<pyo3::types::PyBytes>() {
            return Ok(bytes.as_bytes().to_vec());
        }
        if let Ok(bytearray) = obj.downcast::<pyo3::types::PyByteArray>() {
            // SAFETY: We immediately copy the data, so no concurrent mutation issues
            return Ok(unsafe { bytearray.as_bytes() }.to_vec());
        }
        // Fallback: extract as sequence (slow path for lists)
        obj.extract()
    }
}

// Implement FromPyMessage for Vec<T> where T: FromPyMessage (excluding Vec<u8>)
#[cfg(feature = "python")]
impl<T: FromPyMessage + FromPyMessageNotU8> FromPyMessage for Vec<T> {
    fn from_py(obj: &Bound<'_, PyAny>) -> PyResult<Self> {
        let mut result = Vec::new();
        for item in obj.iter()? {
            result.push(T::from_py(&item?)?);
        }
        Ok(result)
    }
}

// Marker trait to exclude u8 from generic Vec<T> impl (to avoid conflicting impls)
#[cfg(feature = "python")]
pub trait FromPyMessageNotU8 {}
#[cfg(feature = "python")]
impl FromPyMessageNotU8 for bool {}
#[cfg(feature = "python")]
impl FromPyMessageNotU8 for i8 {}
#[cfg(feature = "python")]
impl FromPyMessageNotU8 for i16 {}
#[cfg(feature = "python")]
impl FromPyMessageNotU8 for u16 {}
#[cfg(feature = "python")]
impl FromPyMessageNotU8 for i32 {}
#[cfg(feature = "python")]
impl FromPyMessageNotU8 for u32 {}
#[cfg(feature = "python")]
impl FromPyMessageNotU8 for i64 {}
#[cfg(feature = "python")]
impl FromPyMessageNotU8 for u64 {}
#[cfg(feature = "python")]
impl FromPyMessageNotU8 for f32 {}
#[cfg(feature = "python")]
impl FromPyMessageNotU8 for f64 {}
#[cfg(feature = "python")]
impl FromPyMessageNotU8 for String {}
#[cfg(feature = "python")]
impl<T> FromPyMessageNotU8 for Vec<T> {}
#[cfg(feature = "python")]
impl<T, const N: usize> FromPyMessageNotU8 for [T; N] {}

// Implement IntoPyMessage for primitive types
#[cfg(feature = "python")]
macro_rules! impl_into_py_primitive {
    ($($ty:ty),*) => {
        $(
            impl IntoPyMessage for $ty {
                fn into_py_message(&self, py: Python) -> PyResult<PyObject> {
                    use pyo3::ToPyObject;
                    Ok(self.to_object(py))
                }
            }
        )*
    };
}

#[cfg(feature = "python")]
impl_into_py_primitive!(bool, i8, u8, i16, u16, i32, u32, i64, u64, f32, f64);

#[cfg(feature = "python")]
impl IntoPyMessage for String {
    fn into_py_message(&self, py: Python) -> PyResult<PyObject> {
        use pyo3::ToPyObject;
        Ok(self.to_object(py))
    }
}

// Specialized implementation for Vec<u8> - output as bytes for performance
#[cfg(feature = "python")]
impl IntoPyMessage for Vec<u8> {
    fn into_py_message(&self, py: Python) -> PyResult<PyObject> {
        Ok(pyo3::types::PyBytes::new_bound(py, self).into())
    }
}

// Implement IntoPyMessage for Vec<T> where T: IntoPyMessage (excluding Vec<u8>)
#[cfg(feature = "python")]
impl<T: IntoPyMessage + IntoPyMessageNotU8> IntoPyMessage for Vec<T> {
    fn into_py_message(&self, py: Python) -> PyResult<PyObject> {
        let list = pyo3::types::PyList::empty_bound(py);
        for item in self {
            list.append(item.into_py_message(py)?)?;
        }
        Ok(list.into())
    }
}

// Marker trait to exclude u8 from generic Vec<T> impl
#[cfg(feature = "python")]
pub trait IntoPyMessageNotU8 {}
#[cfg(feature = "python")]
impl IntoPyMessageNotU8 for bool {}
#[cfg(feature = "python")]
impl IntoPyMessageNotU8 for i8 {}
#[cfg(feature = "python")]
impl IntoPyMessageNotU8 for i16 {}
#[cfg(feature = "python")]
impl IntoPyMessageNotU8 for u16 {}
#[cfg(feature = "python")]
impl IntoPyMessageNotU8 for i32 {}
#[cfg(feature = "python")]
impl IntoPyMessageNotU8 for u32 {}
#[cfg(feature = "python")]
impl IntoPyMessageNotU8 for i64 {}
#[cfg(feature = "python")]
impl IntoPyMessageNotU8 for u64 {}
#[cfg(feature = "python")]
impl IntoPyMessageNotU8 for f32 {}
#[cfg(feature = "python")]
impl IntoPyMessageNotU8 for f64 {}
#[cfg(feature = "python")]
impl IntoPyMessageNotU8 for String {}
#[cfg(feature = "python")]
impl<T> IntoPyMessageNotU8 for Vec<T> {}
#[cfg(feature = "python")]
impl<T, const N: usize> IntoPyMessageNotU8 for [T; N] {}

// Re-export for non-python builds (empty module)
#[cfg(not(feature = "python"))]
pub trait FromPyMessage: Sized {}

#[cfg(not(feature = "python"))]
pub trait IntoPyMessage {}
