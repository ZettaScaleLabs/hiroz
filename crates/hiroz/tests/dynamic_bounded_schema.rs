use hiroz::dynamic::{
    FieldType, MessageSchema, MessageSchemaTypeDescription, type_description_msg_to_schema,
};

#[test]
fn bounded_string_descriptions_keep_ros_ids_and_collection_bounds() {
    let schema = MessageSchema::builder("test_msgs/msg/BoundedLabels")
        .field("label", FieldType::BoundedString(8))
        .field(
            "labels",
            FieldType::Sequence(Box::new(FieldType::BoundedString(12))),
        )
        .field(
            "fixed",
            FieldType::Array(Box::new(FieldType::BoundedString(5)), 3),
        )
        .build()
        .unwrap();
    let description = schema.to_type_description_msg().unwrap();
    assert_eq!(
        description.type_description.fields[0].field_type.type_id,
        21
    );
    assert_eq!(
        description.type_description.fields[1].field_type.type_id,
        165
    );
    assert_eq!(
        description.type_description.fields[1]
            .field_type
            .string_capacity,
        12
    );
    assert_eq!(
        description.type_description.fields[2].field_type.type_id,
        69
    );
    assert_eq!(
        description.type_description.fields[2].field_type.capacity,
        3
    );
    assert_eq!(
        description.type_description.fields[2]
            .field_type
            .string_capacity,
        5
    );
    let restored = type_description_msg_to_schema(&description).unwrap();
    for (before, after) in schema.fields.iter().zip(&restored.fields) {
        assert_eq!(before.field_type, after.field_type);
    }
    assert_eq!(
        schema.compute_type_hash().unwrap(),
        restored.compute_type_hash().unwrap()
    );
}

#[test]
fn dynamic_cdr_enforces_bounds_and_rejects_unbacked_lengths() {
    use hiroz::dynamic::DynamicMessage;
    let cases = [
        (
            FieldType::BoundedString(3),
            vec![0, 1, 0, 0, 5, 0, 0, 0, b'a', b'b', b'c', b'd', 0],
        ),
        (
            FieldType::BoundedSequence(Box::new(FieldType::Int16), 1),
            vec![0, 1, 0, 0, 2, 0, 0, 0, 1, 0, 2, 0],
        ),
        (
            FieldType::BoundedSequence(Box::new(FieldType::Uint8), 1),
            vec![0, 1, 0, 0, 2, 0, 0, 0, 1, 2],
        ),
        (
            FieldType::Sequence(Box::new(FieldType::Int64)),
            vec![0, 1, 0, 0, 128, 150, 152, 0],
        ),
        (
            FieldType::Array(Box::new(FieldType::Uint8), usize::MAX),
            vec![0, 1, 0, 0],
        ),
    ];
    for (field, bytes) in cases {
        let schema = MessageSchema::builder("test_msgs/msg/Invalid")
            .field("value", field)
            .build()
            .unwrap();
        assert!(DynamicMessage::from_cdr(&bytes, &schema).is_err());
    }
    let schema = MessageSchema::builder("test_msgs/msg/BoundedBytes")
        .field(
            "value",
            FieldType::BoundedSequence(Box::new(FieldType::Uint8), 2),
        )
        .build()
        .unwrap();
    let bytes = vec![0, 1, 0, 0, 2, 0, 0, 0, 1, 2];
    let decoded = DynamicMessage::from_cdr(&bytes, &schema).unwrap();
    assert_eq!(decoded.to_cdr().unwrap(), bytes);
}

#[test]
fn big_endian_octet_sequences_keep_schema_bounds() {
    use hiroz::dynamic::DynamicMessage;
    let schema = MessageSchema::builder("test_msgs/msg/BoundedOctets")
        .field(
            "value",
            FieldType::BoundedSequence(Box::new(FieldType::Byte), 2),
        )
        .build()
        .unwrap();
    let bytes = [0, 0, 0, 0, 0, 0, 0, 2, 7, 9];
    let decoded = DynamicMessage::from_cdr(&bytes, &schema).unwrap();
    assert_eq!(decoded.to_cdr().unwrap(), [0, 1, 0, 0, 2, 0, 0, 0, 7, 9]);
    let too_long = [0, 0, 0, 0, 0, 0, 0, 3, 7, 9, 11];
    assert!(DynamicMessage::from_cdr(&too_long, &schema).is_err());
    let unbacked = [0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff];
    assert!(DynamicMessage::from_cdr(&unbacked, &schema).is_err());
}
