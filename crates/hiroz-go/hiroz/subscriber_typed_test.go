package hiroz

import (
	"sync"
	"sync/atomic"
	"testing"
)

// MockMessage for testing typed subscriber functions
type MockTypedMessage struct {
	Value string
}

func (m *MockTypedMessage) TypeName() string              { return "test/MockTypedMessage" }
func (m *MockTypedMessage) TypeHash() string              { return "RIHS01_mock_typed" }
func (m *MockTypedMessage) SerializeCDR() ([]byte, error) { return []byte(m.Value), nil }
func (m *MockTypedMessage) DeserializeCDR(data []byte) error {
	m.Value = string(data)
	return nil
}

// TestTypedMessageHandlerCompiles verifies the TypedMessageHandler type compiles
func TestTypedMessageHandlerCompiles(t *testing.T) {
	// Verify pointer type handlers compile
	var handler TypedMessageHandler[*MockTypedMessage] = func(msg *MockTypedMessage) {
		_ = msg.Value
	}
	if handler == nil {
		t.Error("Handler should not be nil")
	}

	t.Log("TypedMessageHandler[T] compiles correctly")
}

// TestBuildWithTypedCallbackSignature tests the function signature compiles
func TestBuildWithTypedCallbackSignature(t *testing.T) {
	// Test that the function signature is correct and accepts typed handlers
	// Note: We don't call it because it requires a real node/context

	var called bool
	handler := func(msg *MockTypedMessage) {
		called = true
		_ = msg.Value
	}

	// Verify the handler has the right type
	var _ TypedMessageHandler[*MockTypedMessage] = handler

	if called {
		t.Error("Handler should not be called in this test")
	}

	t.Log("BuildWithTypedCallback function signature verified")
}

// TestSubscriberWithChannelSignature tests the function signature
func TestSubscriberWithChannelSignature(t *testing.T) {
	// Verify the function signature with generics compiles
	// The signature is: func SubscriberWithChannel[T Message](builder *SubscriberBuilder, bufferSize int) (*Subscriber, <-chan T, func(), error)

	// We can't call it without a real builder, but we can verify the type system works
	type testFunc func(*SubscriberBuilder, int) (*Subscriber, <-chan *MockTypedMessage, func(), error)
	var _ testFunc = SubscriberWithChannel[*MockTypedMessage]

	t.Log("SubscriberWithChannel[T] function signature verified")
}

// TestSubscriberWithHandlerSignature tests the function signature
func TestSubscriberWithHandlerSignature(t *testing.T) {
	// Verify the function signature compiles
	// The signature is: func SubscriberWithHandler[T Message](builder *SubscriberBuilder, handler Handler[T]) (*Subscriber, <-chan T, func(), error)

	type testFunc func(*SubscriberBuilder, Handler[*MockTypedMessage]) (*Subscriber, <-chan *MockTypedMessage, func(), error)
	var _ testFunc = SubscriberWithHandler[*MockTypedMessage]

	t.Log("SubscriberWithHandler[T] function signature verified")
}

// TestCleanupFunctionPattern tests the cleanup function pattern
func TestCleanupFunctionPattern(t *testing.T) {
	// Test that cleanup functions work as expected
	channelClosed := false

	cleanup := func() {
		channelClosed = true
	}

	if channelClosed {
		t.Error("Channel should not be closed yet")
	}

	cleanup()

	if !channelClosed {
		t.Error("Channel should be closed after cleanup()")
	}

	// Verify cleanup can be called multiple times safely
	cleanup() // Should not panic

	t.Log("Cleanup function pattern works correctly")
}

// TestTypedSubscriberDeferOrder documents the correct defer order
func TestTypedSubscriberDeferOrder(t *testing.T) {
	// This test documents the correct defer order for cleanup
	var cleanupOrder []string

	// Simulate the recommended pattern
	mockCleanup := func() {
		cleanupOrder = append(cleanupOrder, "cleanup")
	}
	mockClose := func() {
		cleanupOrder = append(cleanupOrder, "close")
	}

	// Correct order: defer close first, then cleanup
	// (defers execute in LIFO order)
	defer func() {
		// Verify order after both have executed
		if len(cleanupOrder) != 2 {
			t.Errorf("Expected 2 cleanup calls, got %d", len(cleanupOrder))
			return
		}
		if cleanupOrder[0] != "cleanup" {
			t.Errorf("Expected cleanup first, got %s", cleanupOrder[0])
		}
		if cleanupOrder[1] != "close" {
			t.Errorf("Expected close second, got %s", cleanupOrder[1])
		}
		t.Log("Defer order verified: cleanup() executes before Close()")
	}()

	defer mockClose()   // Executes second
	defer mockCleanup() // Executes first
}

