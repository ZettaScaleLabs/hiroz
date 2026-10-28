package hiroz

import (
	"fmt"
	"reflect"
)

// messageFactory allocates a fresh receiver for every incoming message. Generated
// messages implement Message on pointers, whose generic zero value is nil.
func messageFactory[T Message]() (func() T, error) {
	typ := reflect.TypeFor[T]()
	if typ.Kind() == reflect.Interface {
		return nil, fmt.Errorf("typed messages require a concrete message type, got %v", typ)
	}
	if typ.Kind() == reflect.Pointer {
		return func() T { return reflect.New(typ.Elem()).Interface().(T) }, nil
	}
	return func() T { return reflect.Zero(typ).Interface().(T) }, nil
}

// typedMessageCallback is shared by all typed subscription entry points.
func typedMessageCallback[T Message](handler TypedMessageHandler[T]) (MessageHandler, T, error) {
	var zero T
	if handler == nil {
		return nil, zero, fmt.Errorf("callback handler cannot be nil")
	}
	allocate, err := messageFactory[T]()
	if err != nil {
		return nil, zero, err
	}
	callback := func(data []byte) {
		msg := allocate()
		if err := msg.DeserializeCDR(data); err != nil {
			return
		}
		handler(msg)
	}
	return callback, allocate(), nil
}
