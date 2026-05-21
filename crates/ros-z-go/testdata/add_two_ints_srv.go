// Package testdata provides sample generated message types for testing.
// This simulates what ros-z-codegen-go would generate for example_interfaces/srv/AddTwoInts.
package testdata

import (
	"encoding/binary"
	"fmt"
)

// AddTwoIntsRequest is the request message for AddTwoInts service
type AddTwoIntsRequest struct {
	A int64
	B int64
}

const (
	AddTwoIntsRequest_TypeName = "example_interfaces/srv/AddTwoInts_Request"
	AddTwoIntsRequest_TypeHash = "RIHS01_test_add_two_ints_request"
)

func (m *AddTwoIntsRequest) TypeName() string { return AddTwoIntsRequest_TypeName }
func (m *AddTwoIntsRequest) TypeHash() string { return AddTwoIntsRequest_TypeHash }

func (m *AddTwoIntsRequest) SerializeCDR() ([]byte, error) {
	buf := make([]byte, 16)
	binary.LittleEndian.PutUint64(buf[0:8], uint64(m.A))
	binary.LittleEndian.PutUint64(buf[8:16], uint64(m.B))
	return buf, nil
}

func (m *AddTwoIntsRequest) DeserializeCDR(data []byte) error {
	if len(data) < 16 {
		return fmt.Errorf("buffer too short for AddTwoIntsRequest: need 16, got %d", len(data))
	}
	m.A = int64(binary.LittleEndian.Uint64(data[0:8]))
	m.B = int64(binary.LittleEndian.Uint64(data[8:16]))
	return nil
}

// AddTwoIntsResponse is the response message for AddTwoInts service
type AddTwoIntsResponse struct {
	Sum int64
}

const (
	AddTwoIntsResponse_TypeName = "example_interfaces/srv/AddTwoInts_Response"
	AddTwoIntsResponse_TypeHash = "RIHS01_test_add_two_ints_response"
)

func (m *AddTwoIntsResponse) TypeName() string { return AddTwoIntsResponse_TypeName }
func (m *AddTwoIntsResponse) TypeHash() string { return AddTwoIntsResponse_TypeHash }

func (m *AddTwoIntsResponse) SerializeCDR() ([]byte, error) {
	buf := make([]byte, 8)
	binary.LittleEndian.PutUint64(buf[0:8], uint64(m.Sum))
	return buf, nil
}

func (m *AddTwoIntsResponse) DeserializeCDR(data []byte) error {
	if len(data) < 8 {
		return fmt.Errorf("buffer too short for AddTwoIntsResponse: need 8, got %d", len(data))
	}
	m.Sum = int64(binary.LittleEndian.Uint64(data[0:8]))
	return nil
}

// AddTwoInts is the service definition
type AddTwoInts struct{}

const (
	AddTwoInts_TypeName = "example_interfaces/srv/AddTwoInts"
	AddTwoInts_TypeHash = "RIHS01_test_add_two_ints"
)

func (s *AddTwoInts) TypeName() string { return AddTwoInts_TypeName }
func (s *AddTwoInts) TypeHash() string { return AddTwoInts_TypeHash }

func (s *AddTwoInts) SerializeCDR() ([]byte, error) {
	return nil, fmt.Errorf("service definition cannot be serialized")
}

func (s *AddTwoInts) DeserializeCDR(data []byte) error {
	return fmt.Errorf("service definition cannot be deserialized")
}

// Message interface (compatible with rosz.Message)
type Message interface {
	TypeName() string
	TypeHash() string
	SerializeCDR() ([]byte, error)
	DeserializeCDR([]byte) error
}

func (s *AddTwoInts) GetRequest() Message {
	return &AddTwoIntsRequest{}
}

func (s *AddTwoInts) GetResponse() Message {
	return &AddTwoIntsResponse{}
}
