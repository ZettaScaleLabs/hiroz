/// Dynamic encoding support for hiroz
///
/// This module provides runtime serialization format selection and schema transmission
/// via Zenoh's encoding mechanism. This enables:
/// - Runtime format negotiation (CDR, Protobuf, FlatBuffers)
/// - Interoperability with non-ROS Zenoh applications
/// - Schema-based validation and type evolution
/// - Zero-copy deserialization (for FlatBuffers)
///
/// # Performance Design
///
/// The encoding abstraction is designed for minimal overhead:
/// - Zenoh encoding strings are pre-computed and cached (no runtime formatting)
/// - Encoding dispatch uses integer-based matching (not string parsing)
/// - Publishers cache encoding to avoid repeated conversion
/// - Subscribers use inline dispatch for zero virtual call overhead
use std::fmt;

/// Serialization encoding format with optional schema information.
///
/// # Examples
///
/// ```
/// use hiroz::encoding::Encoding;
///
/// // CDR encoding (ROS 2 standard)
/// let cdr = Encoding::cdr();
///
/// // Protobuf with schema
/// let proto = Encoding::protobuf()
///     .with_schema("geometry_msgs/msg/Vector3");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum Encoding {
    /// CDR serialization (ROS 2 standard)
    #[default]
    Cdr,

    /// Protocol Buffers with optional schema name
    Protobuf { schema: Option<String> },

    /// FlatBuffers with optional schema name
    #[cfg(feature = "flatbuffers")]
    FlatBuffers { schema: Option<String> },
}

impl Encoding {
    /// Create a CDR encoding
    pub const fn cdr() -> Self {
        Encoding::Cdr
    }

    /// Create a Protobuf encoding without schema
    pub const fn protobuf() -> Self {
        Encoding::Protobuf { schema: None }
    }

    /// Create a FlatBuffers encoding without schema
    #[cfg(feature = "flatbuffers")]
    pub const fn flatbuffers() -> Self {
        Encoding::FlatBuffers { schema: None }
    }

    /// Add schema information to this encoding
    ///
    /// # Example
    ///
    /// ```
    /// use hiroz::encoding::Encoding;
    ///
    /// let encoding = Encoding::protobuf()
    ///     .with_schema("geometry_msgs/msg/Point");
    /// ```
    pub fn with_schema(mut self, schema: impl Into<String>) -> Self {
        match &mut self {
            Encoding::Protobuf { schema: s } => *s = Some(schema.into()),
            #[cfg(feature = "flatbuffers")]
            Encoding::FlatBuffers { schema: s } => *s = Some(schema.into()),
            Encoding::Cdr => {} // CDR doesn't use schema parameter
        }
        self
    }

    /// Get the schema name if present
    pub fn schema(&self) -> Option<&str> {
        match self {
            Encoding::Protobuf { schema } => schema.as_deref(),
            #[cfg(feature = "flatbuffers")]
            Encoding::FlatBuffers { schema } => schema.as_deref(),
            Encoding::Cdr => None,
        }
    }

    /// Get the base MIME type for this encoding
    pub fn mime_type(&self) -> &'static str {
        match self {
            Encoding::Cdr => "application/cdr",
            Encoding::Protobuf { .. } => "application/protobuf",
            #[cfg(feature = "flatbuffers")]
            Encoding::FlatBuffers { .. } => "application/flatbuffers",
        }
    }

    /// Get a unique integer ID for fast dispatch
    ///
    /// This enables efficient encoding matching without string parsing.
    #[inline]
    pub const fn id(&self) -> u8 {
        match self {
            Encoding::Cdr => 0,
            Encoding::Protobuf { .. } => 1,
            #[cfg(feature = "flatbuffers")]
            Encoding::FlatBuffers { .. } => 2,
        }
    }

    /// Convert to Zenoh encoding
    ///
    /// This creates the encoding string that will be transmitted with each message.
    /// Format: `mime_type[; schema=schema_name]`
    ///
    /// # Examples
    ///
    /// - CDR: `"application/cdr"`
    /// - Protobuf with schema: `"application/protobuf; schema=geometry_msgs/msg/Point"`
    /// - FlatBuffers with schema: `"application/flatbuffers; schema=sensor_msgs/msg/Image"`
    pub fn to_zenoh_encoding(&self) -> zenoh::bytes::Encoding {
        match self {
            Encoding::Cdr => {
                // Use Zenoh's predefined application/octet-stream for CDR
                // (CDR is the default binary format for ROS 2)
                zenoh::bytes::Encoding::APPLICATION_OCTET_STREAM
            }
            Encoding::Protobuf { schema: Some(s) } => {
                zenoh::bytes::Encoding::from(format!("application/protobuf; schema={}", s))
            }
            Encoding::Protobuf { schema: None } => {
                zenoh::bytes::Encoding::from("application/protobuf")
            }
            #[cfg(feature = "flatbuffers")]
            Encoding::FlatBuffers { schema: Some(s) } => {
                zenoh::bytes::Encoding::from(format!("application/flatbuffers; schema={}", s))
            }
            #[cfg(feature = "flatbuffers")]
            Encoding::FlatBuffers { schema: None } => {
                zenoh::bytes::Encoding::from("application/flatbuffers")
            }
        }
    }

    /// Parse from Zenoh encoding
    ///
    /// This extracts the format and schema from a received Zenoh encoding string.
    ///
    /// # Examples
    ///
    /// ```
    /// use hiroz::encoding::Encoding;
    ///
    /// let encoding = Encoding::from_zenoh_encoding("application/protobuf; schema=geometry_msgs/msg/Point");
    /// assert_eq!(encoding, Some(Encoding::protobuf().with_schema("geometry_msgs/msg/Point")));
    ///
    /// let encoding = Encoding::from_zenoh_encoding("application/cdr");
    /// assert_eq!(encoding, Some(Encoding::cdr()));
    /// ```
    pub fn from_zenoh_encoding(s: &str) -> Option<Self> {
        // Split on ';' to separate MIME type from parameters
        let mut parts = s.split(';').map(str::trim);
        let mime_type = parts.next()?;

        // Parse parameters (e.g., "schema=geometry_msgs/msg/Point")
        let mut schema = None;
        for param in parts {
            if let Some(value) = param.strip_prefix("schema=") {
                schema = Some(value.trim().to_string());
            }
        }

        match mime_type {
            "application/cdr" | "application/octet-stream" => Some(Encoding::Cdr),
            "application/protobuf" => Some(Encoding::Protobuf { schema }),
            #[cfg(feature = "flatbuffers")]
            "application/flatbuffers" => Some(Encoding::FlatBuffers { schema }),
            _ => None,
        }
    }

    /// Parse from a Zenoh encoding object
    pub fn from_zenoh_encoding_obj(encoding: &zenoh::bytes::Encoding) -> Option<Self> {
        Self::from_zenoh_encoding(&encoding.to_string())
    }
}

