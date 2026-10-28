package hiroz

import "sync"

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
	rawHandler, template, err := typedMessageCallback(handler)
	if err != nil {
		return nil, err
	}
	return builder.BuildWithCallback(template, rawHandler)
}

// SubscriberWithChannel creates a subscriber that delivers to a channel with automatic deserialization.
// Returns the subscriber, a receive channel, and a cleanup function.
// Closing the subscriber also closes the channel. Cleanup may be called repeatedly
// or concurrently with delivery; it unblocks pending sends.
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
	return SubscriberWithHandler(builder, NewFifoChannel[T](bufferSize))
}

// SubscriberWithHandler integrates the Handler interface directly with the Subscriber.
// Returns the subscriber, receive channel, and a cleanup function.
// Closing the subscriber also runs cleanup. Cleanup is idempotent. Custom handlers
// must allow their drop function to run concurrently with callbacks.
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
	callback, drop, ch := handler.ToCbDropHandler()
	// Make cleanup idempotent even for a custom Handler.
	cleanup := sync.OnceFunc(func() {
		if drop != nil {
			drop()
		}
	})
	rawCallback, template, err := typedMessageCallback[T](callback)
	if err != nil {
		cleanup()
		return nil, nil, nil, err
	}
	sub, err := builder.buildWithCallback(template, rawCallback, cleanup)
	if err != nil {
		cleanup()
		return nil, nil, nil, err
	}
	return sub, ch, cleanup, nil
}
