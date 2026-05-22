use hiroz::{MessageTypeInfo, entity::TypeHash};

#[test]
fn test_type_hash_zero() {
    // TypeHash::zero() should create a valid zero hash
    let zero_hash = TypeHash::zero();

    assert_eq!(zero_hash.version, 1);
    assert_eq!(zero_hash.value, [0u8; 32]);

    #[cfg(feature = "no-type-hash")]
    {
        // Humble: uses placeholder
        assert_eq!(zero_hash.to_rihs_string(), "TypeHashNotSupported");
    }

    #[cfg(not(feature = "no-type-hash"))]
    {
        // Jazzy/Rolling: uses actual hash
        assert_eq!(
            zero_hash.to_rihs_string(),
            "RIHS01_0000000000000000000000000000000000000000000000000000000000000000"
        );

        // It should be equivalent to parsing the zero hash string
        let parsed_zero = TypeHash::from_rihs_string(
            "RIHS01_0000000000000000000000000000000000000000000000000000000000000000",
        )
        .unwrap();
        assert_eq!(zero_hash, parsed_zero);
    }
}

// Mock message type for testing
#[derive(Debug)]
struct MockMessage {
    name: String,
    hash: TypeHash,
}

impl MessageTypeInfo for MockMessage {
    fn type_name() -> &'static str {
        "mock::StaticMessage"
    }

    fn type_hash() -> TypeHash {
        #[cfg(feature = "no-type-hash")]
        {
            TypeHash::zero()
        }
        #[cfg(not(feature = "no-type-hash"))]
        {
            TypeHash::from_rihs_string(
                "RIHS01_1111111111111111111111111111111111111111111111111111111111111111",
            )
            .unwrap()
        }
    }

    // Override dynamic methods to return instance-specific values
    fn type_name_dyn(&self) -> String {
        self.name.clone()
    }

    fn type_hash_dyn(&self) -> TypeHash {
        self.hash.clone()
    }
}

#[test]
fn test_static_type_info() {
    // Static methods work without an instance
    let static_name = MockMessage::type_name();
    let static_hash = MockMessage::type_hash();
    let static_info = MockMessage::type_info();

    assert_eq!(static_name, "mock::StaticMessage");

    #[cfg(feature = "no-type-hash")]
    {
        assert_eq!(static_hash.to_rihs_string(), "TypeHashNotSupported");
    }
    #[cfg(not(feature = "no-type-hash"))]
    {
        assert_eq!(
            static_hash.to_rihs_string(),
            "RIHS01_1111111111111111111111111111111111111111111111111111111111111111"
        );
    }

    assert_eq!(static_info.name, "mock::StaticMessage");
}

#[test]
#[cfg(not(feature = "no-type-hash"))]
fn test_dynamic_type_info() {
    // Dynamic methods work with instance-specific data
    // Skip this test for Humble since it uses hardcoded RIHS01 hashes
    let msg1 = MockMessage {
        name: "geometry_msgs::msg::dds_::Vector3_".to_string(),
        hash: TypeHash::from_rihs_string(
            "RIHS01_2222222222222222222222222222222222222222222222222222222222222222",
        )
        .unwrap(),
    };

    let msg2 = MockMessage {
        name: "std_msgs::msg::dds_::String_".to_string(),
        hash: TypeHash::from_rihs_string(
            "RIHS01_3333333333333333333333333333333333333333333333333333333333333333",
        )
        .unwrap(),
    };

    // Each instance returns its own type info
    assert_eq!(msg1.type_name_dyn(), "geometry_msgs::msg::dds_::Vector3_");
    assert_eq!(
        msg1.type_hash_dyn().to_rihs_string(),
        "RIHS01_2222222222222222222222222222222222222222222222222222222222222222"
    );

    assert_eq!(msg2.type_name_dyn(), "std_msgs::msg::dds_::String_");
    assert_eq!(
        msg2.type_hash_dyn().to_rihs_string(),
        "RIHS01_3333333333333333333333333333333333333333333333333333333333333333"
    );

    // type_info_dyn() combines both
    let info1 = msg1.type_info_dyn();
    assert_eq!(info1.name, "geometry_msgs::msg::dds_::Vector3_");
    assert_eq!(
        info1.hash.to_rihs_string(),
        "RIHS01_2222222222222222222222222222222222222222222222222222222222222222"
    );
}

#[test]
fn test_default_dynamic_delegates_to_static() {
    // A type that doesn't override dynamic methods
    struct SimpleMessage;

    impl MessageTypeInfo for SimpleMessage {
        fn type_name() -> &'static str {
            "simple::Message"
        }

        fn type_hash() -> TypeHash {
            #[cfg(feature = "no-type-hash")]
            {
                TypeHash::zero()
            }
            #[cfg(not(feature = "no-type-hash"))]
            {
                TypeHash::from_rihs_string(
                    "RIHS01_4444444444444444444444444444444444444444444444444444444444444444",
                )
                .unwrap()
            }
        }
    }

    let msg = SimpleMessage;

    // Dynamic methods delegate to static by default
    assert_eq!(msg.type_name_dyn(), "simple::Message");
    assert!(SimpleMessage::message_schema().is_none());

    #[cfg(feature = "no-type-hash")]
    {
        assert_eq!(msg.type_hash_dyn().to_rihs_string(), "TypeHashNotSupported");
    }
    #[cfg(not(feature = "no-type-hash"))]
    {
        assert_eq!(
            msg.type_hash_dyn().to_rihs_string(),
            "RIHS01_4444444444444444444444444444444444444444444444444444444444444444"
        );
    }
}
