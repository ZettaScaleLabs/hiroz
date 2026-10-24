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
