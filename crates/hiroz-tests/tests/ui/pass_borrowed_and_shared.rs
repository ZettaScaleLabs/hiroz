use std::sync::Arc;

use hiroz::{
    local_bus::Published,
    msg::ZMessage,
    pubsub::ZPub,
};
use hiroz_msgs::std_msgs::String as RosString;

type Pub = ZPub<RosString, <RosString as ZMessage>::Serdes>;

fn borrowed(p: &Pub, m: &RosString) -> hiroz::Result<Published> {
    p.publish(m)
}

fn shared(p: &Pub, m: Arc<RosString>) -> hiroz::Result<Published> {
    p.publish(m)
}

fn main() {}
