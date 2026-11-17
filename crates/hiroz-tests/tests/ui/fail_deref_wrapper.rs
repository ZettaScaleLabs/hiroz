use std::ops::Deref;

use hiroz::{msg::ZMessage, pubsub::ZPub};
use hiroz_msgs::std_msgs::String as RosString;

type Pub = ZPub<RosString, <RosString as ZMessage>::Serdes>;

/// A downstream reference wrapper. Deref coercion does not reach a generic
/// argument, so the wrapper is refused rather than routed.
struct Wrapper<'a>(&'a RosString);

impl Deref for Wrapper<'_> {
    type Target = RosString;
    fn deref(&self) -> &RosString {
        self.0
    }
}

fn wrapped(p: &Pub, w: &Wrapper<'_>) {
    let _ = p.publish(w);
}

fn main() {}
