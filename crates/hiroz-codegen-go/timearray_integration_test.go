package main

// This file compiles and runs the generated Go for messages whose fields are
// sequences and fixed arrays of builtin_interfaces/Time and
// builtin_interfaces/Duration. The tests in main_test.go read generated text;
// this one builds it with the Go compiler, so a field type that does not
// compile fails here.

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

const timeArrayProbeSource = `package main_test

import (
	"runtime"
	"testing"

	"timeprobe/generated/builtin_interfaces"
	"timeprobe/generated/test_msgs"
)

var hostileLen = []byte{0x00, 0x01, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff}

func sample() *test_msgs.TimeArrays {
	return &test_msgs.TimeArrays{
		Ts: []builtin_interfaces.Time{{Sec: 1, Nanosec: 2}, {Sec: 3, Nanosec: 4}},
		Ds: []builtin_interfaces.Duration{{Sec: 5, Nanosec: 6}},
		Tf: [2]builtin_interfaces.Time{{Sec: 7, Nanosec: 8}, {Sec: 9, Nanosec: 10}},
		Df: [2]builtin_interfaces.Duration{{Sec: 11, Nanosec: 12}, {Sec: 13, Nanosec: 14}},
	}
}

// A sequence carries a 4-byte length. A fixed array carries none.
func TestTimeArrayWireImage(t *testing.T) {
	got, err := sample().SerializeCDR()
	if err != nil {
		t.Fatalf("SerializeCDR failed: %v", err)
	}
	words := []uint32{2, 1, 2, 3, 4, 1, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14}
	want := []byte{0x00, 0x01, 0x00, 0x00}
	for _, w := range words {
		want = append(want, byte(w), byte(w>>8), byte(w>>16), byte(w>>24))
	}
	if string(got) != string(want) {
		t.Fatalf("wire image mismatch.\n  got:  % x\n  want: % x", got, want)
	}
}

func TestTimeArrayRoundtrip(t *testing.T) {
	raw, err := sample().SerializeCDR()
	if err != nil {
		t.Fatalf("SerializeCDR failed: %v", err)
	}
	var d test_msgs.TimeArrays
	if err := d.DeserializeCDR(raw); err != nil {
		t.Fatalf("DeserializeCDR failed: %v", err)
	}
	s := sample()
	if len(d.Ts) != 2 || d.Ts[0] != s.Ts[0] || d.Ts[1] != s.Ts[1] {
		t.Errorf("Ts = %v, want %v", d.Ts, s.Ts)
	}
	if len(d.Ds) != 1 || d.Ds[0] != s.Ds[0] {
		t.Errorf("Ds = %v, want %v", d.Ds, s.Ds)
	}
	if d.Tf != s.Tf {
		t.Errorf("Tf = %v, want %v", d.Tf, s.Tf)
	}
	if d.Df != s.Df {
		t.Errorf("Df = %v, want %v", d.Df, s.Df)
	}
}

func allocated(f func()) uint64 {
	var a, b runtime.MemStats
	runtime.ReadMemStats(&a)
	f()
	runtime.ReadMemStats(&b)
	return b.TotalAlloc - a.TotalAlloc
}

func rejects(t *testing.T, decode func([]byte) error) {
	t.Helper()
	var err error
	n := allocated(func() { err = decode(hostileLen) })
	if err == nil {
		t.Errorf("hostile length decoded without an error")
	}
	if n > 1<<20 {
		t.Errorf("hostile length reserved %d bytes before it failed", n)
	}
}

func TestHostileTimeSequence(t *testing.T) {
	var m test_msgs.TimeSeq
	rejects(t, m.DeserializeCDR)
}

func TestHostileDurationSequence(t *testing.T) {
	var m test_msgs.DurationSeq
	rejects(t, m.DeserializeCDR)
}
`

func TestTimeDurationArraysCompileAndRoundtrip(t *testing.T) {
	// A skip is not a pass: the reason names the missing tool.
	goBin, err := exec.LookPath("go")
	if err != nil {
		t.Skipf("go binary not found on PATH; skipping time array test: %v", err)
	}

	field := func(name, kind, arrayKind string, size uint) FieldDefinition {
		f := FieldDefinition{Name: name, FieldType: FieldType{Kind: kind}, IsArray: arrayKind != "", ArrayKind: arrayKind}
		if arrayKind == "fixed" {
			f.ArraySize = uintPtr(size)
		}
		return f
	}
	msgs := []MessageDefinition{
		{Package: "builtin_interfaces", Name: "Time", FullName: "builtin_interfaces/Time", Fields: []FieldDefinition{
			field("sec", "Int32", "", 0), field("nanosec", "UInt32", "", 0),
		}},
		{Package: "builtin_interfaces", Name: "Duration", FullName: "builtin_interfaces/Duration", Fields: []FieldDefinition{
			field("sec", "Int32", "", 0), field("nanosec", "UInt32", "", 0),
		}},
		{Package: "test_msgs", Name: "TimeArrays", FullName: "test_msgs/TimeArrays", Fields: []FieldDefinition{
			field("ts", "Time", "unbounded", 0),
			field("ds", "Duration", "unbounded", 0),
			field("tf", "Time", "fixed", 2),
			field("df", "Duration", "fixed", 2),
		}},
		{Package: "test_msgs", Name: "TimeSeq", FullName: "test_msgs/TimeSeq", Fields: []FieldDefinition{
			field("v", "Time", "unbounded", 0),
		}},
		{Package: "test_msgs", Name: "FixedOnly", FullName: "test_msgs/FixedOnly", Fields: []FieldDefinition{
			field("tf", "Time", "fixed", 2),
		}},
		{Package: "test_msgs", Name: "DurationSeq", FullName: "test_msgs/DurationSeq", Fields: []FieldDefinition{
			field("v", "Duration", "unbounded", 0),
		}},
	}

	tmp := t.TempDir()
	if err := os.WriteFile(filepath.Join(tmp, "go.mod"), []byte("module timeprobe\n\ngo 1.23\n"), 0o644); err != nil {
		t.Fatalf("write go.mod: %v", err)
	}
	for _, m := range msgs {
		code, err := GenerateGoMessage(m, "timeprobe")
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
	if err := os.WriteFile(filepath.Join(tmp, "probe_test.go"), []byte(timeArrayProbeSource), 0o644); err != nil {
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
	for _, name := range []string{"TestTimeArrayWireImage", "TestTimeArrayRoundtrip", "TestHostileTimeSequence", "TestHostileDurationSequence"} {
		if !strings.Contains(out, "--- PASS: "+name) {
			t.Errorf("probe printed no PASS line for %s:\n%s", name, out)
		}
	}
}
