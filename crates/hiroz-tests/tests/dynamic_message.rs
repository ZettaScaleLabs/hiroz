//! Tests for the dynamic message examples.
//!
//! Each example file is included directly as a module via `#[path]` so the
//! logic runs in-process without spawning a subprocess.

#[allow(dead_code)]
#[path = "../../hiroz/examples/dynamic_message/basic.rs"]
mod dynamic_message_basic;

#[allow(dead_code)]
#[path = "../../hiroz/examples/dynamic_message/serialization.rs"]
mod dynamic_message_serialization;

#[cfg(feature = "ros-msgs")]
#[allow(dead_code)]
#[path = "../../hiroz/examples/dynamic_message/interop.rs"]
mod dynamic_message_interop;

#[test]
fn test_dynamic_message_basic() {
    dynamic_message_basic::run().expect("dynamic_message_basic failed");
}

#[test]
fn test_dynamic_message_serialization() {
    dynamic_message_serialization::run().expect("dynamic_message_serialization failed");
}

#[cfg(feature = "ros-msgs")]
#[test]
fn test_dynamic_message_interop() {
    dynamic_message_interop::run().expect("dynamic_message_interop failed");
}
