package hiroz

import (
	"context"
	"errors"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

// These tests pin what a context ending does and does not establish. Each one holds user
// code at a barrier while the context ends, so the order of events is fixed by the barrier
// and not by timing. Every wait is a bounded receive, and a failure is an assertion.

const barrierBound = 10 * time.Second

// blockedWindow is how long a test watches for a call that must stay blocked. A snapshot
// taken at once would pass for a call that returns a moment later.
const blockedWindow = 100 * time.Millisecond

// barrier holds user code until the test releases it. Release is idempotent, so a deferred
// release frees the held goroutine when an assertion fails first.
type barrier struct {
	entered chan struct{}
	release chan struct{}
	once    sync.Once
}

func newBarrier() *barrier {
	return &barrier{entered: make(chan struct{}, 4), release: make(chan struct{})}
}

// hold is called by user code. It reports entry, then waits for the release.
func (b *barrier) hold() {
	b.entered <- struct{}{}
	<-b.release
}

func (b *barrier) free() { b.once.Do(func() { close(b.release) }) }

func (b *barrier) waitEntered(t *testing.T, what string) {
	t.Helper()
	receiveBounded(t, b.entered, what)
}

func receiveBounded[V any](t *testing.T, ch <-chan V, what string) V {
	t.Helper()
	select {
	case v := <-ch:
		return v
	case <-time.After(barrierBound):
		t.Fatalf("timed out waiting for %s", what)
		panic("unreachable")
	}
}

// Cancelling while SerializeCDR runs does not end the call: the context bounds the wait for
// the C result, and serialization runs before it. The call returns the context error when
// the serializer returns, and it never reaches the C layer.
func TestCallTypedContextCancelDuringSerializeWaitsForSerializer(t *testing.T) {
	var calls atomic.Int32
	stubServiceCall(t, func([]byte, uint64) ([]byte, int32) { calls.Add(1); return []byte{7}, 0 })
	b := newBarrier()
	defer b.free()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	done := make(chan error, 1)
	go func() { done <- CallTypedContext(ctx, openTestClient(), hookMsg(b.hold), &MockMessage{}) }()
	b.waitEntered(t, "the serializer")
	cancel()
	select {
	case err := <-done:
		t.Fatalf("the call returned %v while the serializer was blocked", err)
	case <-time.After(blockedWindow):
	}

	b.free()
	if err := receiveBounded(t, done, "the call to return"); !errors.Is(err, context.Canceled) {
		t.Fatalf("want context.Canceled, got %v", err)
	}
	if n := calls.Load(); n != 0 {
		t.Errorf("the C layer was called %d times for a call cancelled in the serializer, want 0", n)
	}
}

// Cancelling while DeserializeCDR runs does not interrupt it. The context was live when the
// result arrived, so the call returns nil and the response holds the decoded value.
func TestCallTypedContextCancelDuringDeserializeStillReturnsResult(t *testing.T) {
	stubServiceCall(t, func([]byte, uint64) ([]byte, int32) { return []byte{7}, 0 })
	b := newBarrier()
	defer b.free()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	var decoded []byte
	resp := &stubMsg{de: func(data []byte) error { b.hold(); decoded = data; return nil }}

	done := make(chan error, 1)
	go func() { done <- CallTypedContext(ctx, openTestClient(), mockReq(), resp) }()
	b.waitEntered(t, "the deserializer")
	cancel()
	select {
	case err := <-done:
		t.Fatalf("the call returned %v while the deserializer was blocked", err)
	case <-time.After(blockedWindow):
	}

	b.free()
	if err := receiveBounded(t, done, "the call to return"); err != nil {
		t.Fatalf("want nil from a result that arrived before the cancel, got %v", err)
	}
	if len(decoded) != 1 || decoded[0] != 7 {
		t.Errorf("response holds %v, want [7]", decoded)
	}
}
