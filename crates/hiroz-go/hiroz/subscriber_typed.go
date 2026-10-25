package hiroz

import (
	"context"
	"sync"
)

// TypedMessageHandler is a callback for strongly-typed messages
type TypedMessageHandler[T Message] func(msg T)

// BuildWithTypedCallback creates a subscriber with automatic deserialization.
// The callback receives the already-deserialized message.
//
// Example:
//
//	sub, err := BuildWithTypedCallback(node.CreateSubscriber("chatter"),
//	    func(msg *std_msgs.String) {
//	        log.Printf("Received: %s", msg.Data)
//	    })
func BuildWithTypedCallback[T Message](builder *SubscriberBuilder, handler TypedMessageHandler[T]) (*Subscriber, error) {
	// Create a zero-value instance for type information
	var msgTemplate T

	// Wrap the typed handler with deserialization
	rawHandler := func(data []byte) {
		// Create a new instance of the message type
		var msg T
		if err := msg.DeserializeCDR(data); err != nil {
			// Silently drop malformed messages
			return
		}
		handler(msg)
	}

	return builder.BuildWithCallback(msgTemplate, rawHandler)
}

// SubscriberWithChannel creates a subscriber that delivers to a channel with automatic deserialization.
// Returns the subscriber, a receive channel, and a cleanup function.
// Call the cleanup function to close the channel when done.
//
// Example:
//
//	sub, ch, cleanup, err := SubscriberWithChannel[*std_msgs.String](
//	    node.CreateSubscriber("chatter"), 10)
//	defer cleanup()
//	defer sub.Close()
//
//	for msg := range ch {
//	    log.Printf("Received: %s", msg.Data)
//	}
func SubscriberWithChannel[T Message](builder *SubscriberBuilder, bufferSize int) (*Subscriber, <-chan T, func(), error) {
	var msgTemplate T

	// Create output channel and a done channel used to signal teardown.
	// The done channel eliminates the TOCTOU window that existed with the
	// old atomic.Bool guard: between Load()→false and the channel send,
	// cleanup() could close outCh causing a panic. The select below races
	// the send against <-done so the callback exits cleanly after cleanup.
	outCh := make(chan T, bufferSize)
	done := make(chan struct{})

	rawHandler := func(data []byte) {
		var msg T
		if err := msg.DeserializeCDR(data); err != nil {
			return // drop malformed messages
		}
		select {
		case outCh <- msg:
		case <-done:
		}
	}

	sub, err := builder.BuildWithCallback(msgTemplate, rawHandler)
	if err != nil {
		close(done)
		close(outCh)
		return nil, nil, nil, err
	}

	// cleanup closes done first (unblocks any in-flight send), then outCh
	// (signals range-loop consumers that the stream is finished).
	cleanup := func() {
		close(done)
		close(outCh)
	}

	return sub, outCh, cleanup, nil
}

// SubscriberWithHandler integrates the Handler interface directly with the Subscriber.
// Returns the subscriber, receive channel, and a cleanup function.
// Call cleanup when done to close the handler.
//
// Example:
//
//	handler := hiroz.NewFifoChannel[*std_msgs.String](10)
//	sub, ch, cleanup, err := SubscriberWithHandler(
//	    node.CreateSubscriber("chatter"), handler)
//	defer cleanup()
//	defer sub.Close()
//
//	for msg := range ch {
//	    log.Printf("Received: %s", msg.Data)
//	}
func SubscriberWithHandler[T Message](builder *SubscriberBuilder, handler Handler[T]) (*Subscriber, <-chan T, func(), error) {
	var msgTemplate T

	callback, drop, ch := handler.ToCbDropHandler()

	// Wrap callback with deserialization
	rawCallback := func(data []byte) {
		var msg T
		if err := msg.DeserializeCDR(data); err != nil {
			return
		}
		callback(msg)
	}

	sub, err := builder.BuildWithCallback(msgTemplate, rawCallback)
	if err != nil {
		if drop != nil {
			drop()
		}
		return nil, nil, nil, err
	}

	return sub, ch, drop, nil
}

// BuildWithTypedCallbackContext is BuildWithTypedCallback with a lifetime bound to ctx.
// When ctx ends, the subscriber is destroyed through Node.DestroySubscriber and no further
// callbacks run. If ctx has already ended, the call returns ctx.Err() and creates no
// subscriber. Closing the subscriber earlier is safe.
//
// Example:
//
//	sub, err := hiroz.BuildWithTypedCallbackContext(ctx, node.CreateSubscriber("chatter"),
//	    func(msg *std_msgs.String) { log.Printf("Received: %s", msg.Data) })
func BuildWithTypedCallbackContext[T Message](ctx context.Context, builder *SubscriberBuilder, handler TypedMessageHandler[T]) (*Subscriber, error) {
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	sub, err := BuildWithTypedCallback(builder, handler)
	if err != nil {
		return nil, err
	}
	context.AfterFunc(ctx, func() { _ = sub.node.DestroySubscriber(sub) })
	return sub, nil
}

// SubscriberWithChannelContext is SubscriberWithChannel with a lifetime bound to ctx, and no
// cleanup function. When ctx ends, the subscriber is destroyed and then the channel is
// closed, so a range loop over the channel finishes. If ctx has already ended, the call
// returns ctx.Err() and creates nothing.
//
// A consumer that stops reading does not block the shutdown: a callback that waits on a
// full channel is released first. The channel stays open until ctx ends, so pass a
// context that you cancel.
//
// Example:
//
//	sub, ch, err := hiroz.SubscriberWithChannelContext[*std_msgs.String](
//	    ctx, node.CreateSubscriber("chatter"), 10)
//	for msg := range ch {
//	    log.Printf("Received: %s", msg.Data)
//	}
func SubscriberWithChannelContext[T Message](ctx context.Context, builder *SubscriberBuilder, bufferSize int) (*Subscriber, <-chan T, error) {
	if err := ctx.Err(); err != nil {
		return nil, nil, err
	}
	msgTemplate := newMessage[T]()
	outCh := make(chan T, bufferSize)
	done := make(chan struct{})
	// senders holds a read lock while a callback may send. The teardown takes the write lock
	// before it closes outCh, so no callback sends on a closed channel.
	var senders sync.RWMutex
	closed := false

	rawHandler := func(data []byte) {
		msg := newMessage[T]()
		if err := msg.DeserializeCDR(data); err != nil {
			return // drop malformed messages
		}
		senders.RLock()
		defer senders.RUnlock()
		if closed {
			return
		}
		select {
		case outCh <- msg:
		case <-done:
		}
	}

	sub, err := builder.BuildWithCallback(msgTemplate, rawHandler)
	if err != nil {
		return nil, nil, err
	}

	// Order matters: release blocked callbacks, undeclare so none can start, wait for the
	// callbacks that are still running, then close.
	context.AfterFunc(ctx, func() {
		close(done)
		_ = sub.node.DestroySubscriber(sub)
		senders.Lock()
		closed = true
		senders.Unlock()
		close(outCh)
	})
	return sub, outCh, nil
}
