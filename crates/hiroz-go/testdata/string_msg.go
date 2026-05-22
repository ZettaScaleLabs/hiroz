// Package testdata provides sample generated message types for testing.
// This simulates what hiroz-codegen-go would generate for std_msgs/String.
package testdata

import (
	"encoding/binary"
	"fmt"
)

// String is a ROS 2 message type
// Full name: std_msgs/msg/String
type String struct {
	Data string
}

const (
	String_TypeName = "std_msgs/msg/String"
	String_TypeHash = "RIHS01_d9210a9c1c3f9cba05fe8e2b6e3b9a4a5e8c7d6f1a2b3c4d5e6f7a8b9c0d1e2f3"
)

// TypeName returns the full ROS 2 type name
func (m *String) TypeName() string {
	return String_TypeName
}

// TypeHash returns the ROS 2 type hash (RIHS01 format)
func (m *String) TypeHash() string {
	return String_TypeHash
}

// SerializeCDR serializes the message to CDR format
func (m *String) SerializeCDR() ([]byte, error) {
	// CDR serialization is performed directly in Go
	// The packToRaw method produces CDR-compatible little-endian bytes
	return m.packToRaw(), nil
}

func (m *String) packToRaw() []byte {
	buf := make([]byte, 0, 256)
	// Pack string: Data
	{
		data := []byte(m.Data)
		lenBytes := make([]byte, 4)
		binary.LittleEndian.PutUint32(lenBytes, uint32(len(data)+1))
		buf = append(buf, lenBytes...)
		buf = append(buf, data...)
		buf = append(buf, 0) // null terminator
	}
	return buf
}

// DeserializeCDR deserializes CDR data into the message
func (m *String) DeserializeCDR(data []byte) error {
	// CDR deserialization is performed directly in Go
	return m.unpackFromRaw(data)
}

func (m *String) unpackFromRaw(data []byte) error {
	offset := 0
	_ = offset // suppress unused warning
	// Unpack string: Data
	{
		if offset+4 > len(data) {
			return fmt.Errorf("buffer too short for string length")
		}
		strLen := int(binary.LittleEndian.Uint32(data[offset:]))
		offset += 4
		if strLen > 0 {
			if offset+strLen > len(data) {
				return fmt.Errorf("buffer too short for string data")
			}
			m.Data = string(data[offset : offset+strLen-1]) // exclude null terminator
			offset += strLen
		}
	}
	return nil
}
