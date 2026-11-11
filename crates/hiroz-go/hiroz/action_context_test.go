package hiroz

import (
	"context"
	"errors"
	"runtime"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

func TestSendGoalContextAlreadyCancelled(t *testing.T) {
	c := &ActionClient{action: "/dummy"}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	h, err := c.SendGoalContext(ctx, &MockMessage{typeName: "x", typeHash: "y", data: []byte{1}})
	if h != nil {
		t.Error("a cancelled context must not return a goal handle")
	}
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("want context.Canceled, got %v", err)
	}
}

func TestSendGoalContextOnClosedClient(t *testing.T) {
	c := &ActionClient{action: "/dummy"}
	h, err := c.SendGoalContext(context.Background(), &MockMessage{typeName: "x", typeHash: "y", data: []byte{1}})
	if h != nil || err == nil {
		t.Fatalf("want an error and no handle, got h=%v err=%v", h, err)
	}
	if errors.Is(err, context.Canceled) || errors.Is(err, context.DeadlineExceeded) {
		t.Errorf("closed client must not read as a context error: %v", err)
	}
}

func TestGetTypedResultContextOnClosedHandle(t *testing.T) {
	h := &GoalHandle{}
	err := GetTypedResultContext(context.Background(), h, &MockMessage{})
	if err == nil {
		t.Fatal("expected error on closed goal handle")
	}
	if errors.Is(err, context.Canceled) || errors.Is(err, context.DeadlineExceeded) {
		t.Errorf("closed handle must not read as a context error: %v", err)
	}
}

func goalMsg() *MockMessage { return &MockMessage{typeName: "x", typeHash: "y", data: []byte{1}} }

func stubSendGoal(t *testing.T, f func(c *ActionClient, goalBytes []byte) (*GoalHandle, error)) {
	t.Helper()
	old := sendGoal
	sendGoal = f
	t.Cleanup(func() { sendGoal = old })
}

func openTestActionClient() *ActionClient {
	c := &ActionClient{action: "/dummy"}
	c.handle.Store(&cActionClientHandle{})
	return c
}

// A goal handle with a nil C pointer: Close takes the handle out and the C layer reports an error.
func fakeGoalHandle(c *ActionClient) *GoalHandle {
	h := &GoalHandle{client: c}
	h.handle.Store(&cGoalHandle{})
	return h
}

// Positive control: without a cancel the goal handle comes back and stays open.
func TestSendGoalContextDeliversHandle(t *testing.T) {
	c := openTestActionClient()
	want := fakeGoalHandle(c)
	stubSendGoal(t, func(*ActionClient, []byte) (*GoalHandle, error) { return want, nil })

	h, err := c.SendGoalContext(context.Background(), goalMsg())
	if err != nil || h != want {
		t.Fatalf("want the stub handle and no error, got h=%v err=%v", h, err)
	}
	if h.handle.Load() == nil {
		t.Error("the goal handle was closed although the context never ended")
	}
}

// A cancel while the C layer is busy returns ctx.Err() at once and closes the handle that
// arrives later. Close waits for the call.
func TestSendGoalContextCancelInFlightClosesHandle(t *testing.T) {
	c := openTestActionClient()
	late := fakeGoalHandle(c)
	started := make(chan struct{})
	release, free := newRelease(t)
	stubSendGoal(t, func(*ActionClient, []byte) (*GoalHandle, error) {
		close(started)
		<-release
		return late, nil
	})
	ctx, cancel := context.WithCancel(context.Background())

	type out struct {
		h   *GoalHandle
		err error
	}
	done := make(chan out, 1)
	go func() { h, err := c.SendGoalContext(ctx, goalMsg()); done <- out{h, err} }()
	receiveBounded(t, started, "the C layer to start")
	cancel()

	r := receiveBounded(t, done, "the call to return")
	if r.h != nil || !errors.Is(r.err, context.Canceled) {
		t.Fatalf("want context.Canceled and no handle, got h=%v err=%v", r.h, r.err)
	}
	if late.handle.Load() == nil {
		t.Fatal("the goal handle closed before the C layer returned")
	}

	closed := make(chan struct{})
	go func() { _ = c.Close(); close(closed) }()

	free()
	waitUntil(t, "the late goal handle to close", func() bool { return late.handle.Load() == nil })
	receiveBounded(t, closed, "Close to return")
}

// ownedGoal is a goal whose bytes the caller may change after a call returns. It counts the
// serialisations so a test can tell when SerializeCDR ran.
type ownedGoal struct {
	mu    sync.Mutex
	data  []byte
	calls int
}

