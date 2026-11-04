package hiroz

import "testing"

// A sample can reach the Go callback after a failed build has released the callback state.
// The exported entrypoint must return without a panic: a panic in a callback from C ends the process.
func TestSubscriberCallbackIgnoresAReleasedContext(t *testing.T) {
	calls := 0
	closure := newClosure(func([]byte) { calls++ }, nil)
	saved := closure.selfHandle

	goSubscriberCallback(saved, nil, 0)
	if calls != 1 {
		t.Fatalf("a live context must reach its callback once, got %d calls", calls)
	}

	closure.drop()

	goSubscriberCallback(saved, nil, 0)
	if calls != 1 {
		t.Errorf("a released context must not reach its callback, got %d calls", calls)
	}
}
