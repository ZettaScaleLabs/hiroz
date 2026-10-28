package hiroz

/*
#include <stdlib.h>
#include "hiroz_ffi.h"
*/
import "C"
import (
	"bytes"
	"encoding/json"
	"fmt"
	"strings"
	"time"
	"unsafe"
)

// TypeDescription follows type_description_interfaces/msg/TypeDescription.
// Field type IDs use the ROS RIHS01 specification, including +48 for fixed
// arrays, +96 for bounded sequences and +144 for unbounded sequences.
type TypeDescription struct {
	Type       IndividualTypeDescription   `json:"type_description"`
	Referenced []IndividualTypeDescription `json:"referenced_type_descriptions"`
}
type IndividualTypeDescription struct {
	Name   string             `json:"type_name"`
	Fields []FieldDescription `json:"fields"`
}
type FieldDescription struct {
	Name    string               `json:"name"`
	Type    FieldTypeDescription `json:"type"`
	Default string               `json:"default_value"`
}
type FieldTypeDescription struct {
	ID             uint8  `json:"type_id"`
	Capacity       uint64 `json:"capacity"`
	StringCapacity uint64 `json:"string_capacity"`
	NestedType     string `json:"nested_type_name"`
}

const (
	FieldNested        uint8 = 1
	FieldInt8          uint8 = 2
	FieldUint8         uint8 = 3
	FieldInt16         uint8 = 4
	FieldUint16        uint8 = 5
	FieldInt32         uint8 = 6
	FieldUint32        uint8 = 7
	FieldInt64         uint8 = 8
	FieldUint64        uint8 = 9
	FieldFloat32       uint8 = 10
	FieldFloat64       uint8 = 11
	FieldBool          uint8 = 15
	FieldString        uint8 = 17
	FieldBoundedString uint8 = 21
)

// MessageSchema is immutable after construction and can be shared by messages.
// Description returns a copy suitable for external storage or ROS interoperability.
type MessageSchema struct {
	description       json.RawMessage
	name, hash        string
	transportTypeName string
}

func (s *MessageSchema) TypeName() string { return s.name }
func (s *MessageSchema) TypeHash() string { return s.hash }
func (s *MessageSchema) Description() TypeDescription {
	var d TypeDescription
	_ = json.Unmarshal(s.description, &d)
	return d
}

type schemaResult struct {
	Description       json.RawMessage
	TypeHash          string
	TransportTypeName string
	Error             string
}

func schemaFromResult(r schemaResult) (*MessageSchema, error) {
	if r.Error != "" {
		return nil, fmt.Errorf("schema: %s", r.Error)
	}
	var d TypeDescription
	if err := json.Unmarshal(r.Description, &d); err != nil {
		return nil, err
	}
	if d.Type.Name == "" || r.TypeHash == "" {
		return nil, fmt.Errorf("incomplete schema response")
	}
	return &MessageSchema{
		description: append(json.RawMessage(nil), r.Description...),
		name:        d.Type.Name, hash: r.TypeHash,
		transportTypeName: r.TransportTypeName,
	}, nil
}

// NewMessageSchema validates a ROS description using the native schema model and
// computes its RIHS01 hash. Native unsupported field types return an error.
func NewMessageSchema(d TypeDescription) (*MessageSchema, error) {
	if d.Referenced == nil {
		d.Referenced = []IndividualTypeDescription{}
	}
	encoded, err := json.Marshal(d)
	if err != nil {
		return nil, err
	}
	text := C.CString(string(encoded))
	defer C.free(unsafe.Pointer(text))
	var r schemaResult
	if err = decodeNativeJSON(C.hiroz_schema_validate(text), &r); err != nil {
		return nil, err
	}
	return schemaFromResult(r)
}

// SchemaMessage is an optional Message extension. Builders register its schema
// when the node has a type description service enabled.
type SchemaMessage interface {
	Message
	MessageSchema() *MessageSchema
}

