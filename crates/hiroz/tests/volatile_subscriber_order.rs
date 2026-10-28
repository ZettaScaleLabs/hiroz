#![cfg(feature = "ffi")]

use std::{sync::mpsc, time::Duration};

use hiroz::{Builder, context::ZContextBuilder, msg::ZMessage, ros_msg::MessageTypeInfo};
use hiroz_msgs::std_msgs::String as RosString;
use serde_json::json;
use zenoh::{
    Wait,
    time::{NTP64, Timestamp, TimestampId},
};

// A session shares its timestamp ID across publishers. Concurrent writes can
// arrive out of timestamp order; volatile subscriptions must deliver each sample
// without treating the older timestamp (or an equal one) as a history duplicate.
#[test]
fn volatile_typed_and_raw_subscribers_preserve_out_of_order_timestamps() {
    let context = ZContextBuilder::default()
        .disable_multicast_scouting()
        .with_json("listen/endpoints", json!([]))
        .with_json("scouting/gossip/enabled", json!(false))
        .build()
        .unwrap();
    let node = context
        .create_node("volatile_timestamp_order")
        .build()
        .unwrap();
    let (typed_tx, typed_rx) = mpsc::channel();
    let typed = node
        .create_sub::<RosString>("/timestamp_order")
        .build_with_callback(move |message: RosString| {
            typed_tx.send(message.data).unwrap();
        })
        .unwrap();
    let (raw_tx, raw_rx) = mpsc::channel();
    let raw = node
        .create_raw_subscriber(
            "/timestamp_order",
            RosString::type_name(),
            &RosString::type_hash().to_rihs_string(),
            move |bytes| {
                let message = RosString::deserialize(bytes).unwrap();
                raw_tx.send(message.data).unwrap();
            },
        )
        .unwrap();
    let publisher = node
        .create_raw_publisher(
            "/timestamp_order",
            RosString::type_name(),
            &RosString::type_hash().to_rihs_string(),
        )
        .unwrap();
    let id = TimestampId::try_from([42_u8]).unwrap();
    for (time, data) in [(2, "newer"), (1, "older"), (2, "equal")] {
        publisher
            .inner
            .put(RosString { data: data.into() }.serialize())
            .timestamp(Timestamp::new(NTP64(time), id))
            .wait()
            .unwrap();
    }
    let receive = |rx: mpsc::Receiver<String>| {
        let mut messages = Vec::new();
        while messages.len() < 3 {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(message) => messages.push(message),
                Err(_) => break,
            }
        }
        messages
    };
    let typed_messages = receive(typed_rx);
    let raw_messages = receive(raw_rx);
    drop((typed, raw));
    assert_eq!(
        (typed_messages, raw_messages),
        (
            vec!["newer".to_owned(), "older".to_owned(), "equal".to_owned()],
            vec!["newer".to_owned(), "older".to_owned(), "equal".to_owned()]
        )
    );
}