func (g *ownedGoal) TypeName() string { return "x" }
func (g *ownedGoal) TypeHash() string { return "y" }
func (g *ownedGoal) SerializeCDR() ([]byte, error) {
	g.mu.Lock()
	defer g.mu.Unlock()
	g.calls++
	return append([]byte(nil), g.data...), nil
}
func (g *ownedGoal) DeserializeCDR([]byte) error { return nil }
func (g *ownedGoal) set(b []byte) {
	g.mu.Lock()
	g.data = b
	g.mu.Unlock()
}
func (g *ownedGoal) serialisations() int {
	g.mu.Lock()
	defer g.mu.Unlock()
	return g.calls
}

// SendGoalContext reads the goal before it returns. A caller that changes the goal after a
// cancelled call must not change the goal that the C layer sends later.
func TestSendGoalContextSerialisesBeforeReturn(t *testing.T) {
	c := openTestActionClient()
	goal := &ownedGoal{data: []byte{1}}
	started := make(chan struct{})
	release, free := newRelease(t)
	sent := make(chan []byte, 1)
	stubSendGoal(t, func(_ *ActionClient, b []byte) (*GoalHandle, error) {
		close(started)
		<-release
		sent <- b // the C layer reads the goal bytes here
		return nil, errors.New("stub")
	})
	ctx, cancel := context.WithCancel(context.Background())

	done := make(chan error, 1)
	go func() { _, err := c.SendGoalContext(ctx, goal); done <- err }()
	receiveBounded(t, started, "the C layer to start")
	cancel()
	if err := receiveBounded(t, done, "the call to return"); !errors.Is(err, context.Canceled) {
		t.Fatalf("want context.Canceled, got %v", err)
	}
	if n := goal.serialisations(); n != 1 {
		t.Errorf("the goal was serialised %d times before the call returned, want 1", n)
	}
	goal.set([]byte{9}) // the caller reuses the goal
	free()
	if got := receiveBounded(t, sent, "the C layer to send"); len(got) != 1 || got[0] != 1 {
		t.Errorf("the C layer sent %v, want the goal as it was at the call: [1]", got)
	}
	_ = c.Close()
}

// sharedGoal returns its own backing array from SerializeCDR, as a message that caches its
// buffer does. The Message interface does not forbid this.
type sharedGoal struct{ data []byte }

func (g *sharedGoal) TypeName() string              { return "x" }
func (g *sharedGoal) TypeHash() string              { return "y" }
func (g *sharedGoal) SerializeCDR() ([]byte, error) { return g.data, nil }
func (g *sharedGoal) DeserializeCDR([]byte) error   { return nil }

// A goal that returns shared storage must not change what the C layer sends after the call
// returned on cancel. SendGoalContext copies the bytes before the worker starts.
func TestSendGoalContextOwnsSerialisedBytes(t *testing.T) {
	c := openTestActionClient()
	goal := &sharedGoal{data: []byte{1, 2}}
	started := make(chan struct{})
	release, free := newRelease(t)
	sent := make(chan []byte, 1)
	stubSendGoal(t, func(_ *ActionClient, b []byte) (*GoalHandle, error) {
		close(started)
		<-release
		sent <- append([]byte(nil), b...) // the C layer reads the goal bytes here
		return nil, errors.New("stub")
	})
	ctx, cancel := context.WithCancel(context.Background())

	done := make(chan error, 1)
	go func() { _, err := c.SendGoalContext(ctx, goal); done <- err }()
	receiveBounded(t, started, "the C layer to start")
	cancel()
	if err := receiveBounded(t, done, "the call to return"); !errors.Is(err, context.Canceled) {
		t.Fatalf("want context.Canceled, got %v", err)
	}
	goal.data[0] = 9 // the caller reuses the goal storage in place
	free()
	if got := receiveBounded(t, sent, "the C layer to send"); len(got) != 2 || got[0] != 1 || got[1] != 2 {
		t.Errorf("the C layer sent %v, want the goal as it was at the call: [1 2]", got)
	}
	_ = c.Close()
}

// hookGoal runs hook inside SerializeCDR, as user code does.
type hookGoal struct{ hook func() }

func (g *hookGoal) TypeName() string { return "x" }
func (g *hookGoal) TypeHash() string { return "y" }
func (g *hookGoal) SerializeCDR() ([]byte, error) {
	g.hook()
	return []byte{1}, nil
}
func (g *hookGoal) DeserializeCDR([]byte) error { return nil }

