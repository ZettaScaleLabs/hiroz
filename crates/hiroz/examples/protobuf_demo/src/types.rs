/// Shared protobuf type definitions and trait implementations
use hiroz::{
    MessageTypeInfo, ServiceTypeInfo, WithTypeInfo,
    entity::TypeHash,
    msg::{ProtobufSerdes, ZMessage, ZService},
};

// Include protobuf messages generated from sensor_data.proto
pub mod generated {
    include!(concat!(env!("OUT_DIR"), "/examples.rs"));
}

pub use generated::{CalculateRequest, CalculateResponse, SensorData};

// ========== SensorData Trait Implementations ==========

impl MessageTypeInfo for SensorData {
    fn type_name() -> &'static str {
        "examples::msg::dds_::SensorData_"
    }

    fn type_hash() -> TypeHash {
        TypeHash::zero() // For custom messages without ROS type support
    }
}

impl WithTypeInfo for SensorData {}

impl ZMessage for SensorData {
    type Serdes = hiroz::msg::SerdeCdrSerdes<SensorData>;
}

// SensorData uses serde/CDR for backward compatibility with the original pub/sub demo

// ========== CalculateRequest Trait Implementations ==========

impl MessageTypeInfo for CalculateRequest {
    fn type_name() -> &'static str {
        "examples::srv::dds_::Calculate_Request_"
    }

    fn type_hash() -> TypeHash {
        TypeHash::zero()
    }
}

impl WithTypeInfo for CalculateRequest {}

// Explicitly implement ZMessage to use ProtobufSerdes for pure protobuf serialization
impl ZMessage for CalculateRequest {
    type Serdes = ProtobufSerdes<CalculateRequest>;
}

// ========== CalculateResponse Trait Implementations ==========

impl MessageTypeInfo for CalculateResponse {
    fn type_name() -> &'static str {
        "examples::srv::dds_::Calculate_Response_"
    }

    fn type_hash() -> TypeHash {
        TypeHash::zero()
    }
}

impl WithTypeInfo for CalculateResponse {}

// Explicitly implement ZMessage to use ProtobufSerdes for pure protobuf serialization
impl ZMessage for CalculateResponse {
    type Serdes = ProtobufSerdes<CalculateResponse>;
}

// ========== Calculate Service Definition ==========

pub struct Calculate;

impl ServiceTypeInfo for Calculate {
    fn service_type_info() -> hiroz::entity::TypeInfo {
        hiroz::entity::TypeInfo::new("examples::srv::dds_::Calculate_", TypeHash::zero())
    }
}

impl ZService for Calculate {
    type Request = CalculateRequest;
    type Response = CalculateResponse;
}
