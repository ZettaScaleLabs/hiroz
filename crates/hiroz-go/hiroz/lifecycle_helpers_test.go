package hiroz

import (
	"runtime"
	"sync"
	"testing"
	"time"
)

// waitUntil yields until cond holds. It fails the test after a generous deadline, which a
// passing run never reaches.
func waitUntil(t *testing.T, what string, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(10 * time.Second)
	for !cond() {
		if time.Now().After(deadline) {
			t.Fatalf("timed out waiting for %s", what)
		}
		runtime.Gosched()
	}
}

// newRelease returns a channel that a stub waits on and the function that releases it. The
// function is idempotent and also runs at the end of the test, so a failed assertion cannot
// leave the stub goroutine blocked.
func newRelease(t *testing.T) (<-chan struct{}, func()) {
	t.Helper()
	ch := make(chan struct{})
	var once sync.Once
	free := func() { once.Do(func() { close(ch) }) }
	t.Cleanup(free)
	return ch, free
}

// stubMsg is a message whose serializer and decoder a test supplies. A nil function does the
// default: serialise to [1] and decode nothing.
type stubMsg struct {
	ser func() ([]byte, error)
	de  func([]byte) error
}

func (*stubMsg) TypeName() string { return "x" }
func (*stubMsg) TypeHash() string { return "y" }
func (m *stubMsg) SerializeCDR() ([]byte, error) {
	if m.ser == nil {
		return []byte{1}, nil
	}
	return m.ser()
}
func (m *stubMsg) DeserializeCDR(b []byte) error {
	if m.de == nil {
		return nil
	}
	return m.de(b)
}

// hookMsg runs hook inside SerializeCDR, as user code does.
func hookMsg(hook func()) *stubMsg {
	return &stubMsg{ser: func() ([]byte, error) { hook(); return []byte{1}, nil }}
}

// failMsg runs hook inside SerializeCDR and then fails with errSerializerFailed.
func failMsg(hook func()) *stubMsg {
	return &stubMsg{ser: func() ([]byte, error) { hook(); return nil, errSerializerFailed }}
}

// sharedMsg returns its own backing array from SerializeCDR, as a message that caches its
// buffer does. The Message interface does not forbid this.
func sharedMsg(data []byte) *stubMsg {
	return &stubMsg{ser: func() ([]byte, error) { return data, nil }}
}
