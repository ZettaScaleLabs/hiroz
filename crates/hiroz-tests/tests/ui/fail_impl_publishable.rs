use hiroz::{
    local_bus::Published,
    msg::{ZMessage, ZSerializer},
    pubsub::{Publishable, ZPub},
};

struct Mine;

impl<T: ZMessage, S: ZSerializer> Publishable<T, S> for Mine {
    fn publish_to(self, _: &ZPub<T, S>) -> hiroz::Result<Published> {
        unimplemented!()
    }
}

fn main() {}
