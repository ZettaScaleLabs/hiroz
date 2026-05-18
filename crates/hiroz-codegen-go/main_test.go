package main

import (
	"strings"
	"testing"
)

func TestSanitizePackageName(t *testing.T) {
	tests := []struct {
		input    string
		expected string
	}{
		{"std_msgs", "std_msgs"},
		{"example-interfaces", "example_interfaces"},
		{"my-package", "my_package"},
	}

	for _, test := range tests {
		result := sanitizePackageName(test.input)
		if result != test.expected {
			t.Errorf("sanitizePackageName(%q) = %q, expected %q", test.input, result, test.expected)
		}
	}
}

func TestGenerateGoMessage(t *testing.T) {
	msg := MessageDefinition{
		Package:  "std_msgs",
		Name:     "String",
		FullName: "std_msgs/String",
		TypeHash: "RIHS01_1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
		Fields: []FieldDefinition{
			{
				Name:      "data",
				FieldType: FieldType{Kind: "String"},
				IsArray:   false,
			},
		},
		Constants: []ConstantDefinition{},
	}

	code, err := GenerateGoMessage(msg, "hiroz")
	if err != nil {
		t.Fatalf("GenerateGoMessage failed: %v", err)
	}

	codeStr := string(code)

	// Verify the generated code contains expected elements
	expectedElements := []string{
		"package std_msgs",
		"type String struct",
		"Data string",
		"String_TypeName = \"std_msgs::msg::dds_::String_\"",
		"String_TypeHash = \"RIHS01_",
		"func (m *String) TypeName() string",
		"func (m *String) TypeHash() string",
		"func (m *String) SerializeCDR() ([]byte, error)",
		"func (m *String) DeserializeCDR(data []byte) error",
		"func (m *String) PackToRawAt(buf []byte, offset int) ([]byte, int)",
		"func (m *String) UnpackFromRawAt(data []byte, offset int) (int, error)",
	}

	for _, element := range expectedElements {
		if !strings.Contains(codeStr, element) {
			t.Errorf("Generated code missing expected element: %q", element)
		}
	}
}

func TestFieldToGoType(t *testing.T) {
	tests := []struct {
		field    FieldDefinition
		expected string
	}{
		{
			FieldDefinition{
				FieldType: FieldType{Kind: "String"},
				IsArray:   false,
			},
			"string",
		},
		{
			FieldDefinition{
				FieldType: FieldType{Kind: "Int32"},
				IsArray:   false,
			},
			"int32",
		},
		{
			FieldDefinition{
				FieldType: FieldType{Kind: "Float64"},
				IsArray:   false,
			},
			"float64",
		},
		{
			FieldDefinition{
				FieldType: FieldType{Kind: "Bool"},
				IsArray:   false,
			},
			"bool",
		},
		{
			FieldDefinition{
				FieldType: FieldType{Kind: "Custom", Package: stringPtr("std_msgs"), Name: stringPtr("Header")},
				IsArray:   false,
			},
			"std_msgs.Header",
		},
		{
			FieldDefinition{
				FieldType: FieldType{Kind: "String"},
				IsArray:   true,
				ArraySize: nil,
			},
			"[]string",
		},
		{
			FieldDefinition{
				FieldType: FieldType{Kind: "Int32"},
				IsArray:   true,
				ArrayKind: "fixed",
				ArraySize: uintPtr(10),
			},
			"[10]int32",
		},
	}

	for _, test := range tests {
		result := fieldToGoType(test.field)
		if result != test.expected {
			t.Errorf("fieldToGoType(%+v) = %q, expected %q", test.field, result, test.expected)
		}
	}
}

func TestCdrAlignment(t *testing.T) {
	cases := map[string]int{
		"Bool": 1, "Int8": 1, "UInt8": 1,
		"Int16": 2, "UInt16": 2,
		"Int32": 4, "UInt32": 4, "Float32": 4, "String": 4, "Time": 4, "Duration": 4,
		"Int64": 8, "UInt64": 8, "Float64": 8,
	}
	for kind, want := range cases {
		if got := cdrAlignment(kind); got != want {
			t.Errorf("cdrAlignment(%q) = %d, want %d", kind, got, want)
		}
	}
}

