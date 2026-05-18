package hiroz

import (
	"testing"
)

func TestTimeStructure(t *testing.T) {
	// Test that Time struct has expected fields
	tm := Time{
		Sec:  123,
		Nsec: 456789000,
	}

	if tm.Sec != 123 {
		t.Errorf("Sec = %d, want 123", tm.Sec)
	}
	if tm.Nsec != 456789000 {
		t.Errorf("Nsec = %d, want 456789000", tm.Nsec)
	}
}

func TestDurationStructure(t *testing.T) {
	// Test that Duration struct has expected fields
	d := Duration{
		Sec:  -5,
		Nsec: 500000000,
	}

	if d.Sec != -5 {
		t.Errorf("Sec = %d, want -5", d.Sec)
	}
	if d.Nsec != 500000000 {
		t.Errorf("Nsec = %d, want 500000000", d.Nsec)
	}
}

func TestTimeZeroValue(t *testing.T) {
	var tm Time
	if tm.Sec != 0 || tm.Nsec != 0 {
		t.Errorf("Zero Time should be (0, 0), got (%d, %d)", tm.Sec, tm.Nsec)
	}
}

func TestDurationZeroValue(t *testing.T) {
	var d Duration
	if d.Sec != 0 || d.Nsec != 0 {
		t.Errorf("Zero Duration should be (0, 0), got (%d, %d)", d.Sec, d.Nsec)
	}
}

// MockMessage implements the Message interface for testing
type MockMessage struct {
	typeName string
	typeHash string
	data     []byte
}

func (m *MockMessage) TypeName() string {
	return m.typeName
}

func (m *MockMessage) TypeHash() string {
	return m.typeHash
}

func (m *MockMessage) SerializeCDR() ([]byte, error) {
	return m.data, nil
}

func (m *MockMessage) DeserializeCDR(data []byte) error {
	m.data = make([]byte, len(data))
	copy(m.data, data)
	return nil
}

func TestMessageInterface(t *testing.T) {
	// Verify that MockMessage implements Message interface
	var _ Message = (*MockMessage)(nil)

	msg := &MockMessage{
		typeName: "test_msgs/msg/Test",
		typeHash: "RIHS01_test123",
		data:     []byte{1, 2, 3, 4},
	}

	if msg.TypeName() != "test_msgs/msg/Test" {
		t.Errorf("TypeName() = %q, want %q", msg.TypeName(), "test_msgs/msg/Test")
	}

	if msg.TypeHash() != "RIHS01_test123" {
		t.Errorf("TypeHash() = %q, want %q", msg.TypeHash(), "RIHS01_test123")
	}

	data, err := msg.SerializeCDR()
	if err != nil {
		t.Errorf("SerializeCDR() error = %v", err)
	}
	if len(data) != 4 {
		t.Errorf("SerializeCDR() returned %d bytes, want 4", len(data))
	}

	msg2 := &MockMessage{}
	if err := msg2.DeserializeCDR([]byte{5, 6, 7, 8}); err != nil {
		t.Errorf("DeserializeCDR() error = %v", err)
	}
	if len(msg2.data) != 4 || msg2.data[0] != 5 {
		t.Errorf("DeserializeCDR() data = %v, want [5 6 7 8]", msg2.data)
	}
}
