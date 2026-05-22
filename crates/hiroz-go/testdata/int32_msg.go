// Package testdata provides sample generated message types for testing.
// This simulates what hiroz-codegen-go would generate for std_msgs/Int32.
package testdata

import (
	"encoding/binary"
	"fmt"
)

// Int32 is a ROS 2 message type
// Full name: std_msgs/msg/Int32
type Int32 struct {
	Data int32
}

const (
	Int32_TypeName = "std_msgs/msg/Int32"
	Int32_TypeHash = "RIHS01_a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2"
)

// TypeName returns the full ROS 2 type name
func (m *Int32) TypeName() string {
	return Int32_TypeName
}

// TypeHash returns the ROS 2 type hash (RIHS01 format)
func (m *Int32) TypeHash() string {
	return Int32_TypeHash
}

// SerializeCDR serializes the message to CDR format
func (m *Int32) SerializeCDR() ([]byte, error) {
	return m.packToRaw(), nil
}

func (m *Int32) packToRaw() []byte {
	buf := make([]byte, 0, 256)
	// Pack Int32: Data
	{
		b := make([]byte, 4)
		binary.LittleEndian.PutUint32(b, uint32(m.Data))
		buf = append(buf, b...)
	}
	return buf
}

// DeserializeCDR deserializes CDR data into the message
func (m *Int32) DeserializeCDR(data []byte) error {
	return m.unpackFromRaw(data)
}

func (m *Int32) unpackFromRaw(data []byte) error {
	offset := 0
	_ = offset
	// Unpack Int32: Data
	{
		if offset+4 > len(data) {
			return fmt.Errorf("buffer too short for int32")
		}
		m.Data = int32(binary.LittleEndian.Uint32(data[offset:]))
		offset += 4
	}
	return nil
}
