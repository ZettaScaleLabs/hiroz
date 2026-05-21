package rosz

import (
	"context"
	"errors"
	"testing"
	"time"
)

// These tests exercise the public surface of every binding type with a
// zero-valued / closed handle. They run without a live Zenoh session and
// guard against accidental nil-deref / use-after-close regressions when the
// FFI layer is refactored. Live behaviour is covered separately by
// interop_tests/ under the `integration` build tag.

// --- ServiceClient ---

func TestServiceClientCallOnClosed(t *testing.T) {
	c := &ServiceClient{service: "/dummy"}
	msg := &MockMessage{typeName: "x", typeHash: "y", data: []byte{1}}

	if _, err := c.call(msg); err == nil {
		t.Error("call: expected error on closed client")
	}
	if _, err := c.callWithTimeout(msg, 10*time.Millisecond); err == nil {
		t.Error("callWithTimeout: expected error on closed client")
	}
	if _, err := c.callRaw([]byte{1}, 10); err == nil {
		t.Error("callRaw: expected error on closed client")
	}
}

func TestServiceClientCallTypedOnClosed(t *testing.T) {
	c := &ServiceClient{service: "/dummy"}
	req := &MockMessage{typeName: "x", typeHash: "y", data: []byte{1}}
	resp := &MockMessage{}

	if err := CallTyped(c, req, resp); err == nil {
		t.Error("CallTyped: expected error on closed client")
	}
	if err := CallTypedWithTimeout(c, req, resp, 10*time.Millisecond); err == nil {
		t.Error("CallTypedWithTimeout: expected error on closed client")
	}
}

func TestServiceClientCloseIdempotent(t *testing.T) {
	c := &ServiceClient{service: "/dummy"}
	if err := c.Close(); err != nil {
		t.Errorf("Close on never-opened client returned error: %v", err)
	}
	if err := c.Close(); err != nil {
		t.Errorf("second Close returned error: %v", err)
	}
}

// --- ServiceServer ---

func TestServiceServerCloseIdempotent(t *testing.T) {
	s := &ServiceServer{service: "/dummy"}
	if err := s.Close(); err != nil {
		t.Errorf("Close on never-opened server returned error: %v", err)
	}
	if err := s.Close(); err != nil {
		t.Errorf("second Close returned error: %v", err)
	}
}

// --- Publisher ---

func TestPublisherCloseIdempotent(t *testing.T) {
	p := &Publisher{topic: "/dummy"} // handle == nil
	if err := p.Close(); err != nil {
		t.Errorf("Close on closed publisher returned error: %v", err)
	}
	if err := p.Close(); err != nil {
		t.Errorf("second Close returned error: %v", err)
	}
}

// TestPublisherPublishOnClosed verifies Publish on a zero-handle publisher
// short-circuits with a "closed" error rather than falling through to the
// FFI null-pointer path.
func TestPublisherPublishOnClosed(t *testing.T) {
	p := &Publisher{topic: "/dummy"}
	msg := &MockMessage{typeName: "x", typeHash: "y", data: []byte{1}}

	err := p.Publish(msg)
	if err == nil {
		t.Fatal("Publish on closed publisher: expected error, got nil")
	}
	// Closed-handle errors are plain errors, not wrapped RoszErrors —
	// matches ServiceClient.callRaw and ActionClient.SendGoal.
	var rerr RoszError
	if errors.As(err, &rerr) {
		t.Errorf("closed publisher should not surface a RoszError, got: %v", err)
	}
}

// --- Subscriber ---

func TestSubscriberCloseIdempotent(t *testing.T) {
	s := &Subscriber{}
	if err := s.Close(); err != nil {
		t.Errorf("Close on closed subscriber returned error: %v", err)
	}
	if err := s.Close(); err != nil {
		t.Errorf("second Close returned error: %v", err)
	}
}

// --- ActionClient / GoalHandle ---

func TestActionClientSendGoalOnClosed(t *testing.T) {
	c := &ActionClient{action: "/dummy"}
	goal := &MockMessage{typeName: "x", typeHash: "y", data: []byte{1}}

	if _, err := c.SendGoal(goal); err == nil {
		t.Error("SendGoal on closed action client: expected error")
	}
}

func TestActionClientCloseIdempotent(t *testing.T) {
	c := &ActionClient{action: "/dummy"}
	if err := c.Close(); err != nil {
		t.Errorf("Close on closed action client returned error: %v", err)
	}
	if err := c.Close(); err != nil {
		t.Errorf("second Close returned error: %v", err)
	}
}

