fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = prost_build::Config::new();
    // Enable serde support only for SensorData (for backward compatibility with pub/sub demo)
    config.type_attribute(
        "SensorData",
        "#[derive(serde::Serialize, serde::Deserialize)]",
    );
    // CalculateRequest/Response will use pure protobuf serialization (no serde)
    config.compile_protos(&["proto/sensor_data.proto"], &["proto/"])?;
    println!("cargo:rerun-if-changed=proto/sensor_data.proto");
    Ok(())
}
