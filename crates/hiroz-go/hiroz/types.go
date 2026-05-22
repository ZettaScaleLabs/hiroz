package hiroz

// Message is implemented by all generated message types
type Message interface {
	// TypeName returns the full ROS 2 type name (e.g., "std_msgs/msg/String")
	TypeName() string

	// TypeHash returns the ROS 2 type hash (RIHS01 format)
	TypeHash() string

	// SerializeCDR serializes the message to CDR format
	SerializeCDR() ([]byte, error)

	// DeserializeCDR deserializes CDR data into the message
	DeserializeCDR(data []byte) error
}

// Time represents a ROS 2 time
type Time struct {
	Sec  int32
	Nsec uint32
}

// Duration represents a ROS 2 duration
type Duration struct {
	Sec  int32
	Nsec uint32
}
