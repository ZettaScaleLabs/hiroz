use hiroz::{
    MessageTypeInfo,
    entity::TypeHash,
    msg::{ZMessage, ZSerializer},
};
use hiroz_cdr::{
    BigEndian, CdrBuffer, CdrDeserialize, CdrReader, CdrSerialize, CdrSerializedSize, CdrWriter,
    LittleEndian, cdr_to_vec,
};
use hiroz_msgs::std_msgs::Empty;
use hiroz_msgs::std_srvs::{EmptyRequest, EmptyResponse};

#[derive(Debug)]
struct EmptyThenByte {
    empty: Empty,
    tail: u8,
}

impl CdrSerialize for EmptyThenByte {
    fn cdr_serialize<BO: byteorder::ByteOrder, B: CdrBuffer>(
        &self,
        writer: &mut CdrWriter<'_, BO, B>,
    ) {
        self.empty.cdr_serialize(writer);
        self.tail.cdr_serialize(writer);
    }
}

impl CdrDeserialize for EmptyThenByte {
    fn cdr_deserialize<'de, BO: byteorder::ByteOrder>(
        reader: &mut CdrReader<'de, BO>,
    ) -> hiroz_cdr::Result<Self> {
        Ok(Self {
            empty: Empty::cdr_deserialize(reader)?,
            tail: u8::cdr_deserialize(reader)?,
        })
    }
}

#[test]
fn generated_empty_matches_ros_payload() {
    let message = Empty::default();
    assert_eq!(cdr_to_vec(&message, 1), [0]);
    assert_eq!(
        <<Empty as ZMessage>::Serdes as ZSerializer>::serialize(&message),
        [0, 1, 0, 0, 0]
    );
    assert_eq!(message.cdr_serialized_size(0), 1);

    let mut reader = CdrReader::<LittleEndian>::new(&[0]);
    Empty::cdr_deserialize(&mut reader).unwrap();
    assert_eq!(reader.remaining(), 0);
    assert_eq!(
        Empty::type_hash(),
        TypeHash::from_rihs_string(
            "RIHS01_20b625256f32d5dbc0d04fee44f43c41e51c70d3502f84b4a08e7a9c26a96312"
        )
        .unwrap()
    );
}

#[test]
fn generated_empty_advances_nested_payload() {
    let message = EmptyThenByte {
        empty: Empty::default(),
        tail: 0x7f,
    };
    let bytes = cdr_to_vec(&message, 2);
    assert_eq!(bytes, [0, 0x7f]);

    let mut reader = CdrReader::<LittleEndian>::new(&bytes);
    let decoded = EmptyThenByte::cdr_deserialize(&mut reader).unwrap();
    assert_eq!(decoded.tail, message.tail);
    assert_eq!(reader.remaining(), 0);

    let mut reader = CdrReader::<BigEndian>::new(&bytes);
    let decoded = EmptyThenByte::cdr_deserialize(&mut reader).unwrap();
    assert_eq!(decoded.tail, message.tail);
    assert_eq!(reader.remaining(), 0);
}

#[test]
fn generated_empty_collections_serialize_each_element() {
    let fixed = [Empty::default(), Empty::default()];
    assert_eq!(cdr_to_vec(&fixed, 2), [0, 0]);

    let sequence = vec![Empty::default(), Empty::default()];
    assert_eq!(cdr_to_vec(&sequence, 6), [2, 0, 0, 0, 0, 0]);
}

#[test]
fn generated_empty_service_parts_use_the_synthetic_byte() {
    let request = EmptyRequest::default();
    let response = EmptyResponse::default();
    assert_eq!(cdr_to_vec(&request, 1), [0]);
    assert_eq!(cdr_to_vec(&response, 1), [0]);
    assert_eq!(request.cdr_serialized_size(0), 1);
    assert_eq!(response.cdr_serialized_size(0), 1);
}