// A panic in the caller's SerializeCDR propagates before the call registers, so it leaves no
// registration behind and Close does not wait.
func TestSendGoalContextPanicInSerializeLeavesNoRegistration(t *testing.T) {
	closeReturns := func(c *ActionClient) bool {
		closed := make(chan struct{})
		go func() { _ = c.Close(); close(closed) }()
		select {
		case <-closed:
			return true
		case <-time.After(300 * time.Millisecond):
			return false
		}
	}

	// Positive control: a registration that is still held blocks Close.
	held := openTestActionClient()
	if !held.inflight.enter() {
		t.Fatal("could not register")
	}
	if closeReturns(held) {
		t.Fatal("control: Close returned while a registration was held")
	}
	held.inflight.leave()

	c := openTestActionClient()
	func() {
		defer func() { _ = recover() }()
		_, _ = c.SendGoalContext(context.Background(), &hookGoal{hook: func() { panic("bad goal") }})
	}()
	if !closeReturns(c) {
		t.Fatal("Close is blocked: the panicking call left a registration")
	}
}

// A SerializeCDR that closes the client must not deadlock: the call registers with the client
// only after the goal is serialised. The call then finds the client closed.
func TestSendGoalContextSerializerMayCloseClient(t *testing.T) {
	c := openTestActionClient()
	stubSendGoal(t, func(*ActionClient, []byte) (*GoalHandle, error) {
		t.Error("a goal was sent for a client that its serializer closed")
		return nil, errors.New("stub")
	})
	done := make(chan error, 1)
	go func() {
		_, err := c.SendGoalContext(context.Background(), &hookGoal{hook: func() { _ = c.Close() }})
		done <- err
	}()
	if err := receiveBounded(t, done, "the call to return"); err == nil {
		t.Error("want an error for the closed client, got nil")
	}
}

// If ctx ends while the goal is serialised, nothing is sent.
func TestSendGoalContextEndedDuringSerializeSendsNothing(t *testing.T) {
	c := openTestActionClient()
	sends := 0
	stubSendGoal(t, func(_ *ActionClient, _ []byte) (*GoalHandle, error) {
		sends++
		return nil, errors.New("stub")
	})
	ctx, cancel := context.WithCancel(context.Background())
	h, err := c.SendGoalContext(ctx, &hookGoal{hook: cancel})
	if h != nil || !errors.Is(err, context.Canceled) {
		t.Fatalf("want context.Canceled and no handle, got h=%v err=%v", h, err)
	}
	_ = c.Close() // waits for any worker that started, so sends is final
	if sends != 0 {
		t.Errorf("the goal was sent %d times after ctx ended, want 0", sends)
	}
}

func stubActionResult(t *testing.T, f func() ([]byte, int32)) {
	t.Helper()
	old := actionResult
	actionResult = func(*cGoalHandleC) ([]byte, int32) { return f() }
	t.Cleanup(func() { actionResult = old })
}

// Positive control for the result tests below: a live context reaches the C call.
func TestGetResultWithContextReturnsResult(t *testing.T) {
	c := openTestActionClient()
	h := fakeGoalHandle(c)
	var calls atomic.Int32
	stubActionResult(t, func() ([]byte, int32) { calls.Add(1); return []byte{5}, 0 })

	got, err := h.GetResultWithContext(context.Background())
	if err != nil || len(got) != 1 || got[0] != 5 {
		t.Fatalf("want [5] and no error, got %v, %v", got, err)
	}
	if calls.Load() != 1 {
		t.Errorf("the C call ran %d times, want 1", calls.Load())
	}
	_ = c.Close()
}

// A context that has ended must not start a C result wait. The wait would hold the client
// registration, and ActionClient.Close would wait for it.
func TestGetResultWithContextAlreadyCancelledSkipsCall(t *testing.T) {
	c := openTestActionClient()
	h := fakeGoalHandle(c)
	var calls atomic.Int32
	stubActionResult(t, func() ([]byte, int32) { calls.Add(1); return []byte{5}, 0 })
	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	if _, err := h.GetResultWithContext(ctx); !errors.Is(err, context.Canceled) {
		t.Fatalf("GetResultWithContext: want context.Canceled, got %v", err)
	}
	if err := GetTypedResultContext(ctx, h, &MockMessage{}); !errors.Is(err, context.Canceled) {
		t.Fatalf("GetTypedResultContext: want context.Canceled, got %v", err)
	}
	_ = c.Close() // waits for any worker that started, so calls is final
	if n := calls.Load(); n != 0 {
		t.Errorf("the C call ran %d times after ctx ended, want 0", n)
	}
}

