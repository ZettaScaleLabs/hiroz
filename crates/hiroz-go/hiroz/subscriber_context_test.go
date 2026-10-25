package hiroz

import (
	"context"
	"errors"
	"runtime/cgo"
	"sync/atomic"
	"testing"
	"time"
	"unsafe"
)

func TestSubscriberWithChannelContextAlreadyCancelled(t *testing.T) {
	before := liveClosures.Load()
	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	sub, ch, err := SubscriberWithChannelContext[*MockTypedMessage](ctx, (&Node{}).CreateSubscriber("chatter"), 4)
	if sub != nil || ch != nil {
		t.Errorf("want no subscriber and no channel, got sub=%v ch=%v", sub, ch)
	}
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("want context.Canceled, got %v", err)
	}
	if leaked := liveClosures.Load() - before; leaked != 0 {
		t.Errorf("a cancelled context left %d callback handles alive, want 0", leaked)
	}
}

func TestSubscriberWithChannelContextBuildFailure(t *testing.T) {
	// The node has no session, so the C creator returns null.
	sub, ch, err := SubscriberWithChannelContext[*MockTypedMessage](context.Background(), (&Node{}).CreateSubscriber("chatter"), 4)
	if sub != nil || ch != nil {
		t.Errorf("want no subscriber and no channel, got sub=%v ch=%v", sub, ch)
	}
	if !errors.Is(err, ErrBuildFailed) {
		t.Fatalf("want ErrBuildFailed, got %v", err)
	}
}

func TestBuildWithTypedCallbackContextAlreadyCancelled(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	sub, err := BuildWithTypedCallbackContext(ctx, (&Node{}).CreateSubscriber("chatter"), func(*MockTypedMessage) {})
	if sub != nil {
		t.Error("a cancelled context must not return a subscriber")
	}
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("want context.Canceled, got %v", err)
	}
}

func TestBuildWithTypedCallbackContextBuildFailure(t *testing.T) {
	sub, err := BuildWithTypedCallbackContext(context.Background(), (&Node{}).CreateSubscriber("chatter"), func(*MockTypedMessage) {})
	if sub != nil || !errors.Is(err, ErrBuildFailed) {
		t.Fatalf("want ErrBuildFailed and no subscriber, got sub=%v err=%v", sub, err)
	}
}

// nilSafeMessage decodes nothing, so a nil receiver is safe. The generic subscriber helpers
// decode into a zero value of T, which is a nil pointer for a pointer type.
type nilSafeMessage struct{}

func (*nilSafeMessage) TypeName() string              { return "test/NilSafe" }
func (*nilSafeMessage) TypeHash() string              { return "RIHS01_nil_safe" }
func (*nilSafeMessage) SerializeCDR() ([]byte, error) { return []byte{1}, nil }
func (*nilSafeMessage) DeserializeCDR([]byte) error   { return nil }

// fakeSubscriber replaces the two C calls of a subscriber and lets a test deliver a message.
type fakeSubscriber struct {
	deliver   func([]byte)
	destroyed chan struct{}
	destroys  atomic.Int32
}

func stubSubscriber(t *testing.T) *fakeSubscriber {
	t.Helper()
	f := &fakeSubscriber{destroyed: make(chan struct{}, 16)}
	oldCreate, oldDestroy := createSubscriber, destroySubscriber
	createSubscriber = func(c subscriberCall) *cSubscriber {
		h := cgo.Handle(c.userData)
		f.deliver = func(b []byte) { h.Value().(*closureContext[[]byte]).call(b) }
		return (*cSubscriber)(unsafe.Pointer(new([4096]byte)))
	}
	destroySubscriber = func(*cSubscriber) int32 {
		f.destroys.Add(1)
		f.destroyed <- struct{}{}
		return 0
	}
	t.Cleanup(func() { createSubscriber, destroySubscriber = oldCreate, oldDestroy })
	return f
}

// Positive control and cancel: messages arrive while ctx lives. When ctx ends, the subscriber
// is destroyed once and the channel closes.
func TestSubscriberWithChannelContextCancelDestroysAndCloses(t *testing.T) {
	f := stubSubscriber(t)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	sub, ch, err := SubscriberWithChannelContext[*nilSafeMessage](ctx, (&Node{}).CreateSubscriber("chatter"), 4)
	if err != nil {
		t.Fatalf("build failed: %v", err)
	}

	f.deliver([]byte{1})
	if _, ok := <-ch; !ok {
		t.Fatal("the channel closed before the context ended")
	}
	if n := f.destroys.Load(); n != 0 {
		t.Fatalf("destroys = %d before cancel, want 0", n)
	}

	cancel()
	finished := make(chan struct{})
	go func() {
		for range ch {
		}
		close(finished)
	}()
	select {
	case <-finished:
	case <-time.After(10 * time.Second):
		t.Fatal("the channel did not close after cancel")
	}
	if n := f.destroys.Load(); n != 1 {
		t.Errorf("destroys = %d after cancel, want 1", n)
	}
	if err := sub.Close(); err != nil {
		t.Errorf("Close after cancel: %v", err)
	}
	if n := f.destroys.Load(); n != 1 {
		t.Errorf("destroys = %d after a second Close, want 1", n)
	}
}

