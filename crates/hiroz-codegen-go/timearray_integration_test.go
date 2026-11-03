package main

// This file compiles and runs the generated Go for messages, services and
// actions whose fields are sequences and fixed arrays of
// builtin_interfaces/Time and builtin_interfaces/Duration. The tests in
// main_test.go read generated text; this one builds it with the Go compiler,
// so a field type or an import that does not compile fails here.

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

const timeArrayProbeSource = `package main_test

import (
	"reflect"
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

func padded() *test_msgs.Padded {
	return &test_msgs.Padded{
		A:  0xab,
		Tf: [2]builtin_interfaces.Time{{Sec: 1, Nanosec: 2}, {Sec: 3, Nanosec: 4}},
		Z:  0xcd,
		Df: [1]builtin_interfaces.Duration{{Sec: 5, Nanosec: 6}},
	}
}

func le(words ...uint32) []byte {
	var b []byte
	for _, w := range words {
		b = append(b, byte(w), byte(w>>8), byte(w>>16), byte(w>>24))
	}
	return b
}

// A sequence carries a 4-byte length. A fixed array carries none.
func TestTimeArrayWireImage(t *testing.T) {
	got, err := sample().SerializeCDR()
	if err != nil {
		t.Fatalf("SerializeCDR failed: %v", err)
	}
	want := append([]byte{0x00, 0x01, 0x00, 0x00}, le(2, 1, 2, 3, 4, 1, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14)...)
	if string(got) != string(want) {
		t.Fatalf("wire image mismatch.\n  got:  % x\n  want: % x", got, want)
	}
}

// A uint8 before a fixed Time array needs 3 padding bytes. A uint8 after it
// needs 3 more before the next Duration. The decoder must skip both.
func TestPaddedFixedTimeArrayWireImage(t *testing.T) {
	got, err := padded().SerializeCDR()
	if err != nil {
		t.Fatalf("SerializeCDR failed: %v", err)
	}
	want := []byte{0x00, 0x01, 0x00, 0x00}
	want = append(want, 0xab, 0x00, 0x00, 0x00)
	want = append(want, le(1, 2, 3, 4)...)
	want = append(want, 0xcd, 0x00, 0x00, 0x00)
	want = append(want, le(5, 6)...)
	if string(got) != string(want) {
		t.Fatalf("wire image mismatch.\n  got:  % x\n  want: % x", got, want)
	}
}

func TestPaddedFixedTimeArrayRoundtrip(t *testing.T) {
	raw, err := padded().SerializeCDR()
	if err != nil {
		t.Fatalf("SerializeCDR failed: %v", err)
	}
	var d test_msgs.Padded
	if err := d.DeserializeCDR(raw); err != nil {
		t.Fatalf("DeserializeCDR failed: %v", err)
	}
	if want := padded(); !reflect.DeepEqual(&d, want) {
		t.Errorf("decoded = %+v, want %+v", &d, want)
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
	if want := sample(); !reflect.DeepEqual(&d, want) {
		t.Errorf("decoded = %+v, want %+v", &d, want)
	}
}

// allocated returns the bytes allocated by the whole process while f runs.
// TotalAlloc is process-wide, so the figure includes any other goroutine.
func allocated(f func()) uint64 {
	var a, b runtime.MemStats
	runtime.ReadMemStats(&a)
	f()
	runtime.ReadMemStats(&b)
	return b.TotalAlloc - a.TotalAlloc
}

// A hostile length of 0xffffffff must fail without a huge up-front reservation.
// The 1 MiB bound is far above background allocation and far below the
// reservation that an unchecked length would make.
func rejects(t *testing.T, decode func([]byte) error) {
	t.Helper()
	var err error
	n := allocated(func() { err = decode(hostileLen) })
	if err == nil {
		t.Errorf("hostile length decoded without an error")
	}
	if n > 1<<20 {
		t.Errorf("%d bytes were allocated during the call, above the 1 MiB bound", n)
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

// hirozStub stands in for the hiroz package that generated services and
// actions import. They use only the Message type.
const hirozStub = `package hiroz

