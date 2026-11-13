package hiroz

import (
	"context"
	"errors"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

// lateEndCtx ends only after the C stub has returned. Its Done method is the one the call
// evaluates in its select. It waits for the stub, gives the worker time to queue the result,
// and only then reports the end. The result channel and Done are then both ready when select
// runs, which is the state in which select may take either branch.
type lateEndCtx struct {
	context.Context
	stubReturned <-chan struct{}
	end          bool
	once         sync.Once
	ended        atomic.Bool
	done         chan struct{}
}

func newLateEndCtx(stubReturned <-chan struct{}, end bool) *lateEndCtx {
	return &lateEndCtx{Context: context.Background(), stubReturned: stubReturned, end: end, done: make(chan struct{})}
}

func (c *lateEndCtx) Done() <-chan struct{} {
	c.once.Do(func() {
		<-c.stubReturned
		time.Sleep(5 * time.Millisecond) // let the worker queue its result
		if c.end {
			c.ended.Store(true)
			close(c.done)
		}
	})
	return c.done
}

func (c *lateEndCtx) Err() error {
	if c.ended.Load() {
		return context.Canceled
	}
	return nil
}

// raceRounds is how often a test runs the state above. select picks either branch with equal
// odds, so a call that skips the second check on ctx fails a round with probability 1/2.
const raceRounds = 30

func TestCallTypedContextEndedContextBeatsReadyResult(t *testing.T) {
	var returned chan struct{}
	stubServiceCall(t, func([]byte, uint64) ([]byte, int32) {
		defer close(returned)
		return []byte{7}, 0
	})
	run := func(end bool) error {
		returned = make(chan struct{})
		return CallTypedContext(newLateEndCtx(returned, end), openTestClient(), mockReq(), &MockMessage{})
	}
	// Positive control: a context that never ends lets the same result through.
	if err := run(false); err != nil {
		t.Fatalf("control: want the result, got %v", err)
	}
	for i := 0; i < raceRounds; i++ {
		if err := run(true); !errors.Is(err, context.Canceled) {
			t.Fatalf("round %d: want context.Canceled, got %v", i, err)
		}
	}
}

func TestSendGoalContextEndedContextBeatsReadyHandle(t *testing.T) {
	c := openTestActionClient()
	var returned chan struct{}
	var handle *GoalHandle
	stubSendGoal(t, func(*ActionClient, []byte) (*GoalHandle, error) {
		defer close(returned)
		handle = fakeGoalHandle(c)
		return handle, nil
	})
	run := func(end bool) (*GoalHandle, error) {
		returned = make(chan struct{})
		return c.SendGoalContext(newLateEndCtx(returned, end), goalMsg())
	}
	// Positive control: a context that never ends returns the handle, still open.
	h, err := run(false)
	if err != nil || h == nil || h.handle.Load() == nil {
		t.Fatalf("control: want an open handle, got h=%v err=%v", h, err)
	}
	for i := 0; i < raceRounds; i++ {
		h, err := run(true)
		if h != nil || !errors.Is(err, context.Canceled) {
			t.Fatalf("round %d: want context.Canceled and no handle, got h=%v err=%v", i, h, err)
		}
		late := handle
		waitUntil(t, "the late goal handle to close", func() bool { return late.handle.Load() == nil })
	}
	_ = c.Close()
}

func TestGetResultWithContextEndedContextBeatsReadyResult(t *testing.T) {
	c := openTestActionClient()
	var returned chan struct{}
	stubActionResult(t, func() ([]byte, int32) {
		defer close(returned)
		return []byte{5}, 0
	})
	run := func(h *GoalHandle, end bool) ([]byte, error) {
		returned = make(chan struct{})
		return h.GetResultWithContext(newLateEndCtx(returned, end))
	}
	// Positive control: a context that never ends returns the result and marks the goal done.
	h := fakeGoalHandle(c)
	if got, err := run(h, false); err != nil || len(got) != 1 || got[0] != 5 {
		t.Fatalf("control: want [5], got %v, %v", got, err)
	}
	if h.Status() != GoalStatusSucceeded {
		t.Errorf("control: status %v, want succeeded", h.Status())
	}
	for i := 0; i < raceRounds; i++ {
		if got, err := run(fakeGoalHandle(c), true); got != nil || !errors.Is(err, context.Canceled) {
			t.Fatalf("round %d: want context.Canceled and no data, got %v, %v", i, got, err)
		}
	}
	_ = c.Close()
}
