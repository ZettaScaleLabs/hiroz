use hiroz::{msg::ZMessage, pubsub::ZPub};
use hiroz_msgs::std_msgs::String as RosString;

type Pub = ZPub<RosString, <RosString as ZMessage>::Serdes>;

fn owned(p: &Pub, m: RosString) {
    let _ = p.publish(m);
}

fn main() {}
