package main

// This file is the runtime-compile integration test for the Go message
// codegen. Unlike the other tests in this package, which assert against the
// generated source as a *string*, this test:
//
//   1. Calls GenerateGoMessage for a representative message
//      (`string name; float32 value`) — the canonical alignment-bug case.
//   2. Writes the output to a fresh temp directory alongside its own
//      `go.mod` and a small `_test.go` probe.
//   3. Shells out to `go test` in that temp directory so the generated code
//      is *compiled and executed* — not just text-matched.
//
// The probe verifies two things:
//
//   - Serialize side: the exact CDR-LE byte sequence is produced
//     (encapsulation header + length prefix + string bytes + null terminator
//     + 1 byte of alignment padding + float32 little-endian bits).
//   - Round-trip side: feeding those bytes back through DeserializeCDR
//     reproduces the original field values.
//
// A subtle codegen regression — emitting `offset += 4` before a bounds check,
// swapping `buf` and `data` variable names, an off-by-one in the alignment
// formula — would pass every string-matching test but fail this one at
// runtime. See https://github.com/ZettaScaleLabs/ros-z/issues/182.

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func TestRoundtripExecutesGeneratedGo(t *testing.T) {
	// Skip cleanly when `go` is unavailable on the test host (e.g. running
	// `go test` from a stripped CI image). We still want the assertion
	// failure if `go` is present and the generated code is wrong.
	goBin, err := exec.LookPath("go")
	if err != nil {
		t.Skipf("go binary not found on PATH; skipping runtime roundtrip test: %v", err)
	}

	// 1. Generate a Mixed{string,float32} message — the canonical case where
	// a missing 4-byte alignment pad between the string's null terminator
	// and the float32 would yield a wrong-sized buffer or panic.
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

	// The prefix is irrelevant for this message because it has no
	// cross-package custom fields. It still gets baked into the generated
	// code's import paths, so use something obviously local.
	code, err := GenerateGoMessage(msg, "roundtripprobe")
	if err != nil {
		t.Fatalf("GenerateGoMessage failed: %v", err)
	}

	// 2. Lay out a self-contained Go module in a temp dir:
	//
	//   <tmp>/
	//     go.mod                       module roundtripprobe
	//     test_msgs/mixed.go           generated code under test
	//     probe_test.go                runtime assertion probe
	//
	// `t.TempDir()` is auto-cleaned at the end of the test.
	tmp := t.TempDir()

	pkgDir := filepath.Join(tmp, "test_msgs")
	if err := os.MkdirAll(pkgDir, 0o755); err != nil {
		t.Fatalf("mkdir test_msgs: %v", err)
	}
	if err := os.WriteFile(filepath.Join(pkgDir, "mixed.go"), code, 0o644); err != nil {
		t.Fatalf("write mixed.go: %v", err)
	}

	// Minimal go.mod. The module path is fully self-contained — the probe
	// imports `roundtripprobe/test_msgs` — so the temp tree never reaches
	// out to the host module graph.
	goMod := "module roundtripprobe\n\ngo 1.23\n"
	if err := os.WriteFile(filepath.Join(tmp, "go.mod"), []byte(goMod), 0o644); err != nil {
		t.Fatalf("write go.mod: %v", err)
	}

	if err := os.WriteFile(filepath.Join(tmp, "probe_test.go"), []byte(probeSource), 0o644); err != nil {
		t.Fatalf("write probe_test.go: %v", err)
	}

	// 3. Compile + run. `go test ./...` recursively builds and tests every
	// package in the temp module — i.e. it forces a real compile of the
	// generated code and executes the probe assertions. We disable cache
	// and module download paths to keep the test hermetic and fast.
	cmd := exec.Command(goBin, "test", "-count=1", "./...")
	cmd.Dir = tmp
	cmd.Env = append(os.Environ(),
		"GOFLAGS=-mod=mod",
		// Don't pollute the host's module cache with the throwaway probe.
		"GOMODCACHE="+filepath.Join(tmp, ".gomodcache"),
		"GOCACHE="+filepath.Join(tmp, ".gocache"),
	)
	out, err := cmd.CombinedOutput()
	if err != nil {
		// Dump the generated code on failure: the runtime error message is
		// rarely enough on its own to diagnose a codegen bug.
		t.Logf("---- generated code (test_msgs/mixed.go) ----\n%s", string(code))
		t.Logf("---- probe source (probe_test.go) ----\n%s", probeSource)
		t.Fatalf("go test in temp dir failed: %v\n----- combined output -----\n%s",
			err, strings.TrimSpace(string(out)))
	}
}

// probeSource is the body of the temp module's `probe_test.go`. It is kept
// here as a const so the integration test stays a single self-contained
// file — no `testdata/` fixture to drift out of sync with the assertions.
//
// The expected byte sequence is hard-coded from the CDR-LE spec for
// Mixed{Name: "hi", Value: 1.0}:
//
//	00 01 00 00   encapsulation header (CDR_LE, options = 0)
//	03 00 00 00   string length prefix = len("hi") + 1 = 3
//	68 69 00      "hi" + null terminator
//	00            1 byte alignment pad (body offset 7 -> 8, align 4)
//	00 00 80 3f   float32(1.0) little-endian (0x3f800000)
const probeSource = `package main_test

import (
	"bytes"
	"testing"

	"roundtripprobe/test_msgs"
)

// expectedCDR is the exact wire image for Mixed{Name: "hi", Value: 1.0}.
// Any deviation — wrong length prefix, missing null terminator, missing
// alignment pad, wrong float32 byte order — will fail this comparison.
var expectedCDR = []byte{
	0x00, 0x01, 0x00, 0x00, // CDR_LE encapsulation header
	0x03, 0x00, 0x00, 0x00, // string length prefix (len("hi")+1)
	0x68, 0x69, 0x00, //       "hi\0"
	0x00,                   // padding byte (offset 7 -> 8 for float32 align)
	0x00, 0x00, 0x80, 0x3f, // float32(1.0) little-endian
}

func TestSerializeExactBytes(t *testing.T) {
	m := &test_msgs.Mixed{Name: "hi", Value: 1.0}
	got, err := m.SerializeCDR()
	if err != nil {
		t.Fatalf("SerializeCDR failed: %v", err)
	}
	if !bytes.Equal(got, expectedCDR) {
		t.Fatalf("CDR serialization mismatch.\n  got:  % x\n  want: % x", got, expectedCDR)
	}
}

func TestRoundtripValuesMatch(t *testing.T) {
	original := &test_msgs.Mixed{Name: "hi", Value: 1.0}
	raw, err := original.SerializeCDR()
	if err != nil {
		t.Fatalf("SerializeCDR failed: %v", err)
	}

	var decoded test_msgs.Mixed
	if err := decoded.DeserializeCDR(raw); err != nil {
		t.Fatalf("DeserializeCDR failed: %v", err)
	}

	if decoded.Name != original.Name {
		t.Errorf("Name roundtrip mismatch: got %q, want %q", decoded.Name, original.Name)
	}
	if decoded.Value != original.Value {
		t.Errorf("Value roundtrip mismatch: got %v, want %v", decoded.Value, original.Value)
	}
}
`