// RegisterSchema makes a schema discoverable through this node. Build the node
// with WithTypeDescriptionService first. Generated types can opt in by
// implementing SchemaMessage; registration also supports service request/reply types.
func (n *Node) RegisterSchema(s *MessageSchema) error {
	handle, release, err := n.acquire()
	if err != nil {
		return err
	}
	defer release()
	if s == nil {
		return fmt.Errorf("schema is nil")
	}
	text := C.CString(string(s.description))
	defer C.free(unsafe.Pointer(text))
	if rc := C.hiroz_node_register_schema(handle, text); rc != 0 {
		return newHirozError(ErrorCode(rc), "schema registration failed; enable the node type description service")
	}
	return nil
}

func (n *Node) discoverSchema(name, req, resp string, timeout time.Duration, dest any) error {
	handle, release, err := n.acquire()
	if err != nil {
		return err
	}
	defer release()
	if timeout <= 0 {
		return fmt.Errorf("timeout must be positive")
	}
	a, b, c := C.CString(name), C.CString(req), C.CString(resp)
	defer C.free(unsafe.Pointer(a))
	defer C.free(unsafe.Pointer(b))
	defer C.free(unsafe.Pointer(c))
	millis := timeout / time.Millisecond
	if timeout%time.Millisecond != 0 {
		millis++
	}
	return decodeNativeJSON(C.hiroz_node_discover_schema(handle, a, b, c, C.uint64_t(millis)), dest)
}

// DiscoverTopicSchema resolves a live publisher's schema. The timeout covers
// graph discovery, schema-service route readiness, and the schema request.
func (n *Node) DiscoverTopicSchema(topic string, timeout time.Duration) (*MessageSchema, error) {
	var r schemaResult
	if err := n.discoverSchema(topic, "", "", timeout, &r); err != nil {
		return nil, err
	}
	return schemaFromResult(r)
}

// DiscoverServiceSchemas uses schema type names such as pkg/msg/ServiceRequest.
// timeout bounds the complete discovery operation, including route readiness
// and both schema queries.
func (n *Node) DiscoverServiceSchemas(service, requestType, responseType string, timeout time.Duration) (*MessageSchema, *MessageSchema, error) {
	if requestType == "" || responseType == "" {
		return nil, nil, fmt.Errorf("both service schema type names are required")
	}
	var r struct {
		Request, Response schemaResult
		Error             string
	}
	if err := n.discoverSchema(service, requestType, responseType, timeout, &r); err != nil {
		return nil, nil, err
	}
	if r.Error != "" {
		return nil, nil, fmt.Errorf("schema: %s", r.Error)
	}
	a, err := schemaFromResult(r.Request)
	if err != nil {
		return nil, nil, err
	}
	b, err := schemaFromResult(r.Response)
	return a, b, err
}

// DynamicMessage stores runtime-typed fields. Values accepts JSON-compatible
// values (use []any or integer slices for sequences, not []byte which JSON base64
// encodes). Integer decoding uses json.Number to preserve all 64 bits. The JSON
// bridge requires finite floats. Omitted fields use native schema defaults.
type DynamicMessage struct {
	Schema *MessageSchema
	Values map[string]any
}

func (s *MessageSchema) NewMessage() *DynamicMessage {
	return &DynamicMessage{Schema: s, Values: map[string]any{}}
}

