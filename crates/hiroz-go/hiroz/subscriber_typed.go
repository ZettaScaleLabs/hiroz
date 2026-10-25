package hiroz

import "reflect"

// newMessage returns a usable value of T. For a pointer type such as *std_msgs.String it
// allocates the pointee, because the zero value is a nil pointer and DeserializeCDR on a nil
// receiver panics. For any other type the zero value is already usable.
func newMessage[T Message]() T {
	t := reflect.TypeOf((*T)(nil)).Elem()
	if t.Kind() == reflect.Pointer {
		return reflect.New(t.Elem()).Interface().(T)
	}
	var zero T
	return zero
}

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
	msgTemplate := newMessage[T]()

	// Wrap the typed handler with deserialization
	rawHandler := func(data []byte) {
		// Create a new instance of the message type
		msg := newMessage[T]()
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
	msgTemplate := newMessage[T]()

	// Create output channel and a done channel used to signal teardown.
	// The done channel eliminates the TOCTOU window that existed with the
	// old atomic.Bool guard: between Load()→false and the channel send,
	// cleanup() could close outCh causing a panic. The select below races
	// the send against <-done so the callback exits cleanly after cleanup.
	outCh := make(chan T, bufferSize)
	done := make(chan struct{})

	rawHandler := func(data []byte) {
		msg := newMessage[T]()
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
	msgTemplate := newMessage[T]()

	callback, drop, ch := handler.ToCbDropHandler()

	// Wrap callback with deserialization
	rawCallback := func(data []byte) {
		msg := newMessage[T]()
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
