// Package messages provides minimal message types for the production example
package messages

import (
	"encoding/binary"
	"fmt"

	"github.com/ZettaScaleLabs/ros-z/crates/ros-z-go/rosz"
)

// AddTwoIntsRequest is the request message
type AddTwoIntsRequest struct {
	A int64
	B int64
}

func (m *AddTwoIntsRequest) TypeName() string { return "example_interfaces/srv/AddTwoInts_Request" }
func (m *AddTwoIntsRequest) TypeHash() string { return "RIHS01_test_add_two_ints_request" }

func (m *AddTwoIntsRequest) SerializeCDR() ([]byte, error) {
	buf := make([]byte, 16)
	binary.LittleEndian.PutUint64(buf[0:8], uint64(m.A))
	binary.LittleEndian.PutUint64(buf[8:16], uint64(m.B))
	return buf, nil
}

func (m *AddTwoIntsRequest) DeserializeCDR(data []byte) error {
	if len(data) < 16 {
		return fmt.Errorf("buffer too short: need 16, got %d", len(data))
	}
	m.A = int64(binary.LittleEndian.Uint64(data[0:8]))
	m.B = int64(binary.LittleEndian.Uint64(data[8:16]))
	return nil
}

// AddTwoIntsResponse is the response message
type AddTwoIntsResponse struct {
	Sum int64
}

func (m *AddTwoIntsResponse) TypeName() string { return "example_interfaces/srv/AddTwoInts_Response" }
func (m *AddTwoIntsResponse) TypeHash() string { return "RIHS01_test_add_two_ints_response" }

func (m *AddTwoIntsResponse) SerializeCDR() ([]byte, error) {
	buf := make([]byte, 8)
	binary.LittleEndian.PutUint64(buf[0:8], uint64(m.Sum))
	return buf, nil
}

func (m *AddTwoIntsResponse) DeserializeCDR(data []byte) error {
	if len(data) < 8 {
		return fmt.Errorf("buffer too short: need 8, got %d", len(data))
	}
	m.Sum = int64(binary.LittleEndian.Uint64(data[0:8]))
	return nil
}

// AddTwoInts is the service definition
type AddTwoInts struct{}

func (s *AddTwoInts) TypeName() string { return "example_interfaces/srv/AddTwoInts" }
func (s *AddTwoInts) TypeHash() string { return "RIHS01_test_add_two_ints" }

func (s *AddTwoInts) SerializeCDR() ([]byte, error) {
	return nil, fmt.Errorf("service definition cannot be serialized")
}

func (s *AddTwoInts) DeserializeCDR(data []byte) error {
	return fmt.Errorf("service definition cannot be deserialized")
}

func (s *AddTwoInts) GetRequest() rosz.Message {
	return &AddTwoIntsRequest{}
}

func (s *AddTwoInts) GetResponse() rosz.Message {
	return &AddTwoIntsResponse{}
}
