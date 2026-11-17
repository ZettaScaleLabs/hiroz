use std::sync::Arc;

use hiroz::{msg::ZMessage, pubsub::ZPub};
use hiroz_msgs::std_msgs::String as RosString;

type Pub = ZPub<RosString, <RosString as ZMessage>::Serdes>;

fn ref_to_arc(p: &Pub, m: &Arc<RosString>) {
    let _ = p.publish(m);
}

fn main() {}
