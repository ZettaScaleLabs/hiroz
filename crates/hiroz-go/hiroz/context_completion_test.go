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
	select {
	case <-b.entered:
	case <-time.After(barrierBound):
		t.Fatalf("%s was not reached", what)
	}
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

// A callback that is inside the user handler when the context ends keeps running. The
// subscriber is destroyed and the handler has not returned. The caller learns of completion
// from its own signal, which is what the documentation tells callers to use.
func TestBuildWithTypedCallbackContextCancelDoesNotWaitForHandler(t *testing.T) {
	f := stubSubscriber(t)
	b := newBarrier()
	defer b.free()
	var finished atomic.Bool
	exited := make(chan struct{})
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	_, err := BuildWithTypedCallbackContext(ctx, (&Node{}).CreateSubscriber("chatter"), func(*nilSafeMessage) {
		defer close(exited)
		b.hold()
		finished.Store(true)
	})
	if err != nil {
		t.Fatalf("build failed: %v", err)
	}

	delivered := make(chan struct{})
	go func() { f.deliver([]byte{1}); close(delivered) }()
	b.waitEntered(t, "the handler")

	cancel()
	receiveBounded(t, f.destroyed, "the destroy after cancel")
	if finished.Load() {
		t.Fatal("the handler finished before it was released")
	}
	select {
	case <-exited:
		t.Fatal("cancel waited for the handler: the documented contract says it does not")
	default:
	}

	b.free()
	receiveBounded(t, exited, "the handler to return after its release")
	receiveBounded(t, delivered, "the delivery to return")
	if !finished.Load() {
		t.Error("the handler did not run to its end after the context ended")
	}
}

// gateMessage decodes through a gate that a test can close. The generic helpers build a new
// value of the type for each sample, so the gate is shared through a package variable.
type gateMessage struct{}

var decodeGate atomic.Pointer[barrier]

func (*gateMessage) TypeName() string              { return "test/Gate" }
func (*gateMessage) TypeHash() string              { return "RIHS01_gate" }
func (*gateMessage) SerializeCDR() ([]byte, error) { return []byte{1}, nil }
func (*gateMessage) DeserializeCDR([]byte) error {
	if b := decodeGate.Load(); b != nil {
		b.hold()
	}
	return nil
}

// The channel closes when the teardown has waited for the callbacks that were sending. A
// callback still inside DeserializeCDR is not one of them: the channel closes, the sample
// is dropped, and the user decoder runs on until it returns.
func TestSubscriberWithChannelContextCloseDoesNotWaitForDecoder(t *testing.T) {
	f := stubSubscriber(t)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	_, ch, err := SubscriberWithChannelContext[*gateMessage](ctx, (&Node{}).CreateSubscriber("chatter"), 4)
	if err != nil {
		t.Fatalf("build failed: %v", err)
	}

	// Positive control: with the gate open a sample arrives.
	f.deliver([]byte{1})
	receiveBounded(t, ch, "the control sample")

	b := newBarrier()
	defer b.free()
	decodeGate.Store(b)
	t.Cleanup(func() { decodeGate.Store(nil) })
	delivered := make(chan struct{})
	go func() { f.deliver([]byte{1}); close(delivered) }()
	b.waitEntered(t, "the decoder")

	cancel()
	closed := make(chan int)
	go func() {
		n := 0
		for range ch {
			n++
		}
		closed <- n
	}()
	if n := receiveBounded(t, closed, "the channel to close"); n != 0 {
		t.Errorf("the channel delivered %d samples after cancel, want 0", n)
	}
	select {
	case <-delivered:
		t.Fatal("the decoder returned before it was released")
	default:
	}

	b.free()
	receiveBounded(t, delivered, "the delivery to return after the release")
	if _, ok := <-ch; ok {
		t.Error("the released decoder delivered a sample into a closed channel")
	}
}

// gateRequest holds SerializeCDR at a barrier.
type gateRequest struct {
	MockMessage
	b *barrier
}

func (m *gateRequest) SerializeCDR() ([]byte, error) { m.b.hold(); return []byte{1}, nil }

// gateResponse holds DeserializeCDR at a barrier.
type gateResponse struct {
	MockMessage
	b *barrier
}

func (m *gateResponse) DeserializeCDR(data []byte) error {
	m.b.hold()
	return m.MockMessage.DeserializeCDR(data)
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
	go func() { done <- CallTypedContext(ctx, openTestClient(), &gateRequest{b: b}, &MockMessage{}) }()
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
	resp := &gateResponse{b: b}

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
	if len(resp.data) != 1 || resp.data[0] != 7 {
		t.Errorf("response holds %v, want [7]", resp.data)
	}
}
