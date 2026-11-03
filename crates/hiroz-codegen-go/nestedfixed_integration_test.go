package main

import (
	"strings"
	"testing"
)

const nestedFixedProbeSource = `package main_test

import (
	"testing"

	"timeprobe/generated/test_msgs"
)

func sample() *test_msgs.Outer {
	return &test_msgs.Outer{
		F:    [2]test_msgs.Inner{{A: 1, B: 2}, {A: 3, B: 4}},
		Tail: 5,
	}
}

func TestNestedFixedWireImage(t *testing.T) {
	got, err := sample().SerializeCDR()
	if err != nil {
		t.Fatalf("SerializeCDR failed: %v", err)
	}
	want := []byte{0x00, 0x01, 0x00, 0x00}
	for _, w := range []uint32{1, 2, 3, 4, 5} {
		want = append(want, byte(w), byte(w>>8), byte(w>>16), byte(w>>24))
	}
	if string(got) != string(want) {
		t.Fatalf("wire image mismatch.\n  got:  % x\n  want: % x", got, want)
	}
}

func TestNestedFixedRoundtrip(t *testing.T) {
	raw, err := sample().SerializeCDR()
	if err != nil {
		t.Fatalf("SerializeCDR failed: %v", err)
	}
	var d test_msgs.Outer
	if err := d.DeserializeCDR(raw); err != nil {
		t.Fatalf("DeserializeCDR failed: %v", err)
	}
	if d != *sample() {
		t.Errorf("decoded %v, want %v", d, *sample())
	}
}
`

func TestNestedFixedArrayCompileAndRoundtrip(t *testing.T) {
	m := newProbeModule(t)
	inner := "Inner"
	pkg := "test_msgs"
	msgs := []MessageDefinition{
		{Package: "test_msgs", Name: "Inner", FullName: "test_msgs/Inner", Fields: []FieldDefinition{
			probeField("a", "Int32", "", 0), probeField("b", "Int32", "", 0),
		}},
		{Package: "test_msgs", Name: "Outer", FullName: "test_msgs/Outer", Fields: []FieldDefinition{
			{Name: "f", FieldType: FieldType{Kind: "Custom", Package: &pkg, Name: &inner}, IsArray: true, ArrayKind: "fixed", ArraySize: uintPtr(2)},
			probeField("tail", "Int32", "", 0),
		}},
		{Package: "test_msgs", Name: "FixedOnly", FullName: "test_msgs/FixedOnly", Fields: []FieldDefinition{
			{Name: "f", FieldType: FieldType{Kind: "Custom", Package: &pkg, Name: &inner}, IsArray: true, ArrayKind: "fixed", ArraySize: uintPtr(2)},
		}},
	}

	for _, msg := range msgs {
		m.message(msg)
	}
	m.write("probe_test.go", nestedFixedProbeSource)
	m.run("vet", "./...")
	out := m.run("test", "-count=1", "-v", ".")
	for _, name := range []string{"TestNestedFixedWireImage", "TestNestedFixedRoundtrip"} {
		if !strings.Contains(out, "--- PASS: "+name) {
			t.Errorf("probe printed no PASS line for %s:\n%s", name, out)
		}
	}
}
