// Package testdata provides sample generated message types for testing.
// This simulates what ros-z-codegen-go would generate for example_interfaces/action/Fibonacci.
package testdata

import (
	"encoding/binary"
	"fmt"
)

// FibonacciGoal is the goal message for Fibonacci action
type FibonacciGoal struct {
	Order int32
}

const (
	FibonacciGoal_TypeName = "example_interfaces/action/Fibonacci_Goal"
	FibonacciGoal_TypeHash = "RIHS01_test_fibonacci_goal"
)

func (m *FibonacciGoal) TypeName() string { return FibonacciGoal_TypeName }
func (m *FibonacciGoal) TypeHash() string { return FibonacciGoal_TypeHash }

func (m *FibonacciGoal) SerializeCDR() ([]byte, error) {
	buf := make([]byte, 4)
	binary.LittleEndian.PutUint32(buf[0:4], uint32(m.Order))
	return buf, nil
}

func (m *FibonacciGoal) DeserializeCDR(data []byte) error {
	if len(data) < 4 {
		return fmt.Errorf("buffer too short for FibonacciGoal: need 4, got %d", len(data))
	}
	m.Order = int32(binary.LittleEndian.Uint32(data[0:4]))
	return nil
}

// FibonacciResult is the result message for Fibonacci action
type FibonacciResult struct {
	Sequence []int32
}

const (
	FibonacciResult_TypeName = "example_interfaces/action/Fibonacci_Result"
	FibonacciResult_TypeHash = "RIHS01_test_fibonacci_result"
)

func (m *FibonacciResult) TypeName() string { return FibonacciResult_TypeName }
func (m *FibonacciResult) TypeHash() string { return FibonacciResult_TypeHash }

func (m *FibonacciResult) SerializeCDR() ([]byte, error) {
	buf := make([]byte, 4+len(m.Sequence)*4)
	binary.LittleEndian.PutUint32(buf[0:4], uint32(len(m.Sequence)))
	for i, v := range m.Sequence {
		binary.LittleEndian.PutUint32(buf[4+i*4:4+(i+1)*4], uint32(v))
	}
	return buf, nil
}

func (m *FibonacciResult) DeserializeCDR(data []byte) error {
	if len(data) < 4 {
		return fmt.Errorf("buffer too short for FibonacciResult length: need 4, got %d", len(data))
	}
	n := int(binary.LittleEndian.Uint32(data[0:4]))
	if len(data) < 4+n*4 {
		return fmt.Errorf("buffer too short for FibonacciResult data: need %d, got %d", 4+n*4, len(data))
	}
	m.Sequence = make([]int32, n)
	for i := 0; i < n; i++ {
		m.Sequence[i] = int32(binary.LittleEndian.Uint32(data[4+i*4 : 4+(i+1)*4]))
	}
	return nil
}

// FibonacciFeedback is the feedback message for Fibonacci action
type FibonacciFeedback struct {
	PartialSequence []int32
}

const (
	FibonacciFeedback_TypeName = "example_interfaces/action/Fibonacci_Feedback"
	FibonacciFeedback_TypeHash = "RIHS01_test_fibonacci_feedback"
)

func (m *FibonacciFeedback) TypeName() string { return FibonacciFeedback_TypeName }
func (m *FibonacciFeedback) TypeHash() string { return FibonacciFeedback_TypeHash }

func (m *FibonacciFeedback) SerializeCDR() ([]byte, error) {
	buf := make([]byte, 4+len(m.PartialSequence)*4)
	binary.LittleEndian.PutUint32(buf[0:4], uint32(len(m.PartialSequence)))
	for i, v := range m.PartialSequence {
		binary.LittleEndian.PutUint32(buf[4+i*4:4+(i+1)*4], uint32(v))
	}
	return buf, nil
}

func (m *FibonacciFeedback) DeserializeCDR(data []byte) error {
	if len(data) < 4 {
		return fmt.Errorf("buffer too short for FibonacciFeedback length: need 4, got %d", len(data))
	}
	n := int(binary.LittleEndian.Uint32(data[0:4]))
	if len(data) < 4+n*4 {
		return fmt.Errorf("buffer too short for FibonacciFeedback data: need %d, got %d", 4+n*4, len(data))
	}
	m.PartialSequence = make([]int32, n)
	for i := 0; i < n; i++ {
		m.PartialSequence[i] = int32(binary.LittleEndian.Uint32(data[4+i*4 : 4+(i+1)*4]))
	}
	return nil
}
