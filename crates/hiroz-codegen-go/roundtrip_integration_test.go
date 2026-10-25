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
// runtime. See https://github.com/ZettaScaleLabs/hiroz/issues/182.

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

type crossFailure int

const (
	crossOther crossFailure = iota
	// The toolchain has no such GOOS/GOARCH pair (for example darwin/386).
	crossUnsupportedPair
	// The binary was built but the host cannot execute it.
	crossExecFormat
)

// classifyCrossFailure sorts the output of a failed cross-arch `go test`.
// Only these two precise messages count as an environment limit. Any other
// failure stays a failure.
func classifyCrossFailure(output string) crossFailure {
	switch {
	case strings.Contains(output, "unsupported GOOS/GOARCH pair"):
		return crossUnsupportedPair
	case strings.Contains(output, "exec format error"):
		return crossExecFormat
	}
	return crossOther
}

// targetListed reports whether goos/goarch appears in `go tool dist list`
// output (one "os/arch" pair per line).
func targetListed(distList, goos, goarch string) bool {
	want := goos + "/" + goarch
	for _, line := range strings.Split(distList, "\n") {
		if strings.TrimSpace(line) == want {
			return true
		}
	}
	return false
}

// targetBuildable asks the toolchain whether the host GOOS with goarch is a
// supported target. When the toolchain cannot answer, it returns true so the
// probe runs and fails loudly instead of skipping silently.
func targetBuildable(goBin, goarch string) (bool, string) {
	goosOut, err := exec.Command(goBin, "env", "GOOS").Output()
	if err != nil {
		return true, ""
	}
	goos := strings.TrimSpace(string(goosOut))
	list, err := exec.Command(goBin, "tool", "dist", "list").Output()
	if err != nil {
		return true, ""
	}
	if !targetListed(string(list), goos, goarch) {
		return false, goos + "/" + goarch + " is absent from `go tool dist list`"
	}
	return true, ""
}

func TestCrossFailureClassification(t *testing.T) {
	for _, c := range []struct {
		name string
		out  string
		want crossFailure
	}{
		{"unsupported", "go: unsupported GOOS/GOARCH pair darwin/386", crossUnsupportedPair},
		{"exec", "fork/exec x.test: exec format error", crossExecFormat},
		{"compile", "./x.go:3:1: syntax error", crossOther},
		{"empty", "", crossOther},
	} {
		if got := classifyCrossFailure(c.out); got != c.want {
			t.Errorf("%s: got %d, want %d", c.name, got, c.want)
		}
	}
	list := "darwin/amd64\ndarwin/arm64\nlinux/386\nlinux/amd64\n"
	for _, c := range []struct {
		goos, arch string
		want       bool
	}{
		{"linux", "386", true},
		{"darwin", "386", false},
		{"darwin", "arm64", true},
		{"linux", "38", false},
	} {
		if got := targetListed(list, c.goos, c.arch); got != c.want {
			t.Errorf("%s/%s: got %v, want %v", c.goos, c.arch, got, c.want)
		}
	}
}

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

// hostileCase is one decode probe run in its own `go test` subprocess. A
// subprocess per case matters: a Go out-of-memory abort kills the whole
// process and cannot be recovered, so one shared process would hide every case
// after the first abort.
type hostileCase struct {
	name   string
	goarch string // empty: the host architecture
}

