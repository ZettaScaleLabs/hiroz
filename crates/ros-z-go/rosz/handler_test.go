package rosz

import (
	"sync"
	"testing"
	"time"
)

func TestClosureHandler(t *testing.T) {
	var received []byte
	var dropCalled bool

	closure := NewClosure(
		func(data []byte) {
			received = data
		},
		func() {
			dropCalled = true
		},
	)

	callback, drop, ch := closure.ToCbDropHandler()

	if ch != nil {
		t.Error("Closure should not have a channel")
	}

	// Test callback
	testData := []byte{1, 2, 3}
	callback(testData)

	if string(received) != string(testData) {
		t.Errorf("received = %v, want %v", received, testData)
	}

	// Test drop
	drop()
	if !dropCalled {
		t.Error("drop function was not called")
	}
}

func TestFifoChannelHandler(t *testing.T) {
	fifo := NewFifoChannel[[]byte](10)
	callback, drop, ch := fifo.ToCbDropHandler()

	if ch == nil {
		t.Fatal("FifoChannel should have a channel")
	}

	// Test sending messages
	testData := [][]byte{{1}, {2}, {3}}
	for _, data := range testData {
		callback(data)
	}

	// Receive and verify
	for i, expected := range testData {
		select {
		case received := <-ch:
			if string(received) != string(expected) {
				t.Errorf("message %d: received = %v, want %v", i, received, expected)
			}
		case <-time.After(time.Second):
			t.Fatalf("timeout waiting for message %d", i)
		}
	}

	// Test drop closes channel
	drop()
	select {
	case _, ok := <-ch:
		if ok {
			t.Error("channel should be closed after drop")
		}
	case <-time.After(time.Second):
		t.Error("timeout waiting for channel close")
	}
}

func TestFifoChannelBlocking(t *testing.T) {
	fifo := NewFifoChannel[int](1) // Buffer size 1
	callback, _, ch := fifo.ToCbDropHandler()

	// Fill the channel
	callback(1)

	// Second send should block; check immediately (non-blocking) — if the callback
	// returned already the channel was not actually full, which is a bug.
	done := make(chan bool)
	go func() {
		callback(2) // This blocks
		done <- true
	}()

	select {
	case <-done:
		t.Error("callback should have blocked")
	default:
		// Expected - still blocked
	}

	// Consume one message to unblock
	<-ch

	// Now the goroutine should complete
	select {
	case <-done:
		// Expected
	case <-time.After(time.Second):
		t.Error("callback did not unblock")
	}
}

func TestRingChannelHandler(t *testing.T) {
	ring := NewRingChannel[int](3)
	callback, drop, ch := ring.ToCbDropHandler()

	if ch == nil {
		t.Fatal("RingChannel should have a channel")
	}

	// Fill the channel
	callback(1)
	callback(2)
	callback(3)

	// Send one more - should drop oldest (1)
	callback(4)

	// Verify we get 2, 3, 4 (not 1)
	expected := []int{2, 3, 4}
	for i, want := range expected {
		select {
		case got := <-ch:
			if got != want {
				t.Errorf("message %d: got = %d, want %d", i, got, want)
			}
		case <-time.After(time.Second):
			t.Fatalf("timeout waiting for message %d", i)
		}
	}

	// Channel should be empty now
	select {
	case unexpected := <-ch:
		t.Errorf("channel should be empty, got %d", unexpected)
	default:
		// Expected
	}

	// Test drop closes channel
	drop()
	select {
	case _, ok := <-ch:
		if ok {
			t.Error("channel should be closed after drop")
		}
	case <-time.After(time.Second):
		t.Error("timeout waiting for channel close")
	}
}

func TestRingChannelNoBlock(t *testing.T) {
	ring := NewRingChannel[int](2)
	callback, _, _ := ring.ToCbDropHandler()

	// Send many messages rapidly - none should block
	done := make(chan bool)
	go func() {
		for i := 0; i < 100; i++ {
			callback(i)
		}
		done <- true
	}()

	// Should complete quickly without blocking
	select {
	case <-done:
		// Expected
	case <-time.After(time.Second):
		t.Error("ring channel callback should not block")
	}
}

func TestRingChannelPanicOnZeroCapacity(t *testing.T) {
	defer func() {
		if r := recover(); r == nil {
			t.Error("NewRingChannel(0) should panic")
		}
	}()

	NewRingChannel[int](0)
}

func TestRingChannelPanicOnNegativeCapacity(t *testing.T) {
	defer func() {
		if r := recover(); r == nil {
			t.Error("NewRingChannel(-1) should panic")
		}
	}()

	NewRingChannel[int](-1)
}

func TestHandlerInterface(t *testing.T) {
	// Verify all types implement Handler interface
	var _ Handler[[]byte] = (*Closure[[]byte])(nil)
	var _ Handler[[]byte] = (*FifoChannel[[]byte])(nil)
	var _ Handler[[]byte] = (*RingChannel[[]byte])(nil)
}

func TestFifoChannelConcurrency(t *testing.T) {
	fifo := NewFifoChannel[int](100)
	callback, drop, ch := fifo.ToCbDropHandler()

	// Concurrent senders
	const numSenders = 5
	const msgsPerSender = 20
	var wg sync.WaitGroup

	for i := 0; i < numSenders; i++ {
		wg.Add(1)
		go func(id int) {
			defer wg.Done()
			for j := 0; j < msgsPerSender; j++ {
				callback(id*msgsPerSender + j)
			}
		}(i)
	}

	// Wait for senders
	wg.Wait()
	drop()

	// Count received messages
	count := 0
	for range ch {
		count++
	}

	expected := numSenders * msgsPerSender
	if count != expected {
		t.Errorf("received %d messages, want %d", count, expected)
	}
}
