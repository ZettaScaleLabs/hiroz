package rosz

import "sync"

// Handler provides a unified interface for callback and channel-based message handling.
// Implementations can use direct callbacks (Closure) or channels (FifoChannel, RingChannel).
type Handler[T any] interface {
	// ToCbDropHandler returns the callback function, optional drop function, and receive channel.
	// For callback-based handlers, the channel is nil.
	// For channel-based handlers, the callback sends to the channel.
	ToCbDropHandler() (callback func(T), drop func(), receiver <-chan T)
}

// Closure wraps a direct callback function for message handling.
// This is the default handler type used by BuildWithCallback().
type Closure[T any] struct {
	call func(T)
	drop func()
}

// ToCbDropHandler returns the callback and drop functions with no channel.
func (c *Closure[T]) ToCbDropHandler() (func(T), func(), <-chan T) {
	return c.call, c.drop, nil
}

// NewClosure creates a callback-based handler.
func NewClosure[T any](call func(T), drop func()) *Closure[T] {
	return &Closure[T]{call: call, drop: drop}
}

// FifoChannel delivers messages to a buffered channel.
// When the channel is full, Send blocks until space is available.
type FifoChannel[T any] struct {
	channel chan T
}

// ToCbDropHandler returns a callback that sends to the channel.
func (f *FifoChannel[T]) ToCbDropHandler() (func(T), func(), <-chan T) {
	callback := func(msg T) {
		f.channel <- msg
	}
	drop := func() {
		close(f.channel)
	}
	return callback, drop, f.channel
}

// NewFifoChannel creates a channel-based handler with the specified buffer size.
// A buffer size of 0 creates an unbuffered channel (synchronous).
func NewFifoChannel[T any](bufferSize int) *FifoChannel[T] {
	return &FifoChannel[T]{
		channel: make(chan T, bufferSize),
	}
}

// RingChannel delivers messages to a channel with ring buffer semantics.
// When the channel is full, the oldest message is dropped to make room for the new one.
type RingChannel[T any] struct {
	channel chan T
	mu      sync.Mutex
	closed  bool
}

// ToCbDropHandler returns a callback that sends to the channel with ring buffer behavior.
//
// The returned drop function is safe to call concurrently with an in-flight
// callback: both acquire r.mu, so drop cannot close the channel while a
// callback is mid-send.
func (r *RingChannel[T]) ToCbDropHandler() (func(T), func(), <-chan T) {
	callback := func(msg T) {
		r.mu.Lock()
		defer r.mu.Unlock()
		if r.closed {
			return
		}
		select {
		case r.channel <- msg:
		default:
			// Channel full — drop oldest message and retry
			<-r.channel
			r.channel <- msg
		}
	}
	drop := func() {
		r.mu.Lock()
		defer r.mu.Unlock()
		if !r.closed {
			r.closed = true
			close(r.channel)
		}
	}
	return callback, drop, r.channel
}

// NewRingChannel creates a ring buffer channel handler with the specified capacity.
// The capacity must be greater than 0.
func NewRingChannel[T any](capacity int) *RingChannel[T] {
	if capacity <= 0 {
		panic("ring channel capacity must be > 0")
	}
	return &RingChannel[T]{
		channel: make(chan T, capacity),
	}
}