func TestGenerateGoMessageEmitsAlignment(t *testing.T) {
	// A message with a string followed by a float32: the canonical alignment-bug case.
	// After packing the variable-length string, the float32 read needs 4-byte
	// padding before the binary.LittleEndian.Uint32 call.
	msg := MessageDefinition{
		Package:  "test_msgs",
		Name:     "Mixed",
		FullName: "test_msgs/Mixed",
		TypeHash: "RIHS01_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
		Fields: []FieldDefinition{
			{Name: "name", FieldType: FieldType{Kind: "String"}, IsArray: false},
			{Name: "value", FieldType: FieldType{Kind: "Float32"}, IsArray: false},
		},
		Constants: []ConstantDefinition{},
	}

	code, err := GenerateGoMessage(msg, "test")
	if err != nil {
		t.Fatalf("GenerateGoMessage failed: %v", err)
	}
	codeStr := string(code)

	// We expect at least two "(N - offset%N) % N"-shaped pad expressions
	// (one for the string length prefix, one for the float32). Allow either
	// "%4" or "%2" or "%8" depending on the field, but at least one "%4" must
	// appear for the string + float32 case.
	if !strings.Contains(codeStr, "(4 - offset%4) % 4") {
		t.Errorf("Generated unpack code missing 4-byte alignment expression.\nGenerated:\n%s", codeStr)
	}
}

func TestAlignmentInBothPackAndUnpack(t *testing.T) {
	// Regression test for the CDR alignment bug: a string followed by a float32
	// requires 4-byte padding in BOTH the pack path (PackToRawAt) and the unpack
	// path (UnpackFromRawAt). The old generator emitted no padding at all.
	msg := MessageDefinition{
		Package:  "test_msgs",
		Name:     "Mixed",
		FullName: "test_msgs/Mixed",
		TypeHash: "RIHS01_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
		Fields: []FieldDefinition{
			{Name: "name", FieldType: FieldType{Kind: "String"}, IsArray: false},
			{Name: "value", FieldType: FieldType{Kind: "Float32"}, IsArray: false},
		},
		Constants: []ConstantDefinition{},
	}

	code, err := GenerateGoMessage(msg, "test")
	if err != nil {
		t.Fatalf("GenerateGoMessage failed: %v", err)
	}
	codeStr := string(code)

	// The 4-byte alignment expression must appear at least twice: once for the
	// string length prefix and once for the float32 — in both the pack and unpack
	// methods. strings.Count gives a lower bound without requiring exact positions.
	const alignExpr = "(4 - offset%4) % 4"
	n := strings.Count(codeStr, alignExpr)
	if n < 2 {
		t.Errorf("expected at least 2 occurrences of %q (pack + unpack paths), got %d\nGenerated:\n%s",
			alignExpr, n, codeStr)
	}

	if !strings.Contains(codeStr, "func (m *Mixed) PackToRawAt(") {
		t.Error("Generated code missing PackToRawAt method")
	}
	if !strings.Contains(codeStr, "func (m *Mixed) UnpackFromRawAt(") {
		t.Error("Generated code missing UnpackFromRawAt method")
	}
}

func TestTimeDurationPackGeneration(t *testing.T) {
	// Regression test for the Time/Duration pack bug: the old generatePackField
	// had no case for Time/Duration, so those fields were silently dropped on
	// serialization. Verify that the generated pack and unpack methods both
	// delegate to the nested type's PackToRawAt / UnpackFromRawAt.
	msg := MessageDefinition{
		Package:  "std_msgs",
		Name:     "Header",
		FullName: "std_msgs/msg/Header",
		TypeHash: "RIHS01_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
		Fields: []FieldDefinition{
			{Name: "stamp", FieldType: FieldType{Kind: "Time"}, IsArray: false},
			{Name: "frame_id", FieldType: FieldType{Kind: "String"}, IsArray: false},
		},
		Constants: []ConstantDefinition{},
	}

	code, err := GenerateGoMessage(msg, "test")
	if err != nil {
		t.Fatalf("GenerateGoMessage failed: %v", err)
	}
	codeStr := string(code)

	if !strings.Contains(codeStr, ".PackToRawAt(buf, offset)") {
		t.Errorf("Generated pack code does not call PackToRawAt for Time field.\nGenerated:\n%s", codeStr)
	}
	if !strings.Contains(codeStr, ".UnpackFromRawAt(data, offset)") {
		t.Errorf("Generated unpack code does not call UnpackFromRawAt for Time field.\nGenerated:\n%s", codeStr)
	}
	if !strings.Contains(codeStr, "var err error") {
		t.Errorf("Generated UnpackFromRawAt is missing 'var err error' declaration.\nGenerated:\n%s", codeStr)
	}
}