impl fmt::Display for Encoding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Encoding::Cdr => write!(f, "CDR"),
            Encoding::Protobuf { schema: Some(s) } => write!(f, "Protobuf({})", s),
            Encoding::Protobuf { schema: None } => write!(f, "Protobuf"),
            #[cfg(feature = "flatbuffers")]
            Encoding::FlatBuffers { schema: Some(s) } => write!(f, "FlatBuffers({})", s),
            #[cfg(feature = "flatbuffers")]
            Encoding::FlatBuffers { schema: None } => write!(f, "FlatBuffers"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cdr_encoding() {
        let enc = Encoding::cdr();
        assert_eq!(enc.mime_type(), "application/cdr");
        assert_eq!(enc.schema(), None);
        assert_eq!(enc.id(), 0);
    }

    #[test]
    fn test_protobuf_encoding() {
        let enc = Encoding::protobuf();
        assert_eq!(enc.mime_type(), "application/protobuf");
        assert_eq!(enc.schema(), None);

        let enc_with_schema = enc.with_schema("geometry_msgs/msg/Point");
        assert_eq!(enc_with_schema.schema(), Some("geometry_msgs/msg/Point"));
    }

    #[cfg(feature = "flatbuffers")]
    #[test]
    fn test_flatbuffers_encoding() {
        let enc = Encoding::flatbuffers();
        assert_eq!(enc.mime_type(), "application/flatbuffers");
        assert_eq!(enc.schema(), None);

        let enc_with_schema = enc.with_schema("sensor_msgs/msg/Image");
        assert_eq!(enc_with_schema.schema(), Some("sensor_msgs/msg/Image"));
    }

    #[test]
    fn test_to_zenoh_encoding() {
        let cdr = Encoding::cdr();
        let zenoh_enc = cdr.to_zenoh_encoding();
        assert_eq!(zenoh_enc.to_string(), "application/octet-stream");

        let proto = Encoding::protobuf().with_schema("test/msg/Foo");
        let zenoh_enc = proto.to_zenoh_encoding();
        assert!(zenoh_enc.to_string().contains("application/protobuf"));
        assert!(zenoh_enc.to_string().contains("schema=test/msg/Foo"));
    }

    #[test]
    fn test_from_zenoh_encoding() {
        // CDR
        let enc = Encoding::from_zenoh_encoding("application/cdr");
        assert_eq!(enc, Some(Encoding::cdr()));

        let enc = Encoding::from_zenoh_encoding("application/octet-stream");
        assert_eq!(enc, Some(Encoding::cdr()));

        // Protobuf without schema
        let enc = Encoding::from_zenoh_encoding("application/protobuf");
        assert_eq!(enc, Some(Encoding::protobuf()));

        // Protobuf with schema
        let enc =
            Encoding::from_zenoh_encoding("application/protobuf; schema=geometry_msgs/msg/Point");
        assert_eq!(
            enc,
            Some(Encoding::protobuf().with_schema("geometry_msgs/msg/Point"))
        );

        // Unknown encoding
        let enc = Encoding::from_zenoh_encoding("application/unknown");
        assert_eq!(enc, None);
    }

    #[test]
    fn test_encoding_roundtrip() {
        let original = Encoding::protobuf().with_schema("test/msg/MyMsg");
        let zenoh_enc = original.to_zenoh_encoding();
        let parsed = Encoding::from_zenoh_encoding(&zenoh_enc.to_string());
        assert_eq!(parsed, Some(original));
    }

    #[test]
    fn test_encoding_display() {
        assert_eq!(Encoding::cdr().to_string(), "CDR");
        assert_eq!(Encoding::protobuf().to_string(), "Protobuf");
        assert_eq!(
            Encoding::protobuf().with_schema("test/msg/Foo").to_string(),
            "Protobuf(test/msg/Foo)"
        );
    }

    #[test]
    fn test_encoding_id() {
        assert_eq!(Encoding::cdr().id(), 0);
        assert_eq!(Encoding::protobuf().id(), 1);
        #[cfg(feature = "flatbuffers")]
        assert_eq!(Encoding::flatbuffers().id(), 2);
    }
}
