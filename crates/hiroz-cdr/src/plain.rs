//! `CdrPlain` marker trait for types whose CDR wire layout equals their memory layout.
//!
//! When a type is `CdrPlain`, sequences of that type can be serialized and deserialized
//! with a single bulk memcpy instead of element-by-element encoding.
//!
//! # Safety invariants
//!
//! A type `T: CdrPlain` must satisfy **all** of:
//! 1. No padding bytes — `bytemuck::Pod` guarantees this at compile time.
//! 2. CDR wire layout == in-memory layout on little-endian hosts. For all ROS primitive
//!    numeric types this holds: CDR encodes them in native byte order (LE) without
//!    reordering fields or adding framing.
//! 3. Every possible bit pattern is a valid `T` — again `bytemuck::Pod`.
//!
//! This trait is only defined on little-endian targets because CDR uses little-endian
//! encoding for all primitive types. On a big-endian host the wire bytes would need
//! byte-swapping per element, making the bulk-copy path incorrect.

/// Marker trait for types whose CDR serialized form is identical to their in-memory
/// representation on little-endian hosts.
///
/// # Safety
/// Implementors must guarantee that:
/// - The type has no padding bytes.
/// - The CDR wire layout of the type matches its in-memory layout (true for all ROS
///   numeric primitives on LE hosts).
/// - Every possible bit pattern is a valid value (i.e. the type is `bytemuck::Pod`).
///
/// The `bytemuck::Pod` bound is enforced at the usage sites (`write_pod_slice`,
/// `read_pod_slice`) rather than here so that blanket impls for `[T; N]` can be
/// expressed — `bytemuck::Pod` is only impl'd for fixed array sizes up to 64.
///
/// This trait should only be implemented by codegen for generated message types, or
/// manually for well-known primitive types defined in this crate.
#[cfg(target_endian = "little")]
pub unsafe trait CdrPlain: Copy + 'static {}

// ── Primitive impls ──────────────────────────────────────────────────────────
// bool is excluded: bytemuck::Pod is not impl'd for bool (only 0/1 are valid).
// char is excluded: Rust char is 4-byte Unicode; CDR wchar is 2 bytes.

#[cfg(target_endian = "little")]
unsafe impl CdrPlain for f32 {}
#[cfg(target_endian = "little")]
unsafe impl CdrPlain for f64 {}
#[cfg(target_endian = "little")]
unsafe impl CdrPlain for i8 {}
#[cfg(target_endian = "little")]
unsafe impl CdrPlain for u8 {}
#[cfg(target_endian = "little")]
unsafe impl CdrPlain for i16 {}
#[cfg(target_endian = "little")]
unsafe impl CdrPlain for u16 {}
#[cfg(target_endian = "little")]
unsafe impl CdrPlain for i32 {}
#[cfg(target_endian = "little")]
unsafe impl CdrPlain for u32 {}
#[cfg(target_endian = "little")]
unsafe impl CdrPlain for i64 {}
#[cfg(target_endian = "little")]
unsafe impl CdrPlain for u64 {}

// Fixed arrays of plain types are themselves plain.
#[cfg(target_endian = "little")]
unsafe impl<T: CdrPlain, const N: usize> CdrPlain for [T; N] {}
