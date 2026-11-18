package hiroz

import (
	"context"
	"errors"
	"runtime"
	"sync/atomic"
	"testing"
	"time"
)

// gateShut reports whether shutdown has started on the gate.
func gateShut(g *inflightGate) bool {
	g.mu.Lock()
	defer g.mu.Unlock()
	return g.shut
}

// A call that arrives after Close has started must not register and must not reach C.
func TestCallTypedContextRefusedOnceCloseStarted(t *testing.T) {
	var calls atomic.Int32
	started := make(chan struct{})
	release, free := newRelease(t)
	stubServiceCall(t, func([]byte, uint64) ([]byte, int32) {
		if calls.Add(1) == 1 {
			close(started)
			<-release
		}
		return []byte{1}, 0
	})
	c := openTestClient()

	first := make(chan error, 1)
	go func() { first <- CallTypedContext(context.Background(), c, mockReq(), &MockMessage{}) }()
	receiveBounded(t, started, "the first call to reach the C layer")

	closed := make(chan struct{})
	go func() { _ = c.Close(); close(closed) }()
	waitUntil(t, "Close to start", func() bool { return gateShut(&c.inflight) })

	late := make(chan error, 1)
	go func() { late <- CallTypedContext(context.Background(), c, mockReq(), &MockMessage{}) }()
	if err := receiveBounded(t, late, "the late call to return"); err == nil {
		t.Fatal("a call after Close started must fail")
	}
	if n := calls.Load(); n != 1 {
		t.Errorf("C calls = %d, want 1: the late call reached the C layer", n)
	}

	free()
	if err := receiveBounded(t, first, "the first call to return"); err != nil {
		t.Errorf("the call that began before Close failed: %v", err)
	}
	receiveBounded(t, closed, "Close to return")
}

// Close must end a WaitForServiceContext that has no deadline and no server.
func TestCloseEndsWaitForServiceContext(t *testing.T) {
	entered := make(chan struct{})
	var once atomic.Bool
	stubServiceWait(t, func(uint64) int32 {
		if once.CompareAndSwap(false, true) {
			close(entered)
		}
		runtime.Gosched()
		return int32(ErrorCodeServiceTimeout)
	})
	c := openTestClient()
	waitErr := make(chan error, 1)
	go func() { waitErr <- c.WaitForServiceContext(context.Background()) }()
	receiveBounded(t, entered, "the wait to start")

	closed := make(chan struct{})
	go func() { _ = c.Close(); close(closed) }()

	select {
	case err := <-waitErr:
		if err == nil || errors.Is(err, context.Canceled) || errors.Is(err, context.DeadlineExceeded) {
			t.Errorf("want a closed-client error, got %v", err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("WaitForServiceContext did not return after Close")
	}
	select {
	case <-closed:
	case <-time.After(5 * time.Second):
		t.Fatal("Close did not return")
	}
}

func TestWaitSliceMs(t *testing.T) {
	cases := []struct {
		name      string
		remaining time.Duration
		ms        uint64
		expired   bool
	}{
		{"past", -time.Second, 0, true},
		{"just past", -time.Nanosecond, 0, true},
		{"now", 0, 0, true},
		{"under 1 ms rounds up", 500 * time.Microsecond, 1, false},
		{"fraction rounds up", 1500 * time.Microsecond, 2, false},
		{"exact", 40 * time.Millisecond, 40, false},
		{"capped at a slice", time.Hour, 250, false},
	}
	for _, tc := range cases {
		ms, expired := waitSliceMs(tc.remaining)
		if ms != tc.ms || expired != tc.expired {
			t.Errorf("%s: waitSliceMs(%v) = (%d, %v), want (%d, %v)", tc.name, tc.remaining, ms, expired, tc.ms, tc.expired)
		}
	}
}

// A send that arrives after Close has started must not register and must not reach C.
func TestSendGoalContextRefusedOnceCloseStarted(t *testing.T) {
	c := openTestActionClient()
	var sends atomic.Int32
	started := make(chan struct{})
	release, free := newRelease(t)
	stubSendGoal(t, func(cl *ActionClient, _ []byte) (*GoalHandle, error) {
		if sends.Add(1) == 1 {
			close(started)
			<-release
		}
		return fakeGoalHandle(cl), nil
	})

	type sent struct {
		h   *GoalHandle
		err error
	}
	first := make(chan sent, 1)
	go func() { h, err := c.SendGoalContext(context.Background(), goalMsg()); first <- sent{h, err} }()
	receiveBounded(t, started, "the first call to reach the C layer")

	closed := make(chan struct{})
	go func() { _ = c.Close(); close(closed) }()
	waitUntil(t, "Close to start", func() bool { return gateShut(&c.inflight) })

	lateRes := make(chan sent, 1)
	go func() { h, err := c.SendGoalContext(context.Background(), goalMsg()); lateRes <- sent{h, err} }()
	if r := receiveBounded(t, lateRes, "the late send to return"); r.h != nil || r.err == nil {
		t.Fatalf("want an error and no handle, got h=%v err=%v", r.h, r.err)
	}
	if n := sends.Load(); n != 1 {
		t.Errorf("sends = %d, want 1: the late send reached the C layer", n)
	}
	free()
	if r := receiveBounded(t, first, "the first call to return"); r.err != nil || r.h == nil {
		t.Errorf("the send that began before Close returned h=%v err=%v, want a handle and no error", r.h, r.err)
	}
	receiveBounded(t, closed, "Close to return")
}