// TypeName preserves the advertised wire identity for a discovered schema.
// Schemas constructed locally use the canonical DDS spelling.
func (m *DynamicMessage) TypeName() string {
	if m.Schema.transportTypeName != "" {
		return m.Schema.transportTypeName
	}
	parts := strings.Split(m.Schema.TypeName(), "/")
	if len(parts) == 3 {
		return parts[0] + "::" + parts[1] + "::dds_::" + parts[2] + "_"
	}
	return m.Schema.TypeName()
}
func (m *DynamicMessage) TypeHash() string              { return m.Schema.TypeHash() }
func (m *DynamicMessage) MessageSchema() *MessageSchema { return m.Schema }
func (m *DynamicMessage) SerializeCDR() ([]byte, error) {
	if m == nil || m.Schema == nil {
		return nil, fmt.Errorf("message schema is nil")
	}
	values := m.Values
	if values == nil {
		values = map[string]any{}
	}
	encoded, err := json.Marshal(values)
	if err != nil {
		return nil, err
	}
	a, b := C.CString(string(m.Schema.description)), C.CString(string(encoded))
	defer C.free(unsafe.Pointer(a))
	defer C.free(unsafe.Pointer(b))
	var data *C.uint8_t
	var size C.uintptr_t
	rc := C.hiroz_dynamic_encode(a, b, &data, &size)
	if rc != 0 {
		return nil, newHirozError(ErrorCode(rc), "dynamic CDR encoding failed")
	}
	defer C.hiroz_free_bytes(data, size)
	return append([]byte(nil), unsafe.Slice((*byte)(unsafe.Pointer(data)), int(size))...), nil
}
func (m *DynamicMessage) DeserializeCDR(data []byte) error {
	if m == nil || m.Schema == nil {
		return fmt.Errorf("message schema is nil")
	}
	if len(data) == 0 {
		return fmt.Errorf("empty CDR message")
	}
	a := C.CString(string(m.Schema.description))
	defer C.free(unsafe.Pointer(a))
	ptr := C.hiroz_dynamic_decode(a, (*C.uint8_t)(unsafe.Pointer(&data[0])), C.uintptr_t(len(data)))
	if ptr == nil {
		return fmt.Errorf("dynamic decoding failed")
	}
	defer C.hiroz_json_destroy(ptr)
	var result struct {
		Values map[string]any
		Error  string
	}
	decoder := json.NewDecoder(bytes.NewBufferString(C.GoString(ptr)))
	decoder.UseNumber()
	if err := decoder.Decode(&result); err != nil {
		return err
	}
	if result.Error != "" {
		return fmt.Errorf("dynamic decoding: %s", result.Error)
	}
	m.Values = result.Values
	return nil
}

// BuildDynamicWithCallback decodes every received CDR payload using schema. Malformed
// payloads are delivered as errors, allowing applications to observe decode failures.
func (b *SubscriberBuilder) BuildDynamicWithCallback(schema *MessageSchema, handler func(*DynamicMessage, error)) (*Subscriber, error) {
	if schema == nil || handler == nil {
		return nil, fmt.Errorf("schema and handler required")
	}
	return b.BuildWithCallback(schema.NewMessage(), func(data []byte) {
		message := schema.NewMessage()
		err := message.DeserializeCDR(data)
		handler(message, err)
	})
}

// TypeDescriptionMessage is implemented by generated messages with full schema
// metadata. Regenerate old messages to enable automatic registration.
type TypeDescriptionMessage interface {
	Message
	TypeDescriptionJSON() string
}

// NewMessageSchemaJSON accepts the ROS TypeDescription JSON representation.
func NewMessageSchemaJSON(description string) (*MessageSchema, error) {
	var d TypeDescription
	if err := json.Unmarshal([]byte(description), &d); err != nil {
		return nil, err
	}
	return NewMessageSchema(d)
}
func (n *Node) registerMessageSchema(message Message) error {
	handle, release, err := n.acquire()
	if err != nil {
		return err
	}
	defer release()
	if !bool(C.hiroz_node_has_type_description_service(handle)) {
		return nil
	}
	if message, ok := message.(SchemaMessage); ok {
		return n.RegisterSchema(message.MessageSchema())
	}
	if described, ok := message.(TypeDescriptionMessage); ok {
		schema, err := NewMessageSchemaJSON(described.TypeDescriptionJSON())
		if err != nil {
			return err
		}
		if schema.TypeHash() != message.TypeHash() {
			return fmt.Errorf("schema hash disagrees with message TypeHash")
		}
		return n.RegisterSchema(schema)
	}
	return nil
}
