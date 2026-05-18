package testdata

import (
	"encoding/binary"
	"testing"
)

// --- AddTwoInts Service Tests ---

func TestAddTwoIntsRequestRoundtrip(t *testing.T) {
	tests := []struct {
		name string
		a, b int64
	}{
		{"positive", 5, 3},
		{"zero", 0, 0},
		{"negative", -10, -20},
		{"mixed", -5, 15},
		{"large", 1000000000, 2000000000},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			msg := &AddTwoIntsRequest{A: tt.a, B: tt.b}

			serialized, err := msg.SerializeCDR()
			if err != nil {
				t.Fatalf("SerializeCDR failed: %v", err)
			}

			if len(serialized) != 16 {
				t.Fatalf("expected 16 bytes, got %d", len(serialized))
			}

			deserialized := &AddTwoIntsRequest{}
			if err := deserialized.DeserializeCDR(serialized); err != nil {
				t.Fatalf("DeserializeCDR failed: %v", err)
			}

			if deserialized.A != tt.a || deserialized.B != tt.b {
				t.Errorf("roundtrip mismatch: got (%d, %d), want (%d, %d)",
					deserialized.A, deserialized.B, tt.a, tt.b)
			}
		})
	}
}

func TestAddTwoIntsResponseRoundtrip(t *testing.T) {
	tests := []int64{0, 8, -30, 3000000000}

	for _, sum := range tests {
		msg := &AddTwoIntsResponse{Sum: sum}

		serialized, err := msg.SerializeCDR()
		if err != nil {
			t.Fatalf("SerializeCDR failed: %v", err)
		}

		if len(serialized) != 8 {
			t.Fatalf("expected 8 bytes, got %d", len(serialized))
		}

		deserialized := &AddTwoIntsResponse{}
		if err := deserialized.DeserializeCDR(serialized); err != nil {
			t.Fatalf("DeserializeCDR failed: %v", err)
		}

		if deserialized.Sum != sum {
			t.Errorf("roundtrip mismatch: got %d, want %d", deserialized.Sum, sum)
		}
	}
}

func TestAddTwoIntsRequestCDRFormat(t *testing.T) {
	msg := &AddTwoIntsRequest{A: 5, B: 3}
	data, err := msg.SerializeCDR()
	if err != nil {
		t.Fatalf("SerializeCDR failed: %v", err)
	}

	// Verify little-endian encoding
	gotA := int64(binary.LittleEndian.Uint64(data[0:8]))
	gotB := int64(binary.LittleEndian.Uint64(data[8:16]))

	if gotA != 5 {
		t.Errorf("A = %d, want 5", gotA)
	}
	if gotB != 3 {
		t.Errorf("B = %d, want 3", gotB)
	}
}

func TestAddTwoIntsTypeMetadata(t *testing.T) {
	req := &AddTwoIntsRequest{}
	if req.TypeName() != "example_interfaces/srv/AddTwoInts_Request" {
		t.Errorf("TypeName() = %q", req.TypeName())
	}

	resp := &AddTwoIntsResponse{}
	if resp.TypeName() != "example_interfaces/srv/AddTwoInts_Response" {
		t.Errorf("TypeName() = %q", resp.TypeName())
	}
}

func TestAddTwoIntsDeserializeErrors(t *testing.T) {
	req := &AddTwoIntsRequest{}
	if err := req.DeserializeCDR([]byte{1, 2, 3}); err == nil {
		t.Error("expected error for truncated request buffer")
	}

	resp := &AddTwoIntsResponse{}
	if err := resp.DeserializeCDR([]byte{1, 2}); err == nil {
		t.Error("expected error for truncated response buffer")
	}
}

// --- Fibonacci Action Tests ---

func TestFibonacciGoalRoundtrip(t *testing.T) {
	tests := []int32{0, 1, 5, 10, 100}

	for _, order := range tests {
		msg := &FibonacciGoal{Order: order}

		serialized, err := msg.SerializeCDR()
		if err != nil {
			t.Fatalf("SerializeCDR failed: %v", err)
		}

		if len(serialized) != 4 {
			t.Fatalf("expected 4 bytes, got %d", len(serialized))
		}

		deserialized := &FibonacciGoal{}
		if err := deserialized.DeserializeCDR(serialized); err != nil {
			t.Fatalf("DeserializeCDR failed: %v", err)
		}

		if deserialized.Order != order {
			t.Errorf("roundtrip mismatch: got %d, want %d", deserialized.Order, order)
		}
	}
}

