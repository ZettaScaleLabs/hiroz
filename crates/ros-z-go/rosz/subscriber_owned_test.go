package rosz

import (
	"sync"
	"testing"
)

// TestNodeOwnedSubsField verifies that Node has the ownedSubs and subsMu fields
// required for callback subscriber lifetime management.
//
// Full integration test (BuildWithCallback without assigning result) requires a
// live Zenoh session and is covered by CI integration tests.
func TestNodeOwnedSubsField(t *testing.T) {
	// Verify the Node struct has the correct fields by constructing one directly.
	// This will fail to compile if ownedSubs or subsMu are removed.
	node := &Node{
		ownedSubs: nil,
		subsMu:    sync.Mutex{},
	}

	if node.ownedSubs != nil {
		t.Error("ownedSubs should be nil initially")
	}

	// Simulate what BuildWithCallback does: store a subscriber in the node.
	mockSub := &Subscriber{}

	node.subsMu.Lock()
	node.ownedSubs = append(node.ownedSubs, mockSub)
	node.subsMu.Unlock()

	if len(node.ownedSubs) != 1 {
		t.Errorf("Expected 1 owned sub, got %d", len(node.ownedSubs))
	}

	t.Log("Node.ownedSubs ownership pattern verified")
}

// TestDestroySubscriberRemovesFromOwned verifies that DestroySubscriber removes
// the subscriber from ownedSubs and closes it without requiring a live Zenoh session.
func TestDestroySubscriberRemovesFromOwned(t *testing.T) {
	node := &Node{}

	// Simulate a subscriber stored by BuildWithCallback.
	sub := &Subscriber{}
	node.subsMu.Lock()
	node.ownedSubs = append(node.ownedSubs, sub)
	node.subsMu.Unlock()

	if len(node.ownedSubs) != 1 {
		t.Fatalf("Expected 1 owned sub before destroy, got %d", len(node.ownedSubs))
	}

	// DestroySubscriber should remove sub from ownedSubs.
	// Close() on a nil-handle subscriber is a no-op.
	_ = node.DestroySubscriber(sub)

	node.subsMu.Lock()
	remaining := len(node.ownedSubs)
	node.subsMu.Unlock()

	if remaining != 0 {
		t.Errorf("Expected 0 owned subs after destroy, got %d", remaining)
	}
}

// TestDestroySubscriberUnknownIsNoOp verifies that DestroySubscriber with a
// subscriber not in ownedSubs returns nil and does not panic.
func TestDestroySubscriberUnknownIsNoOp(t *testing.T) {
	node := &Node{}
	unknown := &Subscriber{}
	err := node.DestroySubscriber(unknown)
	if err != nil {
		t.Errorf("Expected nil error for unknown subscriber, got %v", err)
	}
}

// TestClosureCallAndDrop verifies the closure infrastructure used by
// BuildWithCallback: that a closure can be called and dropped cleanly.
// Full end-to-end lifetime test (subscriber active without assignment)
// requires a live Zenoh session and is covered by CI integration tests.
func TestClosureCallAndDrop(t *testing.T) {
	// Demonstrate the pattern using the mock closure infrastructure.
	var callCount int
	handler := func(data []byte) {
		callCount++
	}

	// Simulate the BuildWithCallback closure creation (no real node needed).
	closure := newClosure(handler, nil)
	if closure == nil {
		t.Fatal("closure should not be nil")
	}

	// Simulate message delivery.
	closure.call([]byte("hello"))
	if callCount != 1 {
		t.Errorf("Expected handler called once, got %d", callCount)
	}

	// Cleanup closure (normally done by Rust on subscriber destroy).
	closure.drop()

	t.Log("Callback closure lifetime pattern verified")
}
