package hiroz

import (
	"context"
	"errors"
	"runtime"
	"runtime/cgo"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
	"unsafe"
)

func TestSubscriberWithChannelContextAlreadyCancelled(t *testing.T) {
	// A cancelled context must not reach the C creator, so no callback state exists.
	var creates atomic.Int32
	original := createSubscriber
	createSubscriber = func(subscriberCall) *cSubscriber { creates.Add(1); return nil }
	t.Cleanup(func() { createSubscriber = original })
	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	sub, ch, err := SubscriberWithChannelContext[*MockTypedMessage](ctx, (&Node{}).CreateSubscriber("chatter"), 4)
	if sub != nil || ch != nil {
		t.Errorf("want no subscriber and no channel, got sub=%v ch=%v", sub, ch)
	}
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("want context.Canceled, got %v", err)
	}
	if n := creates.Load(); n != 0 {
		t.Errorf("a cancelled context reached the C creator %d times, want 0", n)
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

// waitCallbackBlocked waits until a callback of SubscriberWithChannelContext is parked in
// its send, so a test cancels only when a callback waits on the full channel.
func waitCallbackBlocked(t *testing.T) {
	t.Helper()
	waitUntil(t, "a callback to block on the full channel", func() bool {
		buf := make([]byte, 1<<20)
		buf = buf[:runtime.Stack(buf, true)]
		for _, g := range strings.Split(string(buf), "\n\n") {
			if strings.Contains(g, "SubscriberWithChannelContext") &&
				(strings.Contains(g, "[select") || strings.Contains(g, "[chan send")) {
				return true
			}
		}
		return false
	})
}

// A callback that waits on a full channel returns when the context ends. The cancel comes
// only after the callback is parked in its send, so the release has to come from the cancel.
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
	waitCallbackBlocked(t)
	cancel()
	receiveBounded(t, returned, "the blocked callback to return after cancel")

	drained := make(chan struct{})
	go func() {
		for range ch {
		}
		close(drained)
	}()
	receiveBounded(t, drained, "the channel to close")
}

// A sample that is inside DeserializeCDR when ctx ends is dropped when the decoder returns,
// also when the decoder returns before the destroy has finished. The destroy is held, so the
// send has a free buffer slot and an ended context to choose between. The loop covers the
// random choice of select.
func TestSubscriberWithChannelContextDecoderReleasedAfterCancelDropsSample(t *testing.T) {
	f := stubSubscriber(t)
	t.Cleanup(func() { decodeGate.Store(nil) })
	for i := 0; i < 25; i++ {
		holdDestroy, freeDestroy := newRelease(t)
		entered := make(chan struct{}, 1)
		destroySubscriber = func(*cSubscriber) int32 {
			entered <- struct{}{}
			<-holdDestroy
			return 0
		}
		b := newBarrier()
		decodeGate.Store(b)
		ctx, cancel := context.WithCancel(context.Background())
		_, ch, err := SubscriberWithChannelContext[*gateMessage](ctx, (&Node{}).CreateSubscriber("chatter"), 4)
		if err != nil {
			t.Fatalf("build failed: %v", err)
		}

		delivered := make(chan struct{})
		go func() { f.deliver([]byte{1}); close(delivered) }()
		b.waitEntered(t, "the decoder")
		cancel()
		receiveBounded(t, entered, "the destroy to start")
		b.free()
		receiveBounded(t, delivered, "the callback to return")
		freeDestroy()

		got := 0
		closedCh := make(chan struct{})
		go func() {
			for range ch {
				got++
			}
			close(closedCh)
		}()
		receiveBounded(t, closedCh, "the channel to close")
		if got != 0 {
			t.Fatalf("iteration %d: a sample that was in the decoder at cancel arrived", i)
		}
	}
}