// A callback that arrives after the channel closed must neither panic nor deliver.
// Before the teardown waited for senders, the closed channel and the finished done channel
// were both ready and the select picked either at random.
func TestSubscriberWithChannelContextLateCallbackIsSafe(t *testing.T) {
	f := stubSubscriber(t)
	ctx, cancel := context.WithCancel(context.Background())
	_, ch, err := SubscriberWithChannelContext[*nilSafeMessage](ctx, (&Node{}).CreateSubscriber("chatter"), 4)
	if err != nil {
		t.Fatalf("build failed: %v", err)
	}
	cancel()
	for range ch {
	}
	for i := 0; i < 1000; i++ {
		f.deliver([]byte{1})
	}
	if _, ok := <-ch; ok {
		t.Error("a late callback delivered a message")
	}
}

// A callback that waits on a full channel returns when the context ends.
func TestSubscriberWithChannelContextCancelReleasesBlockedCallback(t *testing.T) {
	f := stubSubscriber(t)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	_, ch, err := SubscriberWithChannelContext[*nilSafeMessage](ctx, (&Node{}).CreateSubscriber("chatter"), 1)
	if err != nil {
		t.Fatalf("build failed: %v", err)
	}
	f.deliver([]byte{1}) // fills the buffer

	returned := make(chan struct{})
	go func() { f.deliver([]byte{1}); close(returned) }()
	cancel()
	select {
	case <-returned:
	case <-time.After(10 * time.Second):
		t.Fatal("the callback stayed blocked after cancel")
	}
	for range ch {
	}
}

// Closing the subscriber first is safe: the later cancel destroys nothing twice.
func TestSubscriberWithChannelContextEarlyCloseThenCancel(t *testing.T) {
	f := stubSubscriber(t)
	ctx, cancel := context.WithCancel(context.Background())
	sub, ch, err := SubscriberWithChannelContext[*nilSafeMessage](ctx, (&Node{}).CreateSubscriber("chatter"), 1)
	if err != nil {
		t.Fatalf("build failed: %v", err)
	}
	if err := sub.Close(); err != nil {
		t.Fatalf("early Close: %v", err)
	}
	cancel()
	for range ch {
	}
	if n := f.destroys.Load(); n != 1 {
		t.Errorf("destroys = %d, want 1", n)
	}
}

// Positive control and cancel for the callback variant.
func TestBuildWithTypedCallbackContextCancelDestroys(t *testing.T) {
	f := stubSubscriber(t)
	node := &Node{}
	got := make(chan struct{}, 4)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	sub, err := BuildWithTypedCallbackContext(ctx, node.CreateSubscriber("chatter"), func(*nilSafeMessage) { got <- struct{}{} })
	if err != nil {
		t.Fatalf("build failed: %v", err)
	}
	f.deliver([]byte{1})
	select {
	case <-got:
	default:
		t.Fatal("the callback did not run while the context lived")
	}
	if n := f.destroys.Load(); n != 0 {
		t.Fatalf("destroys = %d before cancel, want 0", n)
	}

	cancel()
	<-f.destroyed
	if n := f.destroys.Load(); n != 1 {
		t.Errorf("destroys = %d after cancel, want 1", n)
	}
	if err := sub.Close(); err != nil {
		t.Errorf("Close after cancel: %v", err)
	}
	if n := f.destroys.Load(); n != 1 {
		t.Errorf("destroys = %d after a second Close, want 1", n)
	}
}

func TestBuildWithTypedCallbackContextEarlyCloseThenCancel(t *testing.T) {
	f := stubSubscriber(t)
	node := &Node{}
	ctx, cancel := context.WithCancel(context.Background())
	sub, err := BuildWithTypedCallbackContext(ctx, node.CreateSubscriber("chatter"), func(*nilSafeMessage) {})
	if err != nil {
		t.Fatalf("build failed: %v", err)
	}
	if err := sub.Close(); err != nil {
		t.Fatalf("early Close: %v", err)
	}
	cancel()
	// The cancel path removes the subscriber from the node. Wait for that, then count.
	waitUntil(t, "the cancel path to run", func() bool {
		node.subsMu.Lock()
		defer node.subsMu.Unlock()
		return len(node.ownedSubs) == 0
	})
	if n := f.destroys.Load(); n != 1 {
		t.Errorf("destroys = %d, want 1", n)
	}
}
