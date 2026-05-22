// Package testdata provides sample generated message types for testing.
// This simulates what hiroz-codegen-go would generate for a complex message.
package testdata

import (
	"encoding/binary"
	"fmt"
	"math"
)

// Point represents a 3D point
type Point struct {
	X float64
	Y float64
	Z float64
}

const (
	Point_TypeName = "geometry_msgs/msg/Point"
	Point_TypeHash = "RIHS01_point123456789012345678901234567890123456789012345678901234567"
)

func (m *Point) TypeName() string { return Point_TypeName }
func (m *Point) TypeHash() string { return Point_TypeHash }

func (m *Point) SerializeCDR() ([]byte, error) {
	return m.packToRaw(), nil
}

func (m *Point) packToRaw() []byte {
	buf := make([]byte, 0, 24)
	// Pack X
	b := make([]byte, 8)
	binary.LittleEndian.PutUint64(b, math.Float64bits(m.X))
	buf = append(buf, b...)
	// Pack Y
	binary.LittleEndian.PutUint64(b, math.Float64bits(m.Y))
	buf = append(buf, b...)
	// Pack Z
	binary.LittleEndian.PutUint64(b, math.Float64bits(m.Z))
	buf = append(buf, b...)
	return buf
}

func (m *Point) DeserializeCDR(data []byte) error {
	return m.unpackFromRaw(data)
}

func (m *Point) unpackFromRaw(data []byte) error {
	offset := 0
	if offset+8 > len(data) {
		return fmt.Errorf("buffer too short for X")
	}
	m.X = math.Float64frombits(binary.LittleEndian.Uint64(data[offset:]))
	offset += 8
	if offset+8 > len(data) {
		return fmt.Errorf("buffer too short for Y")
	}
	m.Y = math.Float64frombits(binary.LittleEndian.Uint64(data[offset:]))
	offset += 8
	if offset+8 > len(data) {
		return fmt.Errorf("buffer too short for Z")
	}
	m.Z = math.Float64frombits(binary.LittleEndian.Uint64(data[offset:]))
	return nil
}

// Quaternion represents orientation
type Quaternion struct {
	X float64
	Y float64
	Z float64
	W float64
}

const (
	Quaternion_TypeName = "geometry_msgs/msg/Quaternion"
	Quaternion_TypeHash = "RIHS01_quat1234567890123456789012345678901234567890123456789012345678"
)

func (m *Quaternion) TypeName() string { return Quaternion_TypeName }
func (m *Quaternion) TypeHash() string { return Quaternion_TypeHash }

func (m *Quaternion) SerializeCDR() ([]byte, error) {
	return m.packToRaw(), nil
}

func (m *Quaternion) packToRaw() []byte {
	buf := make([]byte, 0, 32)
	b := make([]byte, 8)
	binary.LittleEndian.PutUint64(b, math.Float64bits(m.X))
	buf = append(buf, b...)
	binary.LittleEndian.PutUint64(b, math.Float64bits(m.Y))
	buf = append(buf, b...)
	binary.LittleEndian.PutUint64(b, math.Float64bits(m.Z))
	buf = append(buf, b...)
	binary.LittleEndian.PutUint64(b, math.Float64bits(m.W))
	buf = append(buf, b...)
	return buf
}

func (m *Quaternion) DeserializeCDR(data []byte) error {
	return m.unpackFromRaw(data)
}

func (m *Quaternion) unpackFromRaw(data []byte) error {
	offset := 0
	if offset+8 > len(data) {
		return fmt.Errorf("buffer too short")
	}
	m.X = math.Float64frombits(binary.LittleEndian.Uint64(data[offset:]))
	offset += 8
	if offset+8 > len(data) {
		return fmt.Errorf("buffer too short")
	}
	m.Y = math.Float64frombits(binary.LittleEndian.Uint64(data[offset:]))
	offset += 8
	if offset+8 > len(data) {
		return fmt.Errorf("buffer too short")
	}
	m.Z = math.Float64frombits(binary.LittleEndian.Uint64(data[offset:]))
	offset += 8
	if offset+8 > len(data) {
		return fmt.Errorf("buffer too short")
	}
	m.W = math.Float64frombits(binary.LittleEndian.Uint64(data[offset:]))
	return nil
}

// Pose represents position and orientation
type Pose struct {
	Position    Point
	Orientation Quaternion
}

const (
	Pose_TypeName = "geometry_msgs/msg/Pose"
	Pose_TypeHash = "RIHS01_pose1234567890123456789012345678901234567890123456789012345678"
)

func (m *Pose) TypeName() string { return Pose_TypeName }
func (m *Pose) TypeHash() string { return Pose_TypeHash }

func (m *Pose) SerializeCDR() ([]byte, error) {
	return m.packToRaw(), nil
}

func (m *Pose) packToRaw() []byte {
	buf := make([]byte, 0, 56)
	buf = append(buf, m.Position.packToRaw()...)
	buf = append(buf, m.Orientation.packToRaw()...)
	return buf
}

func (m *Pose) DeserializeCDR(data []byte) error {
	return m.unpackFromRaw(data)
}

func (m *Pose) unpackFromRaw(data []byte) error {
	if err := m.Position.unpackFromRaw(data); err != nil {
		return err
	}
	if err := m.Orientation.unpackFromRaw(data[24:]); err != nil {
		return err
	}
	return nil
}
