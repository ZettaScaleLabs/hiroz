package hiroz

/*
#include <stdlib.h>
#include "hiroz_ffi.h"

extern hiroz_MessageCallback getSubscriberCallback();
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
}

// BufferGroup selects whether a subscriber joins the CPU buffer group of rmw_zenoh.
//
// A subscriber that joins announces `backends:cpu:` and receives from both the base key and
// the group key. A publisher that sends large byte buffers on the group key only reaches a
// subscriber that joined. A Go subscriber has no message definition, so BufferGroupCpu and
// BufferGroupAuto join the same way. The values are the C ABI of
// hiroz_subscriber_create_with_group.
type BufferGroup int32

const (
	// BufferGroupOff keeps the subscriber out of the group.
	BufferGroupOff BufferGroup = 0
	// BufferGroupCpu joins the CPU buffer group.
	BufferGroupCpu BufferGroup = 1
	// BufferGroupAuto joins the CPU buffer group. It is the default.
	BufferGroupAuto BufferGroup = 2
)

// cSubscriber is the C subscriber type. Tests name it to return a handle from createSubscriber.
type cSubscriber = C.hiroz_subscriber_t

// subscriberCall holds the arguments of one call to hiroz_subscriber_create_with_group.
type subscriberCall struct {
	node     *C.hiroz_node_t
	topic    *C.char
	typeName *C.char
	typeHash *C.char
	callback C.hiroz_MessageCallback
	userData C.uintptr_t
	qos      *C.hiroz_qos_profile_t
	group    int32
}

// createSubscriber makes the one C call that creates a subscriber. A test replaces it to see
// the arguments that reach the C layer. Production code never assigns it.
var createSubscriber = func(c subscriberCall) *cSubscriber {
	return C.hiroz_subscriber_create_with_group(
		c.node,
		c.topic,
		c.typeName,
		c.typeHash,
		c.callback,
		c.userData,
		c.qos,
		C.int32_t(c.group),
	)
}

// SubscriberBuilder builds a Subscriber
type SubscriberBuilder struct {
	node        *Node
	topic       string
	qos         *QosProfile
	bufferGroup BufferGroup
}

// WithQoS sets the QoS profile for the subscriber
func (b *SubscriberBuilder) WithQoS(qos QosProfile) *SubscriberBuilder {
	b.qos = &qos
	return b
}

// WithBufferGroup sets the buffer group of the subscriber. The default is BufferGroupAuto.
// Pass BufferGroupOff to keep the subscriber out of the group.
//
// A QoS that rmw_zenoh does not serve on the group key (transient-local) stays out of the
// group whatever the value is.
func (b *SubscriberBuilder) WithBufferGroup(group BufferGroup) *SubscriberBuilder {
	b.bufferGroup = group
	return b
}

// BuildWithCallback creates the subscriber with a custom callback handler.
// The handler is invoked on a C/Rust thread — avoid long blocking operations.
func (b *SubscriberBuilder) BuildWithCallback(msg Message, handler MessageHandler) (*Subscriber, error) {
	if handler == nil {
		return nil, fmt.Errorf("callback handler cannot be nil")
	}

	// Reject an unknown group before any callback state exists.
	if b.bufferGroup != BufferGroupOff && b.bufferGroup != BufferGroupCpu && b.bufferGroup != BufferGroupAuto {
		return nil, fmt.Errorf("%w: subscriber for %s: unknown buffer group %d", ErrBuildFailed, b.topic, int32(b.bufferGroup))
	}

	pinner := &runtime.Pinner{}
	defer pinner.Unpin()

	topicC := C.CString(b.topic)
	defer C.free(unsafe.Pointer(topicC))

	typeNameC := C.CString(msg.TypeName())
	defer C.free(unsafe.Pointer(typeNameC))

	typeHashC := C.CString(msg.TypeHash())
	defer C.free(unsafe.Pointer(typeHashC))

	// Create the closure with the user's callback. After a successful build the closure stays
	// alive with the subscriber. A failed build creates no subscriber, so nothing can call it.
	closure := newClosure(handler, nil)

	// A nil QoS pointer means the default QoS.
	var cQos C.hiroz_qos_profile_t
	var cQosPtr *C.hiroz_qos_profile_t
	if b.qos != nil {
		cQos = b.qos.toCQos()
		cQosPtr = &cQos
	}

	handle := createSubscriber(subscriberCall{
		node:     b.node.handle,
		topic:    topicC,
		typeName: typeNameC,
		typeHash: typeHashC,
		callback: C.getSubscriberCallback(),
		userData: closure.selfHandle,
		qos:      cQosPtr,
		group:    int32(b.bufferGroup),
	})

	if handle == nil {
		closure.drop()
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
		result := C.hiroz_subscriber_destroy(s.handle)
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
