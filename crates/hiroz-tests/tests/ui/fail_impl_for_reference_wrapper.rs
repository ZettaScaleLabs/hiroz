use hiroz::{
    local_bus::Published,
    msg::ZSerializer,
    pubsub::{Publishable, ZPub},
};
use hiroz_msgs::std_msgs::String as RosString;

struct Mine;

/// `&Mine` is local to this crate, so the orphan rule lets it implement a
/// foreign trait. The seal must still refuse: a reference to a downstream type
/// is not a route `publish` defines.
impl<S: ZSerializer> Publishable<RosString, S> for &Mine {
    fn publish_to(self, _: &ZPub<RosString, S>) -> hiroz::Result<Published> {
        unimplemented!()
    }
}

fn main() {}