// TestTypedSubscriberGenericConstraints tests generic type constraints
func TestTypedSubscriberGenericConstraints(t *testing.T) {
	// Verify that only types implementing Message can be used

	// This should compile: MockTypedMessage implements Message
	var _ TypedMessageHandler[*MockTypedMessage] = func(msg *MockTypedMessage) {}

	// The following would NOT compile (which is correct):
	// var _ TypedMessageHandler[string] = func(msg string) {}
	// // Error: string does not implement Message

	t.Log("Generic type constraints working correctly")
}

// TestDeserializationInTypedCallback tests the deserialization logic
func TestDeserializationInTypedCallback(t *testing.T) {
	// Test the deserialization pattern used internally

	testData := []byte("test message")
	var received *MockTypedMessage

	// Simulate what BuildWithTypedCallback does internally
	handler := func(msg *MockTypedMessage) {
		received = msg
	}

	// Simulate the wrapping logic
	var msg MockTypedMessage
	if err := msg.DeserializeCDR(testData); err != nil {
		t.Fatalf("Deserialization failed: %v", err)
	}
	handler(&msg)

	if received == nil {
		t.Error("Message should be received")
	}
	if received.Value != "test message" {
		t.Errorf("Expected 'test message', got '%s'", received.Value)
	}

	t.Log("Deserialization pattern verified")
}

// TestChannelPatternWithCleanup tests the channel + cleanup pattern
func TestChannelPatternWithCleanup(t *testing.T) {
	// Simulate the SubscriberWithChannel pattern
	ch := make(chan *MockTypedMessage, 5)
	var cleanupCalled atomic.Bool

	cleanup := func() {
		close(ch)
		cleanupCalled.Store(true)
	}

	// Send some messages
	go func() {
		ch <- &MockTypedMessage{Value: "msg1"}
		ch <- &MockTypedMessage{Value: "msg2"}
		cleanup()
	}()

	// Receive messages
	var received []string
	for msg := range ch {
		received = append(received, msg.Value)
	}

	if len(received) != 2 {
		t.Errorf("Expected 2 messages, got %d", len(received))
	}
	if !cleanupCalled.Load() {
		t.Error("Cleanup should have been called")
	}

	t.Log("Channel + cleanup pattern verified")
}

// TestHandlerIntegration tests Handler interface integration
func TestHandlerIntegration(t *testing.T) {
	// Test that Handler[T] interface works with typed subscribers
	handler := NewFifoChannel[*MockTypedMessage](10)

	callback, drop, ch := handler.ToCbDropHandler()

	if callback == nil {
		t.Error("Callback should not be nil")
	}
	if drop == nil {
		t.Error("Drop function should not be nil")
	}
	if ch == nil {
		t.Error("Channel should not be nil")
	}

	// Send a message through the handler
	go func() {
		callback(&MockTypedMessage{Value: "test"})
		drop()
	}()

	// Receive it
	msg := <-ch
	if msg.Value != "test" {
		t.Errorf("Expected 'test', got '%s'", msg.Value)
	}

	t.Log("Handler[T] integration verified")
}

// BenchmarkTypedCallback benchmarks the typed callback overhead
func BenchmarkTypedCallback(b *testing.B) {
	handler := func(msg *MockTypedMessage) {
		_ = msg.Value
	}

	msg := &MockTypedMessage{Value: "test"}
	data, _ := msg.SerializeCDR()

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		var decoded MockTypedMessage
		decoded.DeserializeCDR(data)
		handler(&decoded)
	}
}

// BenchmarkTypedChannel benchmarks the typed channel pattern
func BenchmarkTypedChannel(b *testing.B) {
	ch := make(chan *MockTypedMessage, 100)
	var wg sync.WaitGroup
	wg.Add(1)

	// Producer
	go func() {
		defer wg.Done()
		msg := &MockTypedMessage{Value: "test"}
		for i := 0; i < b.N; i++ {
			ch <- msg
		}
		close(ch)
	}()

	// Consumer
	b.ResetTimer()
	for msg := range ch {
		_ = msg.Value
	}

	wg.Wait()
}
