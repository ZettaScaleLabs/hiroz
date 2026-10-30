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
	// Close ran: the close-once guard is spent, so a second Close is a no-op that returns nil.
	if err := h.Close(); err != nil {
		t.Fatalf("second Close: %v", err)
	}
}

func TestFinishSendGoalLiveContextKeepsHandle(t *testing.T) {
	h := &GoalHandle{}
	got, err := finishSendGoal(context.Background(), h, nil)
	if got != h || err != nil {
		t.Fatalf("want the handle, got h=%v err=%v", got, err)
	}
}

// A cancelled call must not run the caller's serializer.
func TestCallTypedContextCancelledSkipsSerializer(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	req := &countingMessage{}
	err := CallTypedContext(ctx, openTestClient(), req, &MockMessage{})
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("want context.Canceled, got %v", err)
	}
	if req.serialised != 0 {
		t.Errorf("SerializeCDR ran %d times for a cancelled call", req.serialised)
	}
}

type countingMessage struct {
	MockMessage
	serialised int
}

func (m *countingMessage) SerializeCDR() ([]byte, error) {
	m.serialised++
	return []byte{1}, nil
}
