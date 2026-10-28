package hiroz

/*
#include <stdlib.h>
#include "hiroz_ffi.h"

extern hiroz_MessageCallback getSubscriberCallback();
extern hiroz_SubscriberDropCallback getSubscriberDropCallback();
*/
import "C"
import (
	"fmt"
	"runtime"
	"runtime/cgo"
	"sync"
	"unsafe"
)

// MessageHandler is a callback function for received messages
type MessageHandler func(data []byte)

// Subscriber subscribes to messages on a topic
type Subscriber struct {
	handle    *C.hiroz_subscriber_t
	node      *Node
	closeOnce sync.Once
	mu        sync.RWMutex
	onClose   func()
	closeErr  error
}

// SubscriberBuilder builds a Subscriber
type SubscriberBuilder struct {
	node  *Node
	topic string
	qos   *QosProfile
}

// WithQoS sets the QoS profile for the subscriber
func (b *SubscriberBuilder) WithQoS(qos QosProfile) *SubscriberBuilder {
	b.qos = &qos
	return b
}

// BuildWithCallback creates the subscriber with a custom callback handler.
// The handler is invoked on a C/Rust thread — avoid long blocking operations.
func (b *SubscriberBuilder) BuildWithCallback(msg Message, handler MessageHandler) (*Subscriber, error) {
	return b.buildWithCallback(msg, handler, nil)
}

func (b *SubscriberBuilder) buildWithCallback(msg Message, handler MessageHandler, onClose func()) (*Subscriber, error) {
	node, release, err := b.node.acquire()
	if err != nil {
		return nil, err
	}
	defer release()
	if handler == nil {
		return nil, fmt.Errorf("callback handler cannot be nil")
	}

	topicC := C.CString(b.topic)
	defer C.free(unsafe.Pointer(topicC))

	typeNameC := C.CString(msg.TypeName())
	defer C.free(unsafe.Pointer(typeNameC))

	typeHashC := C.CString(msg.TypeHash())
	defer C.free(unsafe.Pointer(typeHashC))

	// Create closure with the user's callback
	// The closure will be dropped by Rust when the subscriber is destroyed
	closure := newClosure(handler, nil)

	var qos *C.hiroz_qos_profile_t
	if b.qos != nil {
		cQos := b.qos.toCQos()
		qos = &cQos
	}
	// Rust owns closure from this call onward, including on creation failure.
	handle := C.hiroz_subscriber_create_owned(
		node, topicC, typeNameC, typeHashC,
		C.getSubscriberCallback(), closure.selfHandle, qos,
		C.getSubscriberDropCallback(),
	)

	if handle == nil {
		return nil, fmt.Errorf("%w: subscriber for %s", ErrBuildFailed, b.topic)
	}

	sub := &Subscriber{
		handle:  handle,
		node:    b.node,
		onClose: onClose,
	}
	runtime.SetFinalizer(sub, (*Subscriber).Close)

	// Store in node so the subscription lives until the node is closed,
	// even if the caller discards the returned *Subscriber.
	if err := b.node.ownSubscriber(sub); err != nil {
		_ = sub.Close()
		return nil, err
	}

	return sub, nil
}

// acquire keeps the native handle alive for a synchronous operation.
func (s *Subscriber) acquire() (*C.hiroz_subscriber_t, func(), error) {
	s.mu.RLock()
	if s.handle == nil {
		s.mu.RUnlock()
		return nil, nil, fmt.Errorf("subscriber is closed")
	}
	return s.handle, s.mu.RUnlock, nil
}

// Close undeclares the subscriber and prevents further delivery. A callback that
// is already running may finish after Close returns; its resources stay alive
// until it returns. Close may be called from a callback.
func (s *Subscriber) Close() error {
	s.closeOnce.Do(func() {
		if s.onClose != nil {
			s.onClose()
		}
		s.mu.Lock()
		if s.handle != nil {
			result := C.hiroz_subscriber_destroy(s.handle)
			s.handle = nil
			if result != 0 {
				s.closeErr = fmt.Errorf("subscriber close failed (rc=%d): %w", result, ErrCloseFailed)
			}
		}
		s.mu.Unlock()
		runtime.SetFinalizer(s, nil)
		if s.node != nil {
			s.node.subsMu.Lock()
			for i, sub := range s.node.ownedSubs {
				if sub == s {
					last := len(s.node.ownedSubs) - 1
					s.node.ownedSubs[i] = s.node.ownedSubs[last]
					s.node.ownedSubs[last] = nil
					s.node.ownedSubs = s.node.ownedSubs[:last]
					break
				}
			}
			s.node.subsMu.Unlock()
		}
	})
	return s.closeErr
}

//export goSubscriberDropCallback
func goSubscriberDropCallback(userData C.uintptr_t) {
	_ = safeCall(func() error {
		cgo.Handle(userData).Value().(*closureContext[[]byte]).drop()
		return nil
	})
}

//export goSubscriberCallback
func goSubscriberCallback(userData C.uintptr_t, data *C.uint8_t, length C.size_t) {
	// Cast userData back to closureContext pointer
	closure := cgo.Handle(userData).Value().(*closureContext[[]byte])

	// Copy data to Go slice
	goData := C.GoBytes(unsafe.Pointer(data), C.int(length))

	logger.Debug("goSubscriberCallback", "len", int(length))

	// safeCall prevents a user-callback panic from crossing the CGo boundary (UB).
	_ = safeCall(func() error {
		closure.call(goData)
		return nil
	})
}