// hostileProbeSource holds the probe tests of the hostile-length round trip.
// Each probe feeds a hand-built CDR body to one generated decoder.
//
// The bytes of every hostile body are `00 01 00 00` (encapsulation header),
// then a 4-byte sequence length of 0xFFFFFFFF, then nothing.
const hostileProbeSource = `package main_test

import (
	"runtime"
	"testing"

	"roundtripprobe/test_msgs"
)

var hostileLen = []byte{0x00, 0x01, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff}

// allocated runs f and returns the bytes the Go allocator handed out during it.
// A reservation counts even when the OS has not touched its pages, so the
// measure does not depend on overcommit settings.
func allocated(f func()) uint64 {
	var a, b runtime.MemStats
	runtime.ReadMemStats(&a)
	f()
	runtime.ReadMemStats(&b)
	return b.TotalAlloc - a.TotalAlloc
}

// rejects decodes raw and requires an error, a small allocation and no panic.
func rejects(t *testing.T, raw []byte, decode func([]byte) error) {
	t.Helper()
	var err error
	n := allocated(func() { err = decode(raw) })
	if err == nil {
		t.Errorf("hostile length decoded without an error")
	}
	if n > 1<<20 {
		t.Errorf("hostile length reserved %d bytes before it failed", n)
	}
}

func TestHostileStringArray(t *testing.T) {
	var m test_msgs.StrArr
	rejects(t, hostileLen, m.DeserializeCDR)
}

func TestHostilePrimitiveArray(t *testing.T) {
	var m test_msgs.PrimArr
	rejects(t, hostileLen, m.DeserializeCDR)
}

func TestHostileCustomArray(t *testing.T) {
	var m test_msgs.CustomArr
	rejects(t, hostileLen, m.DeserializeCDR)
}

// A length one above the bytes left is rejected; a length equal to the bytes
// left decodes. The pair pins the boundary from both sides.
func TestPrimitiveBoundary(t *testing.T) {
	var ok test_msgs.BytesArr
	if err := ok.DeserializeCDR([]byte{0x00, 0x01, 0x00, 0x00, 3, 0, 0, 0, 7, 8, 9}); err != nil {
		t.Fatalf("length equal to the bytes left was rejected: %v", err)
	}
	if len(ok.V) != 3 || ok.V[0] != 7 || ok.V[2] != 9 {
		t.Fatalf("decoded %v, want [7 8 9]", ok.V)
	}
	var over test_msgs.BytesArr
	if err := over.DeserializeCDR([]byte{0x00, 0x01, 0x00, 0x00, 4, 0, 0, 0, 7, 8, 9}); err == nil {
		t.Fatalf("length one above the bytes left decoded without an error")
	}
}

// An element of a message with no fields takes zero bytes, so a valid
// sequence of them is longer than the bytes left. The bound must not reject it.
func TestSequenceOfEmptyMessagesDecodes(t *testing.T) {
	var m test_msgs.EmptyArr
	if err := m.DeserializeCDR([]byte{0x00, 0x01, 0x00, 0x00, 0xe8, 0x03, 0, 0}); err != nil {
		t.Fatalf("a valid sequence of 1000 empty messages was rejected: %v", err)
	}
	if len(m.V) != 1000 {
		t.Fatalf("decoded %d elements, want 1000", len(m.V))
	}
}
`

