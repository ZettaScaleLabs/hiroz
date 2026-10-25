package hiroz

import (
	"context"
	"errors"
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

func stubSendGoal(t *testing.T, f func(c *ActionClient, goal Message) (*GoalHandle, error)) {
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
	stubSendGoal(t, func(*ActionClient, Message) (*GoalHandle, error) { return want, nil })

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
	stubSendGoal(t, func(*ActionClient, Message) (*GoalHandle, error) {
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
