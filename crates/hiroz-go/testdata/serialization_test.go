package testdata

import (
	"bytes"
	"encoding/binary"
	"math"
	"testing"
)

func TestStringMessageRoundtrip(t *testing.T) {
	tests := []struct {
		name string
		data string
	}{
		{"empty", ""},
		{"simple", "Hello, World!"},
		{"unicode", "Hello, 世界! 🌍"},
		{"long", "This is a longer string that tests buffer handling capabilities"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			msg := &String{Data: tt.data}

			// Serialize
			serialized, err := msg.SerializeCDR()
			if err != nil {
				t.Fatalf("SerializeCDR failed: %v", err)
			}

			// Deserialize
			msg2 := &String{}
			if err := msg2.DeserializeCDR(serialized); err != nil {
				t.Fatalf("DeserializeCDR failed: %v", err)
			}

			// Compare
			if msg.Data != msg2.Data {
				t.Errorf("Roundtrip failed: got %q, want %q", msg2.Data, msg.Data)
			}
		})
	}
}

func TestStringMessageCDRFormat(t *testing.T) {
	msg := &String{Data: "test"}
	serialized, err := msg.SerializeCDR()
	if err != nil {
		t.Fatalf("SerializeCDR failed: %v", err)
	}

	// Verify CDR format:
	// - 4 bytes: length (including null terminator) = 5
	// - 4 bytes: "test"
	// - 1 byte: null terminator
	if len(serialized) != 9 {
		t.Errorf("Expected 9 bytes, got %d", len(serialized))
	}

	length := binary.LittleEndian.Uint32(serialized[:4])
	if length != 5 {
		t.Errorf("Expected length 5, got %d", length)
	}

	if string(serialized[4:8]) != "test" {
		t.Errorf("Expected 'test', got %q", string(serialized[4:8]))
	}

	if serialized[8] != 0 {
		t.Errorf("Expected null terminator, got %d", serialized[8])
	}
}

func TestInt32MessageRoundtrip(t *testing.T) {
	tests := []struct {
		name string
		data int32
	}{
		{"zero", 0},
		{"positive", 42},
		{"negative", -42},
		{"max", math.MaxInt32},
		{"min", math.MinInt32},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			msg := &Int32{Data: tt.data}

			serialized, err := msg.SerializeCDR()
			if err != nil {
				t.Fatalf("SerializeCDR failed: %v", err)
			}

			msg2 := &Int32{}
			if err := msg2.DeserializeCDR(serialized); err != nil {
				t.Fatalf("DeserializeCDR failed: %v", err)
			}

			if msg.Data != msg2.Data {
				t.Errorf("Roundtrip failed: got %d, want %d", msg2.Data, msg.Data)
			}
		})
	}
}

func TestInt32MessageCDRFormat(t *testing.T) {
	msg := &Int32{Data: 0x12345678}
	serialized, err := msg.SerializeCDR()
	if err != nil {
		t.Fatalf("SerializeCDR failed: %v", err)
	}

	// Should be exactly 4 bytes in little-endian
	expected := []byte{0x78, 0x56, 0x34, 0x12}
	if !bytes.Equal(serialized, expected) {
		t.Errorf("CDR format incorrect: got %v, want %v", serialized, expected)
	}
}

func TestPointMessageRoundtrip(t *testing.T) {
	tests := []struct {
		name    string
		x, y, z float64
	}{
		{"origin", 0, 0, 0},
		{"unit", 1, 1, 1},
		{"negative", -1.5, -2.5, -3.5},
		{"mixed", 1.23456789, -9.87654321, 0},
		{"large", 1e10, 1e-10, math.MaxFloat64},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			msg := &Point{X: tt.x, Y: tt.y, Z: tt.z}

			serialized, err := msg.SerializeCDR()
			if err != nil {
				t.Fatalf("SerializeCDR failed: %v", err)
			}

			msg2 := &Point{}
			if err := msg2.DeserializeCDR(serialized); err != nil {
				t.Fatalf("DeserializeCDR failed: %v", err)
			}

			if msg.X != msg2.X || msg.Y != msg2.Y || msg.Z != msg2.Z {
				t.Errorf("Roundtrip failed: got (%v,%v,%v), want (%v,%v,%v)",
					msg2.X, msg2.Y, msg2.Z, msg.X, msg.Y, msg.Z)
			}
		})
	}
}

func TestPointMessageCDRFormat(t *testing.T) {
	msg := &Point{X: 1.0, Y: 2.0, Z: 3.0}
	serialized, err := msg.SerializeCDR()
	if err != nil {
		t.Fatalf("SerializeCDR failed: %v", err)
	}

	// Should be 24 bytes (3 x float64)
	if len(serialized) != 24 {
		t.Errorf("Expected 24 bytes, got %d", len(serialized))
	}

	// Verify little-endian float64 values
	x := math.Float64frombits(binary.LittleEndian.Uint64(serialized[0:8]))
	y := math.Float64frombits(binary.LittleEndian.Uint64(serialized[8:16]))
	z := math.Float64frombits(binary.LittleEndian.Uint64(serialized[16:24]))

	if x != 1.0 || y != 2.0 || z != 3.0 {
		t.Errorf("CDR values incorrect: got (%v,%v,%v), want (1,2,3)", x, y, z)
	}
}

