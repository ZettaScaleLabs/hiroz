package hiroz

import (
	"context"
	"errors"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

// select picks at random when ctx.Done and the result channel are both ready. The finish
// functions are what the result branch calls, so these tests give them an ended context and
// a ready result, the state in which select can take either branch. The ended context must
// win in every call below.

func TestFinishContextCallEndedContextBeatsSuccess(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	data, err := finishContextCall(ctx, []byte{1}, nil)
	if !errors.Is(err, context.Canceled) || data != nil {
		t.Fatalf("want context.Canceled and no data, got data=%v err=%v", data, err)
	}
}

// A positive control: with a live context the same result passes through.
func TestFinishContextCallLiveContextKeepsResult(t *testing.T) {
	data, err := finishContextCall(context.Background(), []byte{1}, nil)
	if err != nil || len(data) != 1 {
		t.Fatalf("want the result, got data=%v err=%v", data, err)
	}
}

func TestFinishSendGoalEndedContextClosesHandle(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	h := &GoalHandle{}
	got, err := finishSendGoal(ctx, h, nil)
	if got != nil || !errors.Is(err, context.Canceled) {
		t.Fatalf("want context.Canceled and no handle, got h=%v err=%v", got, err)
	}
	// Close ran: it spent the close-once guard, so this Do does not run its function.
	spent := true
	h.closeOnce.Do(func() { spent = false })
	if !spent {
		t.Fatal("the handle was not closed")
	}
}

func TestFinishSendGoalLiveContextKeepsHandle(t *testing.T) {
	h := &GoalHandle{}
	got, err := finishSendGoal(context.Background(), h, nil)
	if got != h || err != nil {
		t.Fatalf("want the handle, got h=%v err=%v", got, err)
	}
	// A positive control for the close check above: the guard is unspent here.
	unspent := false
	h.closeOnce.Do(func() { unspent = true })
	if !unspent {
		t.Fatal("a live context closed the handle")
	}
}

func TestFinishGetResultEndedContextBeatsSuccess(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	data, err := finishGetResult(ctx, &GoalHandle{}, []byte{1}, nil)
	if !errors.Is(err, context.Canceled) || data != nil {
		t.Fatalf("want context.Canceled and no data, got data=%v err=%v", data, err)
	}
}

// A positive control: with a live context the result passes through, and so does a failure.
func TestFinishGetResultLiveContextKeepsOutcome(t *testing.T) {
	h := &GoalHandle{}
	data, err := finishGetResult(context.Background(), h, []byte{1}, nil)
	if err != nil || len(data) != 1 || data[0] != 1 {
		t.Fatalf("want the result [1], got data=%v err=%v", data, err)
	}
	if h.Status() != GoalStatusSucceeded {
		t.Errorf("status %v, want succeeded", h.Status())
	}
	failure := errors.New("wait failed")
	if _, err := finishGetResult(context.Background(), &GoalHandle{}, nil, failure); !errors.Is(err, failure) {
		t.Errorf("want the failure, got %v", err)
	}
}

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
	run := func(end bool) ([]byte, error) {
		returned = make(chan struct{})
		return fakeGoalHandle(c).GetResultWithContext(newLateEndCtx(returned, end))
	}
	// Positive control: a context that never ends returns the result.
	if got, err := run(false); err != nil || len(got) != 1 || got[0] != 5 {
		t.Fatalf("control: want [5], got %v, %v", got, err)
	}
	for i := 0; i < raceRounds; i++ {
		if got, err := run(true); got != nil || !errors.Is(err, context.Canceled) {
			t.Fatalf("round %d: want context.Canceled and no data, got %v, %v", i, got, err)
		}
	}
	_ = c.Close()
}
