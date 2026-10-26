package hiroz

import (
	"context"
	"errors"
	"sync"
	"testing"
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
	release := make(chan struct{})
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
	<-started
	cancel()

	r := <-done
	if r.h != nil || !errors.Is(r.err, context.Canceled) {
		t.Fatalf("want context.Canceled and no handle, got h=%v err=%v", r.h, r.err)
	}
	if late.handle.Load() == nil {
		t.Fatal("the goal handle closed before the C layer returned")
	}

	closed := make(chan struct{})
	go func() { _ = c.Close(); close(closed) }()

	close(release)
	waitUntil(t, "the late goal handle to close", func() bool { return late.handle.Load() == nil })
	<-closed
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
	release := make(chan struct{})
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
	<-started
	cancel()
	if err := <-done; !errors.Is(err, context.Canceled) {
		t.Fatalf("want context.Canceled, got %v", err)
	}
	if n := goal.serialisations(); n != 1 {
		t.Errorf("the goal was serialised %d times before the call returned, want 1", n)
	}
	goal.set([]byte{9}) // the caller reuses the goal
	close(release)
	if got := <-sent; len(got) != 1 || got[0] != 1 {
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
	release := make(chan struct{})
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
	<-started
	cancel()
	if err := <-done; !errors.Is(err, context.Canceled) {
		t.Fatalf("want context.Canceled, got %v", err)
	}
	goal.data[0] = 9 // the caller reuses the goal storage in place
	close(release)
	if got := <-sent; len(got) != 2 || got[0] != 1 || got[1] != 2 {
		t.Errorf("the C layer sent %v, want the goal as it was at the call: [1 2]", got)
	}
	_ = c.Close()
}
