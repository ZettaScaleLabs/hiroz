//! Tests for DynamicValue and conversion traits.

use crate::dynamic::value::{DynamicValue, FromDynamic, IntoDynamic};

#[test]
fn test_into_dynamic_primitives() {
    assert_eq!(true.into_dynamic(), DynamicValue::Bool(true));
    assert_eq!(false.into_dynamic(), DynamicValue::Bool(false));

    assert_eq!(42i8.into_dynamic(), DynamicValue::Int8(42));
    assert_eq!(42i16.into_dynamic(), DynamicValue::Int16(42));
    assert_eq!(42i32.into_dynamic(), DynamicValue::Int32(42));
    assert_eq!(42i64.into_dynamic(), DynamicValue::Int64(42));

    assert_eq!(42u8.into_dynamic(), DynamicValue::Uint8(42));
    assert_eq!(42u16.into_dynamic(), DynamicValue::Uint16(42));
    assert_eq!(42u32.into_dynamic(), DynamicValue::Uint32(42));
    assert_eq!(42u64.into_dynamic(), DynamicValue::Uint64(42));

    assert_eq!(1.5f32.into_dynamic(), DynamicValue::Float32(1.5));
    assert_eq!(1.5f64.into_dynamic(), DynamicValue::Float64(1.5));

    assert_eq!(
        "hello".into_dynamic(),
        DynamicValue::String("hello".to_string())
    );
    assert_eq!(
        "world".to_string().into_dynamic(),
        DynamicValue::String("world".to_string())
    );
}

#[test]
fn test_from_dynamic_primitives() {
    assert_eq!(bool::from_dynamic(&DynamicValue::Bool(true)), Some(true));
    assert_eq!(bool::from_dynamic(&DynamicValue::Bool(false)), Some(false));

    assert_eq!(i8::from_dynamic(&DynamicValue::Int8(42)), Some(42));
    assert_eq!(i16::from_dynamic(&DynamicValue::Int16(42)), Some(42));
    assert_eq!(i32::from_dynamic(&DynamicValue::Int32(42)), Some(42));
    assert_eq!(i64::from_dynamic(&DynamicValue::Int64(42)), Some(42));

    assert_eq!(u8::from_dynamic(&DynamicValue::Uint8(42)), Some(42));
    assert_eq!(u16::from_dynamic(&DynamicValue::Uint16(42)), Some(42));
    assert_eq!(u32::from_dynamic(&DynamicValue::Uint32(42)), Some(42));
    assert_eq!(u64::from_dynamic(&DynamicValue::Uint64(42)), Some(42));

    assert_eq!(f32::from_dynamic(&DynamicValue::Float32(1.5)), Some(1.5));
    assert_eq!(f64::from_dynamic(&DynamicValue::Float64(1.5)), Some(1.5));

    assert_eq!(
        String::from_dynamic(&DynamicValue::String("hello".to_string())),
        Some("hello".to_string())
    );
}

#[test]
fn test_from_dynamic_type_mismatch() {
    // Trying to extract wrong type should return None
    assert_eq!(i32::from_dynamic(&DynamicValue::Bool(true)), None);
    assert_eq!(bool::from_dynamic(&DynamicValue::Int32(42)), None);
    assert_eq!(String::from_dynamic(&DynamicValue::Float64(1.5)), None);
    assert_eq!(
        f64::from_dynamic(&DynamicValue::String("hello".to_string())),
        None
    );
}

#[test]
fn test_vec_into_dynamic() {
    let v = vec![1i32, 2, 3];
    let dv = v.into_dynamic();
    match dv {
        DynamicValue::Array(arr) => {
            assert_eq!(arr.len(), 3);
            assert_eq!(arr[0], DynamicValue::Int32(1));
            assert_eq!(arr[1], DynamicValue::Int32(2));
            assert_eq!(arr[2], DynamicValue::Int32(3));
        }
        _ => panic!("Expected Array"),
    }
}

#[test]
fn test_nested_vec_into_dynamic() {
    let v = vec![vec![1i32, 2], vec![3, 4]];
    let dv = v.into_dynamic();
    match dv {
        DynamicValue::Array(outer) => {
            assert_eq!(outer.len(), 2);
            match &outer[0] {
                DynamicValue::Array(inner) => {
                    assert_eq!(inner.len(), 2);
                    assert_eq!(inner[0], DynamicValue::Int32(1));
                }
                _ => panic!("Expected inner Array"),
            }
        }
        _ => panic!("Expected outer Array"),
    }
}

#[test]
fn test_dynamic_value_accessors() {
    let bool_val = DynamicValue::Bool(true);
    assert_eq!(bool_val.as_bool(), Some(true));
    assert_eq!(bool_val.as_i32(), None);

    let int_val = DynamicValue::Int32(42);
    assert_eq!(int_val.as_i32(), Some(42));
    assert_eq!(int_val.as_bool(), None);

    let string_val = DynamicValue::String("hello".to_string());
    assert_eq!(string_val.as_str(), Some("hello"));
    assert_eq!(string_val.as_i32(), None);

    let bytes_val = DynamicValue::Bytes(vec![1, 2, 3]);
    assert_eq!(bytes_val.as_bytes(), Some(&[1u8, 2, 3][..]));

    let array_val = DynamicValue::Array(vec![DynamicValue::Int32(1)]);
    assert!(array_val.as_array().is_some());
    assert_eq!(array_val.as_array().unwrap().len(), 1);
}

#[test]
fn test_dynamic_value_is_primitive() {
    assert!(DynamicValue::Bool(true).is_primitive());
    assert!(DynamicValue::Int32(42).is_primitive());
    assert!(DynamicValue::Float64(1.5).is_primitive());
    assert!(DynamicValue::String("hello".to_string()).is_primitive());

    assert!(!DynamicValue::Array(vec![]).is_primitive());
    assert!(!DynamicValue::Bytes(vec![]).is_primitive());
}

#[test]
fn test_dynamic_value_equality() {
    assert_eq!(DynamicValue::Bool(true), DynamicValue::Bool(true));
    assert_ne!(DynamicValue::Bool(true), DynamicValue::Bool(false));

    assert_eq!(DynamicValue::Int32(42), DynamicValue::Int32(42));
    assert_ne!(DynamicValue::Int32(42), DynamicValue::Int32(0));

    assert_eq!(
        DynamicValue::String("hello".to_string()),
        DynamicValue::String("hello".to_string())
    );

    // Different types are never equal
    assert_ne!(DynamicValue::Int32(1), DynamicValue::Int64(1));
}