func TestRoundtripRejectsHostileSequenceLength(t *testing.T) {
	// A skip is not a pass: the reason names the missing tool.
	goBin, err := exec.LookPath("go")
	if err != nil {
		t.Skipf("go binary not found on PATH; skipping hostile sequence length test: %v", err)
	}

	u8 := FieldType{Kind: "UInt8"}
	seq := func(name string, ft FieldType) []FieldDefinition {
		return []FieldDefinition{{Name: "v", FieldType: ft, IsArray: true, ArrayKind: "unbounded"}}
	}
	str := func(s string) *string { return &s }
	custom := func(name string) FieldType {
		return FieldType{Kind: "Custom", Package: str("test_msgs"), Name: str(name)}
	}
	// Time and Duration arrays have no runtime probe: the generated code for such
	// a field does not compile (the slice element type falls back to interface{}).
	// Their bound is checked as text in TestGenerateUnpackBoundsArrayLength.
	msgs := []MessageDefinition{
		{Package: "test_msgs", Name: "Empty", FullName: "test_msgs/Empty"},
		{Package: "test_msgs", Name: "One", FullName: "test_msgs/One", Fields: []FieldDefinition{
			{Name: "x", FieldType: FieldType{Kind: "Int32"}},
		}},
		{Package: "test_msgs", Name: "StrArr", FullName: "test_msgs/StrArr", Fields: seq("v", FieldType{Kind: "String"})},
		{Package: "test_msgs", Name: "PrimArr", FullName: "test_msgs/PrimArr", Fields: seq("v", FieldType{Kind: "Float64"})},
		{Package: "test_msgs", Name: "BytesArr", FullName: "test_msgs/BytesArr", Fields: seq("v", u8)},
		{Package: "test_msgs", Name: "CustomArr", FullName: "test_msgs/CustomArr", Fields: seq("v", custom("One"))},
		{Package: "test_msgs", Name: "EmptyArr", FullName: "test_msgs/EmptyArr", Fields: seq("v", custom("Empty"))},
	}

	tmp := t.TempDir()
	if err := os.WriteFile(filepath.Join(tmp, "go.mod"), []byte("module roundtripprobe\n\ngo 1.23\n"), 0o644); err != nil {
		t.Fatalf("write go.mod: %v", err)
	}
	for _, m := range msgs {
		code, err := GenerateGoMessage(m, "roundtripprobe")
		if err != nil {
			t.Fatalf("GenerateGoMessage(%s) failed: %v", m.Name, err)
		}
		dir := filepath.Join(tmp, m.Package)
		if err := os.MkdirAll(dir, 0o755); err != nil {
			t.Fatalf("mkdir %s: %v", dir, err)
		}
		if err := os.WriteFile(filepath.Join(dir, strings.ToLower(m.Name)+".go"), code, 0o644); err != nil {
			t.Fatalf("write %s: %v", m.Name, err)
		}
	}
	if err := os.WriteFile(filepath.Join(tmp, "probe_test.go"), []byte(hostileProbeSource), 0o644); err != nil {
		t.Fatalf("write probe_test.go: %v", err)
	}

	cases := []hostileCase{
		{name: "TestHostileStringArray"},
		{name: "TestHostilePrimitiveArray"},
		{name: "TestHostileCustomArray"},
		{name: "TestPrimitiveBoundary"},
		{name: "TestSequenceOfEmptyMessagesDecodes"},
		// On a 32-bit target int(uint32) of 0xFFFFFFFF is negative.
		{name: "TestHostilePrimitiveArray", goarch: "386"},
		{name: "TestHostileStringArray", goarch: "386"},
	}
	for _, c := range cases {
		label := c.name
		if c.goarch != "" {
			label += "/" + c.goarch
		}
		t.Run(label, func(t *testing.T) {
			if c.goarch != "" {
				if ok, why := targetBuildable(goBin, c.goarch); !ok {
					t.Skipf("GOARCH=%s is not a buildable target on this host: %s", c.goarch, why)
				}
			}
			cmd := exec.Command(goBin, "test", "-count=1", "-run", "^"+c.name+"$", "-v", ".")
			cmd.Dir = tmp
			cmd.Env = append(os.Environ(),
				"GOFLAGS=-mod=mod",
				"GOMODCACHE="+filepath.Join(tmp, ".gomodcache"),
				"GOCACHE="+filepath.Join(tmp, ".gocache"),
			)
			if c.goarch != "" {
				cmd.Env = append(cmd.Env, "GOARCH="+c.goarch, "CGO_ENABLED=0")
			}
			out, err := cmd.CombinedOutput()
			text := string(out)
			if err != nil {
				switch classifyCrossFailure(text) {
				case crossUnsupportedPair:
					if c.goarch != "" {
						t.Skipf("GOARCH=%s: Go reports an unsupported GOOS/GOARCH pair on this host", c.goarch)
					}
				case crossExecFormat:
					if c.goarch != "" {
						t.Skipf("host cannot run GOARCH=%s binaries: UNKNOWN, not a pass", c.goarch)
					}
				}
				t.Fatalf("probe %s failed: %v\n----- combined output -----\n%s", label, err, strings.TrimSpace(text))
			}
			if !strings.Contains(text, "--- PASS: "+c.name) {
				t.Fatalf("probe %s printed no PASS line:\n%s", label, text)
			}
		})
	}
}
