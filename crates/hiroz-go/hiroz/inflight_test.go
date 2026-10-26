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
// Before the gate, such a call loaded the handle that Close was about to free.
func TestCallTypedContextRefusedOnceCloseStarted(t *testing.T) {
	var calls atomic.Int32
	started := make(chan struct{})
	release := make(chan struct{})
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
	<-started

	closed := make(chan struct{})
	go func() { _ = c.Close(); close(closed) }()
	waitUntil(t, "Close to start", func() bool { return gateShut(&c.inflight) })

	err := CallTypedContext(context.Background(), c, mockReq(), &MockMessage{})
	if err == nil {
		t.Fatal("a call after Close started must fail")
	}
	if n := calls.Load(); n != 1 {
		t.Errorf("C calls = %d, want 1: the late call reached the C layer", n)
	}

	close(release)
	if err := <-first; err != nil {
		t.Errorf("the call that began before Close failed: %v", err)
	}
	<-closed
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
	<-entered

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
	release := make(chan struct{})
	stubSendGoal(t, func(cl *ActionClient, _ []byte) (*GoalHandle, error) {
		if sends.Add(1) == 1 {
			close(started)
			<-release
		}
		return fakeGoalHandle(cl), nil
	})

	first := make(chan error, 1)
	go func() { h, err := c.SendGoalContext(context.Background(), goalMsg()); _ = h; first <- err }()
	<-started

	closed := make(chan struct{})
	go func() { _ = c.Close(); close(closed) }()
	waitUntil(t, "Close to start", func() bool { return gateShut(&c.inflight) })

	h, err := c.SendGoalContext(context.Background(), goalMsg())
	if h != nil || err == nil {
		t.Fatalf("want an error and no handle, got h=%v err=%v", h, err)
	}
	if n := sends.Load(); n != 1 {
		t.Errorf("sends = %d, want 1: the late send reached the C layer", n)
	}
	close(release)
	if err := <-first; err != nil {
		t.Errorf("the send that began before Close failed: %v", err)
	}
	<-closed
}
