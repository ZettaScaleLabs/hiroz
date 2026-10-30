package hiroz

import (
	"context"
	"errors"
	"testing"
)

// select picks at random when ctx.Done and the result channel are both ready. The finish
// functions are what the result branch calls, so these tests give them an ended context and
// a ready result, the state in which select can take either branch. Without the recheck,
// each call below returned the result.

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