// An ended context wins over a closed handle, as it does in the service call variants.
func TestGetResultWithContextCancelledBeatsClosedHandle(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if _, err := (&GoalHandle{}).GetResultWithContext(ctx); !errors.Is(err, context.Canceled) {
		t.Fatalf("want context.Canceled, got %v", err)
	}
}

// errSerializerFailed is the error that failingGoal and failingReq return.
var errSerializerFailed = errors.New("serializer failed")

// failingGoal runs hook inside SerializeCDR and then fails.
type failingGoal struct{ hook func() }

func (g *failingGoal) TypeName() string { return "x" }
func (g *failingGoal) TypeHash() string { return "y" }
func (g *failingGoal) SerializeCDR() ([]byte, error) {
	g.hook()
	return nil, errSerializerFailed
}
func (g *failingGoal) DeserializeCDR([]byte) error { return nil }

// A serializer error that follows the end of ctx returns the context error.
func TestSendGoalContextEndedDuringFailingSerializeReturnsCtxErr(t *testing.T) {
	c := openTestActionClient()
	ctx, cancel := context.WithCancel(context.Background())
	_, err := c.SendGoalContext(ctx, &failingGoal{hook: cancel})
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("want context.Canceled, got %v", err)
	}
	_ = c.Close()
}

// Positive control: with a live context the serializer error comes back as it is.
func TestSendGoalContextFailingSerializeReturnsSerializerError(t *testing.T) {
	c := openTestActionClient()
	_, err := c.SendGoalContext(context.Background(), &failingGoal{hook: func() {}})
	if !errors.Is(err, errSerializerFailed) {
		t.Fatalf("want the serializer error, got %v", err)
	}
	_ = c.Close()
}

// After a cancel the caller may drop its last reference to the goal handle while the C result
// wait still runs. The wait must keep the handle alive, because the handle finalizer destroys
// the C goal handle that the wait reads.
func TestGetResultWithContextKeepsGoalHandleAliveDuringWait(t *testing.T) {
	c := openTestActionClient()
	started := make(chan struct{})
	release, free := newRelease(t)
	stubActionResult(t, func() ([]byte, int32) {
		close(started)
		<-release
		return nil, 1
	})
	var finalized atomic.Bool
	func() {
		h := fakeGoalHandle(c)
		runtime.SetFinalizer(h, func(*GoalHandle) { finalized.Store(true) })
		ctx, cancel := context.WithCancel(context.Background())
		done := make(chan error, 1)
		go func() { _, err := h.GetResultWithContext(ctx); done <- err }()
		receiveBounded(t, started, "the C wait to start")
		cancel()
		if err := receiveBounded(t, done, "the call to return"); !errors.Is(err, context.Canceled) {
			t.Fatalf("want context.Canceled, got %v", err)
		}
	}()

	for i := 0; i < 50; i++ {
		runtime.GC()
		time.Sleep(time.Millisecond)
	}
	if finalized.Load() {
		t.Fatal("the goal handle was finalized while the C result wait was still running")
	}

	// Positive control: once the wait has returned, the same handle is collected.
	free()
	_ = c.Close() // waits for the wait to return
	waitUntil(t, "the goal handle to be finalized after the wait", func() bool {
		runtime.GC()
		return finalized.Load()
	})
}

// gateGoal holds SerializeCDR at a barrier.
type gateGoal struct {
	MockMessage
	b *barrier
}

func (g *gateGoal) SerializeCDR() ([]byte, error) { g.b.hold(); return []byte{1}, nil }

// Cancelling while the goal is serialised does not end the call: serialisation runs on the
// caller's goroutine before the worker starts. The call returns when the serializer returns,
// with the context error, and nothing is sent.
func TestSendGoalContextCancelDuringSerializeWaitsForSerializer(t *testing.T) {
	c := openTestActionClient()
	var sends atomic.Int32
	stubSendGoal(t, func(*ActionClient, []byte) (*GoalHandle, error) {
		sends.Add(1)
		return nil, errors.New("stub")
	})
	b := newBarrier()
	defer b.free()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	done := make(chan error, 1)
	go func() { _, err := c.SendGoalContext(ctx, &gateGoal{b: b}); done <- err }()
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
	_ = c.Close() // waits for any worker that started, so sends is final
	if n := sends.Load(); n != 0 {
		t.Errorf("the goal was sent %d times for a call cancelled in the serializer, want 0", n)
	}
}

func stubActionCancel(t *testing.T, f func() int32) {
	t.Helper()
	old := actionCancel
	actionCancel = func(*cGoalHandleC) int32 { return f() }
	t.Cleanup(func() { actionCancel = old })
}

