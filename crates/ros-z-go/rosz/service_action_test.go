package rosz

import (
	"errors"
	"testing"
	"time"
)

// --- GoalStatus Tests ---

func TestGoalStatusIsActive(t *testing.T) {
	tests := []struct {
		status GoalStatus
		active bool
	}{
		{GoalStatusUnknown, false},
		{GoalStatusAccepted, true},
		{GoalStatusExecuting, true},
		{GoalStatusCanceling, true},
		{GoalStatusSucceeded, false},
		{GoalStatusCanceled, false},
		{GoalStatusAborted, false},
	}

	for _, tt := range tests {
		if got := tt.status.IsActive(); got != tt.active {
			t.Errorf("GoalStatus(%d).IsActive() = %v, want %v", tt.status, got, tt.active)
		}
	}
}

func TestGoalStatusIsTerminal(t *testing.T) {
	tests := []struct {
		status   GoalStatus
		terminal bool
	}{
		{GoalStatusUnknown, false},
		{GoalStatusAccepted, false},
		{GoalStatusExecuting, false},
		{GoalStatusCanceling, false},
		{GoalStatusSucceeded, true},
		{GoalStatusCanceled, true},
		{GoalStatusAborted, true},
	}

	for _, tt := range tests {
		if got := tt.status.IsTerminal(); got != tt.terminal {
			t.Errorf("GoalStatus(%d).IsTerminal() = %v, want %v", tt.status, got, tt.terminal)
		}
	}
}

// --- GoalID Tests ---

func TestGoalIDSize(t *testing.T) {
	var id GoalID
	if len(id) != 16 {
		t.Errorf("GoalID length = %d, want 16", len(id))
	}
}

func TestGoalIDZeroValue(t *testing.T) {
	var id GoalID
	for i, b := range id {
		if b != 0 {
			t.Errorf("zero GoalID[%d] = %d, want 0", i, b)
		}
	}
}

// --- Service Interface Tests ---

type mockService struct{}

func (s *mockService) TypeName() string                 { return "test/srv/Mock" }
func (s *mockService) TypeHash() string                 { return "RIHS01_mock" }
func (s *mockService) SerializeCDR() ([]byte, error)    { return nil, nil }
func (s *mockService) DeserializeCDR(data []byte) error { return nil }
func (s *mockService) GetRequest() Message {
	return &MockMessage{typeName: "test/srv/Mock_Request", typeHash: "RIHS01_mock_req"}
}
func (s *mockService) GetResponse() Message {
	return &MockMessage{typeName: "test/srv/Mock_Response", typeHash: "RIHS01_mock_resp"}
}

func TestServiceInterface(t *testing.T) {
	var _ Service = (*mockService)(nil)

	svc := &mockService{}
	if svc.TypeName() != "test/srv/Mock" {
		t.Errorf("TypeName() = %q", svc.TypeName())
	}

	req := svc.GetRequest()
	if req.TypeName() != "test/srv/Mock_Request" {
		t.Errorf("GetRequest().TypeName() = %q", req.TypeName())
	}

	resp := svc.GetResponse()
	if resp.TypeName() != "test/srv/Mock_Response" {
		t.Errorf("GetResponse().TypeName() = %q", resp.TypeName())
	}
}

// TestWaitForServiceClosedClient verifies WaitForService returns an error when
// the client has already been closed, without requiring a live Zenoh session.
func TestWaitForServiceClosedClient(t *testing.T) {
	c := &ServiceClient{service: "/dummy"} // handle is nil
	err := c.WaitForService(10 * time.Millisecond)
	if err == nil {
		t.Fatal("expected error from WaitForService on closed client, got nil")
	}
	// Closed-client error should not be wrapped as a service timeout.
	var rerr RoszError
	if errors.As(err, &rerr) && rerr.Code() == ErrorCodeServiceTimeout {
		t.Errorf("closed client should not surface ServiceTimeout, got: %v", err)
	}
}

// --- Action Interface Tests ---

type mockAction struct{}

func (a *mockAction) TypeName() string { return "test/action/Mock" }
func (a *mockAction) GetGoal() Message {
	return &MockMessage{typeName: "test/action/Mock_Goal", typeHash: "RIHS01_mock_goal"}
}
func (a *mockAction) GetResult() Message {
	return &MockMessage{typeName: "test/action/Mock_Result", typeHash: "RIHS01_mock_result"}
}
func (a *mockAction) GetFeedback() Message {
	return &MockMessage{typeName: "test/action/Mock_Feedback", typeHash: "RIHS01_mock_feedback"}
}
func (a *mockAction) SendGoalHash() string        { return "RIHS01_mock_send_goal" }
func (a *mockAction) GetResultHash() string       { return "RIHS01_mock_get_result" }
func (a *mockAction) CancelGoalHash() string      { return "RIHS01_mock_cancel_goal" }
func (a *mockAction) FeedbackMessageHash() string { return "RIHS01_mock_feedback_msg" }
func (a *mockAction) StatusHash() string          { return "RIHS01_mock_status" }

func TestActionInterface(t *testing.T) {
	var _ Action = (*mockAction)(nil)

	action := &mockAction{}
	if action.TypeName() != "test/action/Mock" {
		t.Errorf("TypeName() = %q", action.TypeName())
	}

	goal := action.GetGoal()
	if goal.TypeName() != "test/action/Mock_Goal" {
		t.Errorf("GetGoal().TypeName() = %q", goal.TypeName())
	}

	result := action.GetResult()
	if result.TypeName() != "test/action/Mock_Result" {
		t.Errorf("GetResult().TypeName() = %q", result.TypeName())
	}

	feedback := action.GetFeedback()
	if feedback.TypeName() != "test/action/Mock_Feedback" {
		t.Errorf("GetFeedback().TypeName() = %q", feedback.TypeName())
	}
}

// --- Closure Tests ---
// Note: The callback registry has been replaced with cgo.Handle-based closures.
// The closures are tested indirectly through the FFI integration tests.