func TestCapitalize(t *testing.T) {
	tests := []struct {
		input    string
		expected string
	}{
		{"data", "Data"},
		{"count", "Count"},
		{"", ""},
		{"a", "A"},
		{"ABC", "ABC"},
	}

	for _, test := range tests {
		result := capitalize(test.input)
		if result != test.expected {
			t.Errorf("capitalize(%q) = %q, expected %q", test.input, result, test.expected)
		}
	}
}

func stringPtr(s string) *string {
	return &s
}

func uintPtr(u uint) *uint {
	return &u
}

func TestGenerateGoMessageWithAllTypes(t *testing.T) {
	msg := MessageDefinition{
		Package:  "test_msgs",
		Name:     "AllTypes",
		FullName: "test_msgs/AllTypes",
		TypeHash: "RIHS01_abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
		Fields: []FieldDefinition{
			{Name: "bool_field", FieldType: FieldType{Kind: "Bool"}, IsArray: false},
			{Name: "int8_field", FieldType: FieldType{Kind: "Int8"}, IsArray: false},
			{Name: "uint8_field", FieldType: FieldType{Kind: "UInt8"}, IsArray: false},
			{Name: "int16_field", FieldType: FieldType{Kind: "Int16"}, IsArray: false},
			{Name: "uint16_field", FieldType: FieldType{Kind: "UInt16"}, IsArray: false},
			{Name: "int32_field", FieldType: FieldType{Kind: "Int32"}, IsArray: false},
			{Name: "uint32_field", FieldType: FieldType{Kind: "UInt32"}, IsArray: false},
			{Name: "int64_field", FieldType: FieldType{Kind: "Int64"}, IsArray: false},
			{Name: "uint64_field", FieldType: FieldType{Kind: "UInt64"}, IsArray: false},
			{Name: "float32_field", FieldType: FieldType{Kind: "Float32"}, IsArray: false},
			{Name: "float64_field", FieldType: FieldType{Kind: "Float64"}, IsArray: false},
			{Name: "string_field", FieldType: FieldType{Kind: "String"}, IsArray: false},
		},
		Constants: []ConstantDefinition{},
	}

	code, err := GenerateGoMessage(msg, "test")
	if err != nil {
		t.Fatalf("GenerateGoMessage failed: %v", err)
	}

	codeStr := string(code)

	// Verify struct fields exist (capitalize() only capitalizes the first letter)
	// The expected pattern is "FieldName type" but gofmt may change spacing
	expectedFieldNames := []string{
		"Bool_field",
		"Int8_field",
		"Uint8_field",
		"Int16_field",
		"Uint16_field",
		"Int32_field",
		"Uint32_field",
		"Int64_field",
		"Uint64_field",
		"Float32_field",
		"Float64_field",
		"String_field",
	}

	for _, fieldName := range expectedFieldNames {
		if !strings.Contains(codeStr, fieldName) {
			t.Errorf("Generated code missing expected field name: %q\nGenerated:\n%s", fieldName, codeStr[:500])
		}
	}

	// Verify math import for float types
	if !strings.Contains(codeStr, `"math"`) {
		t.Error("Generated code should import math for float types")
	}
}

func TestGenerateGoMessageWithArrays(t *testing.T) {
	fixedSize := uint(5)
	msg := MessageDefinition{
		Package:  "test_msgs",
		Name:     "ArrayTypes",
		FullName: "test_msgs/ArrayTypes",
		TypeHash: "RIHS01_1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
		Fields: []FieldDefinition{
			{Name: "unbounded_strings", FieldType: FieldType{Kind: "String"}, IsArray: true, ArrayKind: "unbounded"},
			{Name: "fixed_ints", FieldType: FieldType{Kind: "Int32"}, IsArray: true, ArrayKind: "fixed", ArraySize: &fixedSize},
			{Name: "bounded_floats", FieldType: FieldType{Kind: "Float64"}, IsArray: true, ArrayKind: "bounded", ArraySize: &fixedSize},
		},
		Constants: []ConstantDefinition{},
	}

	code, err := GenerateGoMessage(msg, "test")
	if err != nil {
		t.Fatalf("GenerateGoMessage failed: %v", err)
	}

	codeStr := string(code)

	// Verify array fields exist by name
	expectedFieldNames := []string{
		"Unbounded_strings",
		"Fixed_ints",
		"Bounded_floats",
	}

	for _, fieldName := range expectedFieldNames {
		if !strings.Contains(codeStr, fieldName) {
			t.Errorf("Generated code missing expected array field name: %q\nGenerated:\n%s", fieldName, codeStr[:500])
		}
	}

	// Verify array type syntax
	if !strings.Contains(codeStr, "[]string") {
		t.Error("Generated code should contain unbounded string array type")
	}
	if !strings.Contains(codeStr, "[5]int32") {
		t.Error("Generated code should contain fixed int32 array type")
	}
	if !strings.Contains(codeStr, "[]float64") {
		t.Error("Generated code should contain bounded float64 array type")
	}
}

