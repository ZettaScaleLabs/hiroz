package hiroz

import (
	"errors"
	"fmt"
	"testing"
)

func TestHirozError(t *testing.T) {
	err := newHirozError(ErrorCodeServiceTimeout, "service timed out")

	if err.Code() != ErrorCodeServiceTimeout {
		t.Errorf("Code() = %d, want %d", err.Code(), ErrorCodeServiceTimeout)
	}

	if err.Message() != "service timed out" {
		t.Errorf("Message() = %q, want %q", err.Message(), "service timed out")
	}

	expected := "hiroz error -10: service timed out"
	if err.Error() != expected {
		t.Errorf("Error() = %q, want %q", err.Error(), expected)
	}
}

func TestHirozErrorTimeout(t *testing.T) {
	tests := []struct {
		code      ErrorCode
		isTimeout bool
	}{
		{ErrorCodeServiceTimeout, true},
		{ErrorCodeServiceCallFailed, false},
		{ErrorCodeActionGoalRejected, false},
		{ErrorCodeSuccess, false},
	}

	for _, tt := range tests {
		err := newHirozError(tt.code, "test")
		if got := err.Timeout(); got != tt.isTimeout {
			t.Errorf("Timeout() for code %d = %v, want %v", tt.code, got, tt.isTimeout)
		}
		// IsTimeout is a deprecated alias; must return the same result
		if got := err.IsTimeout(); got != tt.isTimeout {
			t.Errorf("IsTimeout() for code %d = %v, want %v", tt.code, got, tt.isTimeout)
		}
	}
}

func TestHirozErrorIsError(t *testing.T) {
	// Verify HirozError implements error interface
	var err error = newHirozError(ErrorCodePublishFailed, "test")
	if err.Error() == "" {
		t.Error("HirozError should implement error interface")
	}
}

func TestHirozErrorTypeAssertion(t *testing.T) {
	var err error = newHirozError(ErrorCodeServiceTimeout, "timeout occurred")

	hirozErr, ok := err.(HirozError)
	if !ok {
		t.Fatal("type assertion to HirozError failed")
	}

	if hirozErr.Code() != ErrorCodeServiceTimeout {
		t.Errorf("Code() = %d, want %d", hirozErr.Code(), ErrorCodeServiceTimeout)
	}

	if !hirozErr.Timeout() {
		t.Error("Timeout() should return true")
	}
}

func TestHirozErrorWithErrors(t *testing.T) {
	err := newHirozError(ErrorCodeActionGoalRejected, "goal rejected")

	// errors.Is matches by error code (message is ignored)
	if !errors.Is(err, newHirozError(ErrorCodeActionGoalRejected, "different message")) {
		t.Error("errors.Is should match HirozError with same code")
	}

	// errors.Is should not match different codes
	if errors.Is(err, newHirozError(ErrorCodeServiceTimeout, "goal rejected")) {
		t.Error("errors.Is should not match HirozError with different code")
	}

	// Sentinel errors should work with errors.Is
	if !errors.Is(err, ErrGoalRejected) {
		t.Error("errors.Is should match sentinel ErrGoalRejected")
	}

	timeoutErr := newHirozError(ErrorCodeServiceTimeout, "call timed out")
	if !errors.Is(timeoutErr, ErrTimeout) {
		t.Error("errors.Is should match sentinel ErrTimeout")
	}

	// errors.As should work
	var targetErr HirozError
	if !errors.As(err, &targetErr) {
		t.Error("errors.As should work for HirozError")
	}

	if targetErr.Code() != ErrorCodeActionGoalRejected {
		t.Errorf("Code() after errors.As = %d, want %d", targetErr.Code(), ErrorCodeActionGoalRejected)
	}
}

func TestHirozErrorIsNoRecursion(t *testing.T) {
	// Wrapping a HirozError should not cause infinite recursion in Is()
	inner := newHirozError(ErrorCodeServiceTimeout, "inner timeout")
	wrapped := fmt.Errorf("outer: %w", inner)

	// errors.Is walks the chain and calls Is() — must not infinite-loop
	if !errors.Is(wrapped, ErrTimeout) {
		t.Error("errors.Is should find ErrTimeout through wrapped chain")
	}

	// Double-wrapped
	doubleWrapped := fmt.Errorf("double: %w", wrapped)
	if !errors.Is(doubleWrapped, ErrTimeout) {
		t.Error("errors.Is should find ErrTimeout through double-wrapped chain")
	}

	// Different code should not match
	if errors.Is(doubleWrapped, ErrGoalRejected) {
		t.Error("errors.Is should not match different error code in chain")
	}
}

func TestErrorCodeConstants(t *testing.T) {
	// Verify error code values match Rust FFI
	tests := []struct {
		code     ErrorCode
		expected int32
	}{
		{ErrorCodeSuccess, 0},
		{ErrorCodeNullPointer, -1},
		{ErrorCodeInvalidUtf8, -2},
		{ErrorCodeSessionClosed, -3},
		{ErrorCodePublishFailed, -4},
		{ErrorCodeSerializationFailed, -5},
		{ErrorCodeSubscribeFailed, -6},
		{ErrorCodeNodeCreationFailed, -7},
		{ErrorCodeContextCreationFailed, -8},
		{ErrorCodeServiceCallFailed, -9},
		{ErrorCodeServiceTimeout, -10},
		{ErrorCodeActionGoalRejected, -11},
		{ErrorCodeActionCancelFailed, -12},
		{ErrorCodeActionResultFailed, -13},
		{ErrorCodeActionFeedbackFailed, -14},
		{ErrorCodeDeserializationFailed, -15},
		{ErrorCodeUnknown, -100},
	}

	for _, tt := range tests {
		if int32(tt.code) != tt.expected {
			t.Errorf("ErrorCode value mismatch: got %d, want %d", tt.code, tt.expected)
		}
	}
}