func TestPoseMessageRoundtrip(t *testing.T) {
	msg := &Pose{
		Position:    Point{X: 1.0, Y: 2.0, Z: 3.0},
		Orientation: Quaternion{X: 0.0, Y: 0.0, Z: 0.0, W: 1.0},
	}

	serialized, err := msg.SerializeCDR()
	if err != nil {
		t.Fatalf("SerializeCDR failed: %v", err)
	}

	msg2 := &Pose{}
	if err := msg2.DeserializeCDR(serialized); err != nil {
		t.Fatalf("DeserializeCDR failed: %v", err)
	}

	// Compare positions
	if msg.Position.X != msg2.Position.X ||
		msg.Position.Y != msg2.Position.Y ||
		msg.Position.Z != msg2.Position.Z {
		t.Errorf("Position mismatch")
	}

	// Compare orientations
	if msg.Orientation.X != msg2.Orientation.X ||
		msg.Orientation.Y != msg2.Orientation.Y ||
		msg.Orientation.Z != msg2.Orientation.Z ||
		msg.Orientation.W != msg2.Orientation.W {
		t.Errorf("Orientation mismatch")
	}
}

func TestPoseMessageCDRFormat(t *testing.T) {
	msg := &Pose{
		Position:    Point{X: 1.0, Y: 2.0, Z: 3.0},
		Orientation: Quaternion{X: 0.1, Y: 0.2, Z: 0.3, W: 0.9},
	}

	serialized, err := msg.SerializeCDR()
	if err != nil {
		t.Fatalf("SerializeCDR failed: %v", err)
	}

	// Should be 56 bytes (7 x float64)
	if len(serialized) != 56 {
		t.Errorf("Expected 56 bytes, got %d", len(serialized))
	}
}

func TestTypeMetadata(t *testing.T) {
	tests := []struct {
		msg interface {
			TypeName() string
			TypeHash() string
		}
		typeName   string
		hashPrefix string
	}{
		{&String{}, "std_msgs/msg/String", "RIHS01_"},
		{&Int32{}, "std_msgs/msg/Int32", "RIHS01_"},
		{&Point{}, "geometry_msgs/msg/Point", "RIHS01_"},
		{&Quaternion{}, "geometry_msgs/msg/Quaternion", "RIHS01_"},
		{&Pose{}, "geometry_msgs/msg/Pose", "RIHS01_"},
	}

	for _, tt := range tests {
		t.Run(tt.typeName, func(t *testing.T) {
			if tt.msg.TypeName() != tt.typeName {
				t.Errorf("TypeName() = %q, want %q", tt.msg.TypeName(), tt.typeName)
			}
			if len(tt.msg.TypeHash()) < 7 || tt.msg.TypeHash()[:7] != tt.hashPrefix {
				t.Errorf("TypeHash() should start with %q, got %q", tt.hashPrefix, tt.msg.TypeHash())
			}
		})
	}
}

func TestDeserializeErrors(t *testing.T) {
	t.Run("String_EmptyBuffer", func(t *testing.T) {
		msg := &String{}
		err := msg.DeserializeCDR([]byte{})
		if err == nil {
			t.Error("Expected error for empty buffer")
		}
	})

	t.Run("String_TruncatedLength", func(t *testing.T) {
		msg := &String{}
		err := msg.DeserializeCDR([]byte{0x05}) // Only 1 byte, need 4
		if err == nil {
			t.Error("Expected error for truncated length")
		}
	})

	t.Run("String_TruncatedData", func(t *testing.T) {
		msg := &String{}
		// Length says 10, but only 2 data bytes
		err := msg.DeserializeCDR([]byte{0x0a, 0x00, 0x00, 0x00, 'a', 'b'})
		if err == nil {
			t.Error("Expected error for truncated data")
		}
	})

	t.Run("Int32_TruncatedBuffer", func(t *testing.T) {
		msg := &Int32{}
		err := msg.DeserializeCDR([]byte{0x01, 0x02}) // Only 2 bytes, need 4
		if err == nil {
			t.Error("Expected error for truncated buffer")
		}
	})

	t.Run("Point_TruncatedBuffer", func(t *testing.T) {
		msg := &Point{}
		err := msg.DeserializeCDR(make([]byte, 16)) // Only 16 bytes, need 24
		if err == nil {
			t.Error("Expected error for truncated buffer")
		}
	})
}

// Benchmarks

func BenchmarkStringSerialize(b *testing.B) {
	msg := &String{Data: "Hello, World! This is a benchmark test string."}
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		_, _ = msg.SerializeCDR()
	}
}

func BenchmarkStringDeserialize(b *testing.B) {
	msg := &String{Data: "Hello, World! This is a benchmark test string."}
	data, _ := msg.SerializeCDR()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		msg2 := &String{}
		_ = msg2.DeserializeCDR(data)
	}
}

func BenchmarkPoseSerialize(b *testing.B) {
	msg := &Pose{
		Position:    Point{X: 1.0, Y: 2.0, Z: 3.0},
		Orientation: Quaternion{X: 0.0, Y: 0.0, Z: 0.0, W: 1.0},
	}
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		_, _ = msg.SerializeCDR()
	}
}

func BenchmarkPoseDeserialize(b *testing.B) {
	msg := &Pose{
		Position:    Point{X: 1.0, Y: 2.0, Z: 3.0},
		Orientation: Quaternion{X: 0.0, Y: 0.0, Z: 0.0, W: 1.0},
	}
	data, _ := msg.SerializeCDR()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		msg2 := &Pose{}
		_ = msg2.DeserializeCDR(data)
	}
}