// Positive control for the cancel tests below: before Close, Cancel reaches the C call.
func TestGoalHandleCancelBeforeCloseReachesCCall(t *testing.T) {
	c := openTestActionClient()
	h := fakeGoalHandle(c)
	var calls atomic.Int32
	stubActionCancel(t, func() int32 { calls.Add(1); return 0 })

	if err := h.Cancel(); err != nil {
		t.Fatalf("Cancel: want no error, got %v", err)
	}
	if calls.Load() != 1 {
		t.Errorf("the C call ran %d times, want 1", calls.Load())
	}
	if h.Status() != GoalStatusCanceling {
		t.Errorf("status %v, want canceling", h.Status())
	}
	_ = c.Close()
}

// Control for the cancel tests: a result wait on a closed client already returns the closed
// error and makes no C call.
func TestGoalHandleGetResultAfterCloseReturnsClosedError(t *testing.T) {
	c := openTestActionClient()
	h := fakeGoalHandle(c)
	var calls atomic.Int32
	stubActionResult(t, func() ([]byte, int32) { calls.Add(1); return []byte{5}, 0 })
	_ = c.Close()

	if _, err := h.GetResultWithContext(context.Background()); err == nil || err.Error() != "action client is closed" {
		t.Fatalf("want the closed-client error, got %v", err)
	}
	if calls.Load() != 0 {
		t.Errorf("the C call ran %d times after Close, want 0", calls.Load())
	}
}

// A handle that outlives its client must not reach the C call: the C call reads the client
// that Close freed.
func TestGoalHandleCancelAfterCloseReturnsClosedError(t *testing.T) {
	c := openTestActionClient()
	h := fakeGoalHandle(c)
	var calls atomic.Int32
	stubActionCancel(t, func() int32 { calls.Add(1); return 0 })
	_ = c.Close()

	err := h.Cancel()
	if err == nil || err.Error() != "action client is closed" {
		t.Fatalf("want the closed-client error, got %v", err)
	}
	if n := calls.Load(); n != 0 {
		t.Errorf("the C call ran %d times after Close, want 0", n)
	}
}

// Close waits for a registered cancel call, as it does for the other registered calls.
func TestActionClientCloseWaitsForRegisteredCancel(t *testing.T) {
	c := openTestActionClient()
	h := fakeGoalHandle(c)
	started := make(chan struct{})
	release, free := newRelease(t)
	stubActionCancel(t, func() int32 {
		close(started)
		<-release
		return 0
	})
	done := make(chan error, 1)
	go func() { done <- h.Cancel() }()
	receiveBounded(t, started, "the C call to start")

	closed := make(chan struct{})
	go func() { _ = c.Close(); close(closed) }()
	select {
	case <-closed:
		t.Fatal("Close returned while a cancel call was still running")
	case <-time.After(100 * time.Millisecond):
	}
	free()
	receiveBounded(t, closed, "Close to return after the C call ended")
	receiveBounded(t, done, "Cancel to return")
}

// cancelInto cancels h and reports the result. It does not use h after the call.
func cancelInto(h *GoalHandle, done chan<- error) { done <- h.Cancel() }

// Cancel keeps the goal handle alive for the whole C call, also when the call fails: the
// handle finalizer destroys the C handle that the call reads. Today the status update after
// the call keeps the handle live on every path. This test fails if that line and the
// runtime.KeepAlive call both go.
func TestGoalHandleCancelKeepsGoalHandleAliveDuringCall(t *testing.T) {
	c := openTestActionClient()
	started := make(chan struct{})
	release, free := newRelease(t)
	stubActionCancel(t, func() int32 {
		close(started)
		<-release
		return 1
	})
	var finalized atomic.Bool
	done := make(chan error, 1)
	func() {
		h := fakeGoalHandle(c)
		runtime.SetFinalizer(h, func(*GoalHandle) { finalized.Store(true) })
		// The goroutine takes h as an argument. A closure over h would keep it reachable.
		go cancelInto(h, done)
		receiveBounded(t, started, "the C call to start")
	}()

	for i := 0; i < 50; i++ {
		runtime.GC()
		time.Sleep(time.Millisecond)
	}
	if finalized.Load() {
		t.Fatal("the goal handle was finalized while the C cancel call was still running")
	}

	// Positive control: once the call has returned, the same handle is collected.
	free()
	if err := receiveBounded(t, done, "Cancel to return"); err == nil {
		t.Error("want the cancel failure, got nil")
	}
	waitUntil(t, "the goal handle to be finalized after the call", func() bool {
		runtime.GC()
		return finalized.Load()
	})
	_ = c.Close()
}