func TestFibonacciResultRoundtrip(t *testing.T) {
	tests := []struct {
		name     string
		sequence []int32
	}{
		{"empty", []int32{}},
		{"single", []int32{0}},
		{"fib5", []int32{0, 1, 1, 2, 3}},
		{"fib10", []int32{0, 1, 1, 2, 3, 5, 8, 13, 21, 34}},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			msg := &FibonacciResult{Sequence: tt.sequence}

			serialized, err := msg.SerializeCDR()
			if err != nil {
				t.Fatalf("SerializeCDR failed: %v", err)
			}

			expectedLen := 4 + len(tt.sequence)*4
			if len(serialized) != expectedLen {
				t.Fatalf("expected %d bytes, got %d", expectedLen, len(serialized))
			}

			deserialized := &FibonacciResult{}
			if err := deserialized.DeserializeCDR(serialized); err != nil {
				t.Fatalf("DeserializeCDR failed: %v", err)
			}

			if len(deserialized.Sequence) != len(tt.sequence) {
				t.Fatalf("sequence length mismatch: got %d, want %d",
					len(deserialized.Sequence), len(tt.sequence))
			}

			for i, v := range tt.sequence {
				if deserialized.Sequence[i] != v {
					t.Errorf("Sequence[%d] = %d, want %d", i, deserialized.Sequence[i], v)
				}
			}
		})
	}
}

func TestFibonacciFeedbackRoundtrip(t *testing.T) {
	msg := &FibonacciFeedback{PartialSequence: []int32{0, 1, 1, 2, 3}}

	serialized, err := msg.SerializeCDR()
	if err != nil {
		t.Fatalf("SerializeCDR failed: %v", err)
	}

	deserialized := &FibonacciFeedback{}
	if err := deserialized.DeserializeCDR(serialized); err != nil {
		t.Fatalf("DeserializeCDR failed: %v", err)
	}

	if len(deserialized.PartialSequence) != 5 {
		t.Fatalf("PartialSequence length = %d, want 5", len(deserialized.PartialSequence))
	}

	expected := []int32{0, 1, 1, 2, 3}
	for i, v := range expected {
		if deserialized.PartialSequence[i] != v {
			t.Errorf("PartialSequence[%d] = %d, want %d", i, deserialized.PartialSequence[i], v)
		}
	}
}

func TestFibonacciResultCDRFormat(t *testing.T) {
	msg := &FibonacciResult{Sequence: []int32{0, 1, 1}}
	data, err := msg.SerializeCDR()
	if err != nil {
		t.Fatalf("SerializeCDR failed: %v", err)
	}

	// First 4 bytes: sequence length (3)
	seqLen := binary.LittleEndian.Uint32(data[0:4])
	if seqLen != 3 {
		t.Errorf("sequence length = %d, want 3", seqLen)
	}

	// Elements: 0, 1, 1
	for i, expected := range []int32{0, 1, 1} {
		got := int32(binary.LittleEndian.Uint32(data[4+i*4 : 4+(i+1)*4]))
		if got != expected {
			t.Errorf("element[%d] = %d, want %d", i, got, expected)
		}
	}
}

func TestFibonacciTypeMetadata(t *testing.T) {
	goal := &FibonacciGoal{}
	if goal.TypeName() != "example_interfaces/action/Fibonacci_Goal" {
		t.Errorf("Goal TypeName() = %q", goal.TypeName())
	}

	result := &FibonacciResult{}
	if result.TypeName() != "example_interfaces/action/Fibonacci_Result" {
		t.Errorf("Result TypeName() = %q", result.TypeName())
	}

	feedback := &FibonacciFeedback{}
	if feedback.TypeName() != "example_interfaces/action/Fibonacci_Feedback" {
		t.Errorf("Feedback TypeName() = %q", feedback.TypeName())
	}
}

func TestFibonacciDeserializeErrors(t *testing.T) {
	goal := &FibonacciGoal{}
	if err := goal.DeserializeCDR([]byte{1, 2}); err == nil {
		t.Error("expected error for truncated goal buffer")
	}

	result := &FibonacciResult{}
	if err := result.DeserializeCDR([]byte{1}); err == nil {
		t.Error("expected error for truncated result length buffer")
	}

	// Result with length=5 but insufficient data
	badResult := make([]byte, 8)
	binary.LittleEndian.PutUint32(badResult[0:4], 5)
	if err := result.DeserializeCDR(badResult); err == nil {
		t.Error("expected error for truncated result data buffer")
	}
}