type Message interface {
	TypeName() string
	TypeHash() string
	SerializeCDR() ([]byte, error)
	DeserializeCDR([]byte) error
}
`

func probeField(name, kind, arrayKind string, size uint) FieldDefinition {
	f := FieldDefinition{Name: name, FieldType: FieldType{Kind: kind}, IsArray: arrayKind != "", ArrayKind: arrayKind}
	if arrayKind == "fixed" {
		f.ArraySize = uintPtr(size)
	}
	return f
}

func builtinTimeMessages() []MessageDefinition {
	return []MessageDefinition{
		{Package: "builtin_interfaces", Name: "Time", FullName: "builtin_interfaces/Time", Fields: []FieldDefinition{
			probeField("sec", "Int32", "", 0), probeField("nanosec", "UInt32", "", 0),
		}},
		{Package: "builtin_interfaces", Name: "Duration", FullName: "builtin_interfaces/Duration", Fields: []FieldDefinition{
			probeField("sec", "Int32", "", 0), probeField("nanosec", "UInt32", "", 0),
		}},
	}
}

// probeModule is a temporary Go module that holds generated code.
type probeModule struct {
	t     *testing.T
	goBin string
	dir   string
}

func newProbeModule(t *testing.T) *probeModule {
	t.Helper()
	// A skip is not a pass: the reason names the missing tool.
	goBin, err := exec.LookPath("go")
	if err != nil {
		t.Skipf("go binary not found on PATH; skipping generated code test: %v", err)
	}
	m := &probeModule{t: t, goBin: goBin, dir: t.TempDir()}
	m.write("go.mod", "module timeprobe\n\ngo 1.23\n")
	m.write("hiroz/hiroz.go", hirozStub)
	return m
}

func (m *probeModule) write(rel string, content string) {
	m.t.Helper()
	path := filepath.Join(m.dir, rel)
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		m.t.Fatalf("mkdir for %s: %v", rel, err)
	}
	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		m.t.Fatalf("write %s: %v", rel, err)
	}
}

func (m *probeModule) message(msg MessageDefinition) {
	m.t.Helper()
	code, err := GenerateGoMessage(msg, "timeprobe")
	if err != nil {
		m.t.Fatalf("GenerateGoMessage(%s) failed: %v", msg.Name, err)
	}
	m.write(filepath.Join("generated", msg.Package, strings.ToLower(msg.Name)+".go"), string(code))
}

func (m *probeModule) service(srv ServiceDefinition) {
	m.t.Helper()
	code, err := GenerateGoService(srv, "timeprobe")
	if err != nil {
		m.t.Fatalf("GenerateGoService(%s) failed: %v", srv.Name, err)
	}
	m.write(filepath.Join("generated", srv.Package, "srv_"+strings.ToLower(srv.Name)+".go"), string(code))
}

func (m *probeModule) action(action ActionDefinition) {
	m.t.Helper()
	code, err := GenerateGoAction(action, "timeprobe")
	if err != nil {
		m.t.Fatalf("GenerateGoAction(%s) failed: %v", action.Name, err)
	}
	m.write(filepath.Join("generated", action.Package, "action_"+strings.ToLower(action.Name)+".go"), string(code))
}

// run fails the test when the go command fails. "go vet" and "go test"
// compile every package, so no separate "go build" is needed.
func (m *probeModule) run(args ...string) string {
	m.t.Helper()
	cmd := exec.Command(m.goBin, args...)
	cmd.Dir = m.dir
	cmd.Env = append(os.Environ(),
		"GOFLAGS=-mod=mod",
		"GOMODCACHE="+filepath.Join(m.dir, ".gomodcache"),
		"GOCACHE="+filepath.Join(m.dir, ".gocache"),
	)
	out, err := cmd.CombinedOutput()
	if err != nil {
		m.t.Fatalf("go %s failed: %v\n----- combined output -----\n%s", strings.Join(args, " "), err, strings.TrimSpace(string(out)))
	}
	return string(out)
}

func TestTimeDurationArraysCompileAndRoundtrip(t *testing.T) {
	m := newProbeModule(t)
	msgs := append(builtinTimeMessages(),
		MessageDefinition{Package: "test_msgs", Name: "TimeArrays", FullName: "test_msgs/TimeArrays", Fields: []FieldDefinition{
			probeField("ts", "Time", "unbounded", 0),
			probeField("ds", "Duration", "unbounded", 0),
			probeField("tf", "Time", "fixed", 2),
			probeField("df", "Duration", "fixed", 2),
		}},
		MessageDefinition{Package: "test_msgs", Name: "TimeSeq", FullName: "test_msgs/TimeSeq", Fields: []FieldDefinition{
			probeField("v", "Time", "unbounded", 0),
		}},
		MessageDefinition{Package: "test_msgs", Name: "FixedOnly", FullName: "test_msgs/FixedOnly", Fields: []FieldDefinition{
			probeField("tf", "Time", "fixed", 2),
		}},
		MessageDefinition{Package: "test_msgs", Name: "DurationSeq", FullName: "test_msgs/DurationSeq", Fields: []FieldDefinition{
			probeField("v", "Duration", "unbounded", 0),
		}},
		// The uint8 fields force padding before and after the fixed arrays.
		MessageDefinition{Package: "test_msgs", Name: "Padded", FullName: "test_msgs/Padded", Fields: []FieldDefinition{
			probeField("a", "UInt8", "", 0),
			probeField("tf", "Time", "fixed", 2),
			probeField("z", "UInt8", "", 0),
			probeField("df", "Duration", "fixed", 1),
		}},
	)
	for _, msg := range msgs {
		m.message(msg)
	}
	m.write("probe_test.go", timeArrayProbeSource)

	m.run("vet", "./...")
	out := m.run("test", "-count=1", "-v", ".")
	for _, name := range []string{
		"TestTimeArrayWireImage", "TestTimeArrayRoundtrip",
		"TestPaddedFixedTimeArrayWireImage", "TestPaddedFixedTimeArrayRoundtrip",
		"TestHostileTimeSequence", "TestHostileDurationSequence",
	} {
		if !strings.Contains(out, "--- PASS: "+name) {
			t.Errorf("probe printed no PASS line for %s:\n%s", name, out)
		}
	}
}

// The service and action generators each decide on their own whether to import
// encoding/binary. A fixed Time or Duration array needs no binary import, so a
// service or action with only such fields must not import it.
func TestTimeDurationArraysInServiceCompile(t *testing.T) {
	m := newProbeModule(t)
	for _, msg := range builtinTimeMessages() {
		m.message(msg)
	}
	m.service(ServiceDefinition{
		Package: "test_msgs", Name: "TimeSrv", FullName: "test_msgs/srv/TimeSrv",
		Request: MessageDefinition{Package: "test_msgs", Name: "TimeSrv_Request", FullName: "test_msgs/srv/TimeSrv_Request", Fields: []FieldDefinition{
			probeField("tf", "Time", "fixed", 2),
		}},
		Response: MessageDefinition{Package: "test_msgs", Name: "TimeSrv_Response", FullName: "test_msgs/srv/TimeSrv_Response", Fields: []FieldDefinition{
			probeField("df", "Duration", "fixed", 2),
		}},
	})
	m.run("vet", "./...")
}

func TestTimeDurationArraysInActionCompile(t *testing.T) {
	m := newProbeModule(t)
	for _, msg := range builtinTimeMessages() {
		m.message(msg)
	}
	result := MessageDefinition{Package: "test_msgs", Name: "TimeAct_Result", FullName: "test_msgs/action/TimeAct_Result", Fields: []FieldDefinition{
		probeField("df", "Duration", "fixed", 2),
	}}
	feedback := MessageDefinition{Package: "test_msgs", Name: "TimeAct_Feedback", FullName: "test_msgs/action/TimeAct_Feedback", Fields: []FieldDefinition{
		probeField("tf", "Time", "fixed", 1),
	}}
	m.action(ActionDefinition{
		Package: "test_msgs", Name: "TimeAct", FullName: "test_msgs/action/TimeAct",
		Goal: MessageDefinition{Package: "test_msgs", Name: "TimeAct_Goal", FullName: "test_msgs/action/TimeAct_Goal", Fields: []FieldDefinition{
			probeField("tf", "Time", "fixed", 2),
		}},
		Result:   &result,
		Feedback: &feedback,
	})
	m.run("vet", "./...")
}
