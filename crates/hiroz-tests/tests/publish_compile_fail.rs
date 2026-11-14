//! What `ZPub::publish` refuses to compile.
//!
//! `publish` takes `&T` (the wire) or `Arc<T>` (the publisher's route). Every other
//! argument must be a compile error rather than a silent route, and `Publishable`
//! must stay closed to downstream implementations. The `pass` cases are the
//! positive control: they use the same imports and publisher type, so a
//! `compile_fail` case cannot be failing for an unrelated reason.
#![cfg(feature = "ros-msgs")]

#[test]
fn publish_accepts_only_borrowed_and_shared_messages() {
    let t = trybuild::TestCases::new();
    t.pass("tests/ui/pass_borrowed_and_shared.rs");
    t.compile_fail("tests/ui/fail_impl_publishable.rs");
    t.compile_fail("tests/ui/fail_owned_message.rs");
    t.compile_fail("tests/ui/fail_ref_to_arc.rs");
    t.compile_fail("tests/ui/fail_deref_wrapper.rs");
}