// A callback that starts before the build reports failure must not stay blocked on a channel
// that nobody can read.
func TestSubscriberWithChannelContextBuildFailureReleasesCallback(t *testing.T) {
	returned := make(chan struct{})
	original := createSubscriber
	createSubscriber = func(c subscriberCall) *cSubscriber {
		call := cgo.Handle(c.userData).Value().(*closureContext[[]byte]).call
		go func() { call([]byte{1}); close(returned) }()
		return nil
	}
	t.Cleanup(func() { createSubscriber = original })

	sub, ch, err := SubscriberWithChannelContext[*nilSafeMessage](context.Background(), (&Node{}).CreateSubscriber("chatter"), 0)
	if sub != nil || ch != nil || !errors.Is(err, ErrBuildFailed) {
		t.Fatalf("want ErrBuildFailed and no subscriber, got sub=%v ch=%v err=%v", sub, ch, err)
	}
	receiveBounded(t, returned, "the callback to return after the failed build")
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
	receiveBounded(t, f.destroyed, "the destroy after cancel")
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

// countingCtx is a context that counts how often an AfterFunc is registered on it and how
// often that registration is stopped. context.AfterFunc calls the AfterFunc method of a
// parent that has one. The own done channel keeps the standard library from registering
// the child on a wrapped parent instead.
type countingCtx struct {
	context.Context
	done       chan struct{}
	once       sync.Once
	mu         sync.Mutex
	entries    []*afterEntry
	registered atomic.Int32
	stopped    atomic.Int32
}

type afterEntry struct {
	f       func()
	settled bool
}

func newCountingCtx() *countingCtx {
	return &countingCtx{Context: context.Background(), done: make(chan struct{})}
}

func (c *countingCtx) Done() <-chan struct{} { return c.done }

func (c *countingCtx) Err() error {
	select {
	case <-c.done:
		return context.Canceled
	default:
		return nil
	}
}

func (c *countingCtx) AfterFunc(f func()) func() bool {
	c.registered.Add(1)
	e := &afterEntry{f: f}
	c.mu.Lock()
	c.entries = append(c.entries, e)
	c.mu.Unlock()
	return func() bool {
		c.stopped.Add(1)
		c.mu.Lock()
		defer c.mu.Unlock()
		if e.settled {
			return false
		}
		e.settled = true
		return true
	}
}

func (c *countingCtx) cancel() {
	c.once.Do(func() {
		close(c.done)
		c.mu.Lock()
		defer c.mu.Unlock()
		for _, e := range c.entries {
			if !e.settled {
				e.settled = true
				go e.f()
			}
		}
	})
}

// A subscriber that closes early must not stay registered on a context that lives on. The
// registration holds the subscriber, its node and its closure until the context ends.
func TestBuildWithTypedCallbackContextEarlyCloseReleasesRegistration(t *testing.T) {
	stubSubscriber(t)
	node := &Node{}
	ctx := newCountingCtx()
	defer ctx.cancel()
	sub, err := BuildWithTypedCallbackContext(ctx, node.CreateSubscriber("chatter"), func(*nilSafeMessage) {})
	if err != nil {
		t.Fatalf("build failed: %v", err)
	}
	// Positive control: the build registered once, and nothing stopped it yet.
	if r, s := ctx.registered.Load(), ctx.stopped.Load(); r != 1 || s != 0 {
		t.Fatalf("after build: registered=%d stopped=%d, want 1 and 0", r, s)
	}
	if err := sub.Close(); err != nil {
		t.Fatalf("early Close: %v", err)
	}
	if s := ctx.stopped.Load(); s != 1 {
		t.Errorf("stopped = %d after an early Close, want 1: the context still holds the subscriber", s)
	}
	node.subsMu.Lock()
	owned := len(node.ownedSubs)
	node.subsMu.Unlock()
	if owned != 0 {
		t.Errorf("the node still owns %d subscribers after an early Close, want 0", owned)
	}
}

// An early Close of the channel variant releases the registration and closes the channel,
// because nothing else would close it before the context ends.
func TestSubscriberWithChannelContextEarlyCloseReleasesAndCloses(t *testing.T) {
	f := stubSubscriber(t)
	ctx := newCountingCtx()
	defer ctx.cancel()
	sub, ch, err := SubscriberWithChannelContext[*nilSafeMessage](ctx, (&Node{}).CreateSubscriber("chatter"), 1)
	if err != nil {
		t.Fatalf("build failed: %v", err)
	}
	if r, s := ctx.registered.Load(), ctx.stopped.Load(); r != 1 || s != 0 {
		t.Fatalf("after build: registered=%d stopped=%d, want 1 and 0", r, s)
	}
	if err := sub.Close(); err != nil {
		t.Fatalf("early Close: %v", err)
	}
	if s := ctx.stopped.Load(); s != 1 {
		t.Errorf("stopped = %d after an early Close, want 1: the context still holds the subscriber", s)
	}
	drained := make(chan struct{})
	go func() {
		for range ch {
		}
		close(drained)
	}()
	receiveBounded(t, drained, "the channel to close after an early Close")
	if n := f.destroys.Load(); n != 1 {
		t.Errorf("destroys = %d, want 1", n)
	}
}
