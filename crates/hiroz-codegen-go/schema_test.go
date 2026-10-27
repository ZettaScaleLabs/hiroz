package main

import (
	"encoding/json"
	"go/parser"
	"go/token"
	"strings"
	"testing"
)

func TestGeneratedTypeDescriptions(t *testing.T) {
	description := json.RawMessage(`{"type_description":{"type_name":"test_msgs/msg/Value","fields":[{"name":"value","type":{"type_id":9,"capacity":0,"string_capacity":0,"nested_type_name":""},"default_value":""}]},"referenced_type_descriptions":[]}`)
	message := MessageDefinition{Package: "test_msgs", Name: "Value", FullName: "test_msgs/Value", TypeHash: "RIHS01_test", Fields: []FieldDefinition{{Name: "value", FieldType: FieldType{Kind: "UInt64"}}}}
	// Decode the manifest field rather than referring to the new struct member,
	// so this same test also compiles on the pre-fix generator.
	if err := json.Unmarshal([]byte(`{"type_description":`+string(description)+`}`), &message); err != nil {
		t.Fatal(err)
	}
	cases := map[string]func() ([]byte, error){
		"message": func() ([]byte, error) { return GenerateGoMessage(message, "test") },
		"service": func() ([]byte, error) {
			return GenerateGoService(ServiceDefinition{Package: "test_msgs", Name: "Service", Request: message, Response: message}, "test")
		},
		"action": func() ([]byte, error) {
			return GenerateGoAction(ActionDefinition{Package: "test_msgs", Name: "Action", Goal: message, Result: &message, Feedback: &message}, "test")
		},
	}
	for name, generate := range cases {
		t.Run(name, func(t *testing.T) {
			source, err := generate()
			if err != nil {
				t.Fatal(err)
			}
			if _, err = parser.ParseFile(token.NewFileSet(), "generated.go", source, parser.AllErrors); err != nil {
				t.Fatal(err)
			}
			if !strings.Contains(string(source), "TypeDescriptionJSON() string") {
				t.Fatalf("schema missing: %s", source)
			}
			if !strings.Contains(string(source), `\"type_id\":9`) {
				t.Fatal("descriptor changed")
			}
		})
	}
}