func TestGoalHandleOnClosed(t *testing.T) {
	h := &GoalHandle{}

	if err := h.Cancel(); err == nil {
		t.Error("Cancel on closed goal handle: expected error")
	}
	if _, err := h.GetResult(); err == nil {
		t.Error("GetResult on closed goal handle: expected error")
	}

	ctx, cancel := context.WithTimeout(context.Background(), 50*time.Millisecond)
	defer cancel()
	if _, err := h.GetResultWithContext(ctx); err == nil {
		t.Error("GetResultWithContext on closed goal handle: expected error")
	}

	if err := h.Close(); err != nil {
		t.Errorf("Close on closed goal handle returned error: %v", err)
	}
	if err := h.Close(); err != nil {
		t.Errorf("second Close returned error: %v", err)
	}
}

func TestGoalHandleStatusAccessors(t *testing.T) {
	h := &GoalHandle{}
	// Zero value is GoalStatusUnknown.
	if got := h.Status(); got != GoalStatusUnknown {
		t.Errorf("Status() zero value = %v, want %v", got, GoalStatusUnknown)
	}
	if h.IsActive() {
		t.Error("zero GoalHandle should not be active")
	}
	if h.IsTerminal() {
		t.Error("zero GoalHandle should not be terminal")
	}

	h.setStatus(GoalStatusExecuting)
	if !h.IsActive() {
		t.Error("Executing should be active")
	}
	h.setStatus(GoalStatusSucceeded)
	if !h.IsTerminal() {
		t.Error("Succeeded should be terminal")
	}

	// GoalID() returns the stored ID (zero value here).
	var zero GoalID
	if id := h.GoalID(); id != zero {
		t.Errorf("GoalID() = %v, want zero", id)
	}
}

// --- ActionServer / ServerGoalHandle ---

func TestActionServerCloseIdempotent(t *testing.T) {
	s := &ActionServer{action: "/dummy"}
	if err := s.Close(); err != nil {
		t.Errorf("Close on closed action server returned error: %v", err)
	}
	if err := s.Close(); err != nil {
		t.Errorf("second Close returned error: %v", err)
	}
}

func TestServerGoalHandleOnClosedServer(t *testing.T) {
	srv := &ActionServer{action: "/dummy"} // handle is nil
	h := &ServerGoalHandle{server: srv}
	feedback := &MockMessage{typeName: "x", typeHash: "y", data: []byte{1}}
	result := &MockMessage{typeName: "x", typeHash: "y", data: []byte{1}}

	if got := h.IsCancelRequested(); got {
		t.Error("IsCancelRequested on closed server: expected false")
	}
	if err := h.PublishFeedback(feedback); err == nil {
		t.Error("PublishFeedback on closed server: expected error")
	}
	if err := h.Succeed(result); err == nil {
		t.Error("Succeed on closed server: expected error")
	}
	if err := h.Abort(result); err == nil {
		t.Error("Abort on closed server: expected error")
	}
	if err := h.Canceled(result); err == nil {
		t.Error("Canceled on closed server: expected error")
	}
}

// --- Node / Context ---

func TestNodeCloseIdempotent(t *testing.T) {
	n := &Node{} // handle is nil
	if err := n.Close(); err != nil {
		t.Errorf("Close on never-opened node returned error: %v", err)
	}
	if err := n.Close(); err != nil {
		t.Errorf("second Close returned error: %v", err)
	}
}

func TestContextCloseIdempotent(t *testing.T) {
	c := &Context{} // handle is nil
	if err := c.Close(); err != nil {
		t.Errorf("Close on never-opened context returned error: %v", err)
	}
	if err := c.Close(); err != nil {
		t.Errorf("second Close returned error: %v", err)
	}
}

// --- Graph queries on closed context ---

func TestGraphQueriesOnClosedContext(t *testing.T) {
	c := &Context{} // handle is nil

	if _, err := c.GetTopicNamesAndTypes(); err == nil {
		t.Error("GetTopicNamesAndTypes on closed context: expected error")
	}
	if _, err := c.GetNodeNames(); err == nil {
		t.Error("GetNodeNames on closed context: expected error")
	}
	if _, err := c.GetServiceNamesAndTypes(); err == nil {
		t.Error("GetServiceNamesAndTypes on closed context: expected error")
	}
	if _, err := c.NodeExists("foo", "/"); err == nil {
		t.Error("NodeExists on closed context: expected error")
	}
}

// --- DestroySubscriber on unowned sub (no live Zenoh required) ---

func TestDestroySubscriberUnowned(t *testing.T) {
	n := &Node{}
	sub := &Subscriber{} // not added to n.ownedSubs
	if err := n.DestroySubscriber(sub); err != nil {
		t.Errorf("DestroySubscriber on unowned sub returned error: %v", err)
	}
}
