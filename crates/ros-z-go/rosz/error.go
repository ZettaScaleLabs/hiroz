package rosz

import "fmt"

// ErrorCode represents FFI error codes returned from the Rust layer
type ErrorCode int32

const (
	// ErrorCodeSuccess indicates the operation completed successfully
	ErrorCodeSuccess ErrorCode = 0

	// ErrorCodeNullPointer indicates a null pointer was passed to FFI
	ErrorCodeNullPointer ErrorCode = -1

	// ErrorCodeInvalidUtf8 indicates invalid UTF-8 in string conversion
	ErrorCodeInvalidUtf8 ErrorCode = -2

	// ErrorCodeSessionClosed indicates the Zenoh session is closed
	ErrorCodeSessionClosed ErrorCode = -3

	// ErrorCodePublishFailed indicates message publishing failed
	ErrorCodePublishFailed ErrorCode = -4

	// ErrorCodeSerializationFailed indicates CDR serialization failed
	ErrorCodeSerializationFailed ErrorCode = -5

	// ErrorCodeSubscribeFailed indicates subscriber creation failed
	ErrorCodeSubscribeFailed ErrorCode = -6

	// ErrorCodeNodeCreationFailed indicates node creation failed
	ErrorCodeNodeCreationFailed ErrorCode = -7

	// ErrorCodeContextCreationFailed indicates context creation failed
	ErrorCodeContextCreationFailed ErrorCode = -8

	// ErrorCodeServiceCallFailed indicates service call failed
	ErrorCodeServiceCallFailed ErrorCode = -9

	// ErrorCodeServiceTimeout indicates service call timed out
	ErrorCodeServiceTimeout ErrorCode = -10

	// ErrorCodeActionGoalRejected indicates action goal was rejected by server
	ErrorCodeActionGoalRejected ErrorCode = -11

	// ErrorCodeActionCancelFailed indicates action cancellation failed
	ErrorCodeActionCancelFailed ErrorCode = -12

	// ErrorCodeActionResultFailed indicates getting action result failed
	ErrorCodeActionResultFailed ErrorCode = -13

	// ErrorCodeActionFeedbackFailed indicates publishing action feedback failed
	ErrorCodeActionFeedbackFailed ErrorCode = -14

	// ErrorCodeDeserializationFailed indicates CDR deserialization failed
	ErrorCodeDeserializationFailed ErrorCode = -15

	// ErrorCodeBuildFailed indicates a builder failed to construct the entity (FFI returned null)
	ErrorCodeBuildFailed ErrorCode = -16

	// ErrorCodeCloseFailed indicates a Close() call on an entity failed
	ErrorCodeCloseFailed ErrorCode = -17

	// ErrorCodeUnknown indicates an unknown error occurred
	ErrorCodeUnknown ErrorCode = -100
)

// RoszError represents a structured error from the ros-z FFI layer
type RoszError struct {
	code ErrorCode
	msg  string
}

// Error implements the error interface.
// Format: "rosz error N: message"
func (e RoszError) Error() string {
	return fmt.Sprintf("rosz error %d: %s", e.code, e.msg)
}

// Code returns the FFI error code
func (e RoszError) Code() ErrorCode {
	return e.code
}

// Message returns the error message without the code
func (e RoszError) Message() string {
	return e.msg
}

// newRoszError creates a new RoszError with the given code and message.
// This is an internal constructor; callers outside the package use errors.Is
// with the sentinel variables (ErrTimeout, ErrGoalRejected, etc.).
func newRoszError(code ErrorCode, msg string) RoszError {
	return RoszError{code: code, msg: msg}
}

// Timeout reports whether the error is a service call timeout.
// It satisfies the net.Error interface convention for timeout detection.
// Use errors.Is(err, rosz.ErrTimeout) for idiomatic timeout checks.
func (e RoszError) Timeout() bool {
	return e.code == ErrorCodeServiceTimeout
}

// IsTimeout reports whether the error is a service call timeout.
//
// Deprecated: use Timeout() or errors.Is(err, rosz.ErrTimeout) instead.
func (e RoszError) IsTimeout() bool {
	return e.Timeout()
}

// Is reports whether target matches this error by comparing error codes.
// This enables errors.Is() support for RoszError.
// Uses direct type assertion (not errors.As) to avoid recursive chain walking.
func (e RoszError) Is(target error) bool {
	t, ok := target.(RoszError)
	if ok {
		return e.code == t.code
	}
	return false
}

// Sentinel errors for common failure modes.
// Use errors.Is(err, rosz.ErrTimeout) etc. to check for specific conditions.
var (
	ErrTimeout      = newRoszError(ErrorCodeServiceTimeout, "service call timed out")
	ErrGoalRejected = newRoszError(ErrorCodeActionGoalRejected, "goal rejected")
	ErrResultFailed = newRoszError(ErrorCodeActionResultFailed, "action result failed")
	ErrCancelFailed = newRoszError(ErrorCodeActionCancelFailed, "action cancel failed")
	// ErrBuildFailed is returned when a builder's Build() call fails (FFI returned null pointer).
	// Use errors.Is(err, rosz.ErrBuildFailed) to detect construction failures.
	ErrBuildFailed = newRoszError(ErrorCodeBuildFailed, "failed to build entity")
	// ErrCloseFailed is returned when a Close() call fails.
	// Use errors.Is(err, rosz.ErrCloseFailed) to detect close failures.
	ErrCloseFailed = newRoszError(ErrorCodeCloseFailed, "close failed")
)
