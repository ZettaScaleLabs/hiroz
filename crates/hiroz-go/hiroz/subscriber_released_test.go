package hiroz

import "testing"

// A sample can reach the Go callback after a failed build has released the callback state.
// The lookup must report that the state is gone and must not panic: a panic in a callback
// from C ends the process.
func TestSubscriberClosureReportsAReleasedContext(t *testing.T) {
	closure := newClosure(func([]byte) {}, nil)
	userData := uintptr(closure.selfHandle)

	if got, ok := subscriberClosure(userData); !ok || got != closure {
		t.Fatalf("a live context must be found, got ok=%v", ok)
	}

	closure.drop()

	if got, ok := subscriberClosure(userData); ok || got != nil {
		t.Errorf("a released context must not be found, got ok=%v", ok)
	}
}
