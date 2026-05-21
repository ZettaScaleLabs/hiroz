package rosz

/*
#include <stdlib.h>
#include "ros_z_ffi.h"

extern ros_z_MessageCallback getSubscriberCallback();
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
	handle    *C.ros_z_subscriber_t
	node      *Node
	closeOnce sync.Once
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
	if handler == nil {
		return nil, fmt.Errorf("callback handler cannot be nil")
	}

	pinner := &runtime.Pinner{}
	defer pinner.Unpin()

	topicC := C.CString(b.topic)
	defer C.free(unsafe.Pointer(topicC))

	typeNameC := C.CString(msg.TypeName())
	defer C.free(unsafe.Pointer(typeNameC))

	typeHashC := C.CString(msg.TypeHash())
	defer C.free(unsafe.Pointer(typeHashC))

	// Create closure with the user's callback
	// The closure will be dropped by Rust when the subscriber is destroyed
	closure := newClosure(handler, nil)

	var handle *C.ros_z_subscriber_t
	if b.qos != nil {
		cQos := b.qos.toCQos()
		handle = C.ros_z_subscriber_create_with_qos(
			b.node.handle,
			topicC,
			typeNameC,
			typeHashC,
			C.getSubscriberCallback(),
			closure.selfHandle,
			&cQos,
		)
	} else {
		handle = C.ros_z_subscriber_create(
			b.node.handle,
			topicC,
			typeNameC,
			typeHashC,
			C.getSubscriberCallback(),
			closure.selfHandle,
		)
	}

	if handle == nil {
		return nil, fmt.Errorf("%w: subscriber for %s", ErrBuildFailed, b.topic)
	}

	sub := &Subscriber{
		handle: handle,
		node:   b.node,
	}
	runtime.SetFinalizer(sub, (*Subscriber).Close)

	// Store in node so the subscription lives until the node is closed,
	// even if the caller discards the returned *Subscriber.
	b.node.subsMu.Lock()
	b.node.ownedSubs = append(b.node.ownedSubs, sub)
	b.node.subsMu.Unlock()

	return sub, nil
}

// Close destroys the subscriber
func (s *Subscriber) Close() error {
	var err error
	s.closeOnce.Do(func() {
		if s.handle == nil {
			return
		}
		result := C.ros_z_subscriber_destroy(s.handle)
		s.handle = nil
		if result != 0 {
			err = fmt.Errorf("subscriber close failed (rc=%d): %w", result, ErrCloseFailed)
		}
	})
	return err
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
