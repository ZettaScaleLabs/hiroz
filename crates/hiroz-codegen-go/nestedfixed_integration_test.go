package main

// This file builds and runs the generated Go for a message with a fixed array
// of a nested message. CDR writes a fixed array as its elements only, with no
// 4-byte length prefix.

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

const nestedFixedProbeSource = `package main_test

import (
	"testing"

	"nestedprobe/generated/test_msgs"
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
	// A skip is not a pass: the reason names the missing tool.
	goBin, err := exec.LookPath("go")
	if err != nil {
		t.Skipf("go binary not found on PATH; skipping nested fixed array test: %v", err)
	}

	field := func(name string, ft FieldType, arrayKind string, size uint) FieldDefinition {
		f := FieldDefinition{Name: name, FieldType: ft, IsArray: arrayKind != "", ArrayKind: arrayKind}
		if arrayKind == "fixed" {
			f.ArraySize = uintPtr(size)
		}
		return f
	}
	prim := func(kind string) FieldType { return FieldType{Kind: kind} }
	inner := "Inner"
	pkg := "test_msgs"
	msgs := []MessageDefinition{
		{Package: "test_msgs", Name: "Inner", FullName: "test_msgs/Inner", Fields: []FieldDefinition{
			field("a", prim("Int32"), "", 0), field("b", prim("Int32"), "", 0),
		}},
		{Package: "test_msgs", Name: "Outer", FullName: "test_msgs/Outer", Fields: []FieldDefinition{
			field("f", FieldType{Kind: "Custom", Package: &pkg, Name: &inner}, "fixed", 2),
			field("tail", prim("Int32"), "", 0),
		}},
		{Package: "test_msgs", Name: "FixedOnly", FullName: "test_msgs/FixedOnly", Fields: []FieldDefinition{
			field("f", FieldType{Kind: "Custom", Package: &pkg, Name: &inner}, "fixed", 2),
		}},
	}

	tmp := t.TempDir()
	if err := os.WriteFile(filepath.Join(tmp, "go.mod"), []byte("module nestedprobe\n\ngo 1.23\n"), 0o644); err != nil {
		t.Fatalf("write go.mod: %v", err)
	}
	for _, m := range msgs {
		code, err := GenerateGoMessage(m, "nestedprobe")
		if err != nil {
			t.Fatalf("GenerateGoMessage(%s) failed: %v", m.Name, err)
		}
		dir := filepath.Join(tmp, "generated", m.Package)
		if err := os.MkdirAll(dir, 0o755); err != nil {
			t.Fatalf("mkdir %s: %v", dir, err)
		}
		if err := os.WriteFile(filepath.Join(dir, strings.ToLower(m.Name)+".go"), code, 0o644); err != nil {
			t.Fatalf("write %s: %v", m.Name, err)
		}
	}
	if err := os.WriteFile(filepath.Join(tmp, "probe_test.go"), []byte(nestedFixedProbeSource), 0o644); err != nil {
		t.Fatalf("write probe_test.go: %v", err)
	}

	run := func(args ...string) string {
		t.Helper()
		cmd := exec.Command(goBin, args...)
		cmd.Dir = tmp
		cmd.Env = append(os.Environ(),
			"GOFLAGS=-mod=mod",
			"GOMODCACHE="+filepath.Join(tmp, ".gomodcache"),
			"GOCACHE="+filepath.Join(tmp, ".gocache"),
		)
		out, err := cmd.CombinedOutput()
		if err != nil {
			t.Fatalf("go %s failed: %v\n----- combined output -----\n%s", strings.Join(args, " "), err, strings.TrimSpace(string(out)))
		}
		return string(out)
	}

	run("build", "./generated/...")
	run("vet", "./...")
	out := run("test", "-count=1", "-v", ".")
	for _, name := range []string{"TestNestedFixedWireImage", "TestNestedFixedRoundtrip"} {
		if !strings.Contains(out, "--- PASS: "+name) {
			t.Errorf("probe printed no PASS line for %s:\n%s", name, out)
		}
	}
}