func TestGenerateGoService(t *testing.T) {
	srv := ServiceDefinition{
		Package:  "example_interfaces",
		Name:     "AddTwoInts",
		FullName: "example_interfaces/srv/AddTwoInts",
		TypeHash: "RIHS01_service12345678901234567890123456789012345678901234567890123456",
		Request: MessageDefinition{
			Package:  "example_interfaces",
			Name:     "AddTwoInts_Request",
			FullName: "example_interfaces/srv/AddTwoInts_Request",
			TypeHash: "RIHS01_request12345678901234567890123456789012345678901234567890123456",
			Fields: []FieldDefinition{
				{Name: "a", FieldType: FieldType{Kind: "Int64"}, IsArray: false},
				{Name: "b", FieldType: FieldType{Kind: "Int64"}, IsArray: false},
			},
		},
		Response: MessageDefinition{
			Package:  "example_interfaces",
			Name:     "AddTwoInts_Response",
			FullName: "example_interfaces/srv/AddTwoInts_Response",
			TypeHash: "RIHS01_response1234567890123456789012345678901234567890123456789012",
			Fields: []FieldDefinition{
				{Name: "sum", FieldType: FieldType{Kind: "Int64"}, IsArray: false},
			},
		},
	}

	code, err := GenerateGoService(srv, "test")
	if err != nil {
		t.Fatalf("GenerateGoService failed: %v", err)
	}

	codeStr := string(code)

	// Verify service elements
	expectedElements := []string{
		"package example_interfaces",
		"type AddTwoInts struct{}",
		"AddTwoInts_TypeName = \"example_interfaces::srv::dds_::AddTwoInts_\"",
		"type AddTwoIntsRequest struct",
		"type AddTwoIntsResponse struct",
		"A int64",
		"B int64",
		"Sum int64",
		"func (m *AddTwoIntsRequest) SerializeCDR()",
		"func (m *AddTwoIntsResponse) DeserializeCDR(",
	}

	for _, element := range expectedElements {
		if !strings.Contains(codeStr, element) {
			t.Errorf("Generated service code missing expected element: %q", element)
		}
	}
}

func TestGenerateUnpackField(t *testing.T) {
	// Test that unpack code is generated (not just TODO comments)
	msg := MessageDefinition{
		Package:  "test_msgs",
		Name:     "Simple",
		FullName: "test_msgs/Simple",
		TypeHash: "RIHS01_simple12345678901234567890123456789012345678901234567890123456",
		Fields: []FieldDefinition{
			{Name: "value", FieldType: FieldType{Kind: "Int32"}, IsArray: false},
			{Name: "name", FieldType: FieldType{Kind: "String"}, IsArray: false},
		},
		Constants: []ConstantDefinition{},
	}

	code, err := GenerateGoMessage(msg, "test")
	if err != nil {
		t.Fatalf("GenerateGoMessage failed: %v", err)
	}

	codeStr := string(code)

	// Verify unpack methods are properly generated (not just TODOs)
	if strings.Contains(codeStr, "// TODO: unpack") {
		t.Error("Unpack code should be fully implemented, not contain TODO comments")
	}

	// Check for actual unpack logic
	expectedUnpackElements := []string{
		"binary.LittleEndian.Uint32(data[offset:])",
		"m.Value = int32(",
		"strLen := int(binary.LittleEndian.Uint32(data[offset:]))",
	}

	for _, element := range expectedUnpackElements {
		if !strings.Contains(codeStr, element) {
			t.Errorf("Generated unpack code missing expected element: %q", element)
		}
	}
}
