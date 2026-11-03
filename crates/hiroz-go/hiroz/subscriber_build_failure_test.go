package hiroz

import (
	"runtime/cgo"
	"testing"
)

// handleIsValid reports whether h still names a value. A deleted cgo.Handle panics on Value.
func handleIsValid(h cgo.Handle) (valid bool) {
	defer func() {
		if recover() != nil {
			valid = false
		}
	}()
	h.Value()
	return true
}

// A build that the C layer rejects must delete every handle of its callback state.
// The replaced C creator returns null, as it does when the node has no session. It also
// records the handles of the closure that the build passed to it.
func TestFailedSubscriberBuildReleasesCallbackHandles(t *testing.T) {
	var handles map[string]cgo.Handle
	original := createSubscriber
	createSubscriber = func(c subscriberCall) *cSubscriber {
		closure := cgo.Handle(uintptr(c.userData)).Value().(*closureContext[[]byte])
		handles = map[string]cgo.Handle{
			"onCall": cgo.Handle(closure.onCall),
			"self":   cgo.Handle(closure.selfHandle),
			"pinner": cgo.Handle(closure.pinner),
		}
		for name, h := range handles {
			if !handleIsValid(h) {
				t.Errorf("handle %s must be valid while the build runs", name)
			}
		}
		return nil
	}
	t.Cleanup(func() { createSubscriber = original })

	sub, err := (&Node{}).CreateSubscriber("chatter").
		BuildWithCallback(&MockMessage{typeName: "std_msgs/msg/String", typeHash: "RIHS01_build_failure"}, func([]byte) {})
	if err == nil || sub != nil {
		t.Fatalf("want an error and no subscriber, got sub=%v err=%v", sub, err)
	}
	if handles == nil {
		t.Fatal("the C creator was not called")
	}
	for name, h := range handles {
		if handleIsValid(h) {
			t.Errorf("handle %s is still valid after the failed build", name)
		}
	}
}

// An unknown group fails before the build creates any callback state.
func TestUnknownGroupBuildCreatesNoCallbackState(t *testing.T) {
	called := false
	original := createSubscriber
	createSubscriber = func(subscriberCall) *cSubscriber {
		called = true
		return nil
	}
	t.Cleanup(func() { createSubscriber = original })

	for _, group := range []BufferGroup{3, -1} {
		sub, err := (&Node{}).CreateSubscriber("chatter").
			WithBufferGroup(group).
			BuildWithCallback(&MockMessage{typeName: "std_msgs/msg/String", typeHash: "RIHS01_build_failure"}, func([]byte) {})
		if err == nil || sub != nil {
			t.Errorf("group %d: want an error and no subscriber, got sub=%v err=%v", group, sub, err)
		}
	}
	if called {
		t.Error("the C creator must not be called for an unknown group")
	}
}
