package hiroz

/*
#include <stdlib.h>
#include "hiroz_ffi.h"
*/
import "C"
import (
	"fmt"
	"runtime"
	"sync"
	"unsafe"
)

// Publisher publishes messages to a topic
type Publisher struct {
	handle    *C.hiroz_publisher_t
	node      *Node
	topic     string
	closeOnce sync.Once
	mu        sync.Mutex
	closeErr  error
	active    int
	retired   *C.hiroz_publisher_t
}

// PublisherBuilder builds a Publisher
type PublisherBuilder struct {
	node  *Node
	topic string
	qos   *QosProfile
}

// WithQoS sets the QoS profile for the publisher
func (b *PublisherBuilder) WithQoS(qos QosProfile) *PublisherBuilder {
	b.qos = &qos
	return b
}

// Build creates the publisher for the given message type
func (b *PublisherBuilder) Build(msg Message) (*Publisher, error) {
	node, release, err := b.node.acquire()
	if err != nil {
		return nil, err
	}
	defer release()
	topicC := C.CString(b.topic)
	defer C.free(unsafe.Pointer(topicC))

	typeNameC := C.CString(msg.TypeName())
	defer C.free(unsafe.Pointer(typeNameC))

	typeHashC := C.CString(msg.TypeHash())
	defer C.free(unsafe.Pointer(typeHashC))

	var handle *C.hiroz_publisher_t
	if b.qos != nil {
		cQos := b.qos.toCQos()
		handle = C.hiroz_publisher_create_with_qos(node, topicC, typeNameC, typeHashC, &cQos)
	} else {
		handle = C.hiroz_publisher_create(node, topicC, typeNameC, typeHashC)
	}

	if handle == nil {
		return nil, fmt.Errorf("%w: publisher for %s", ErrBuildFailed, b.topic)
	}

	pub := &Publisher{
		handle: handle,
		node:   b.node,
		topic:  b.topic,
	}
	runtime.SetFinalizer(pub, (*Publisher).Close)

	return pub, nil
}

// Publish publishes a message
func (p *Publisher) Publish(msg Message) error {
	data, err := msg.SerializeCDR()
	if err != nil {
		return fmt.Errorf("serialization failed: %w", err)
	}

	if len(data) == 0 {
		return fmt.Errorf("empty message")
	}

	handle, release, err := p.acquire()
	if err != nil {
		return err
	}
	defer release()

	// Pin the data to prevent GC relocation during the C call
	pinner := &runtime.Pinner{}
	defer pinner.Unpin()
	pinner.Pin(&data[0])

	result := C.hiroz_publisher_publish(
		handle,
		(*C.uint8_t)(unsafe.Pointer(&data[0])),
		C.size_t(len(data)),
	)

	logger.Debug("hiroz_publisher_publish", "topic", p.topic, "len", len(data), "rc", int(result))

	if result != 0 {
		return newHirozError(ErrorCodePublishFailed,
			fmt.Sprintf("publisher[%s] publish failed (rc=%d)", p.topic, result))
	}
	return nil
}

// acquire keeps the native handle alive for a synchronous operation. Local
// publication can invoke a Go subscriber inline, including one that calls Close.
// Reference counting permits that reentrant close without destroying the handle
// underneath the active publish or blocking on the callback itself.
func (p *Publisher) acquire() (*C.hiroz_publisher_t, func(), error) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.handle == nil {
		return nil, nil, fmt.Errorf("publisher is closed")
	}
	p.active++
	return p.handle, func() {
		p.mu.Lock()
		defer p.mu.Unlock()
		p.active--
		p.destroyRetired()
	}, nil
}

// destroyRetired is called with mu held after Close has disabled new operations.
func (p *Publisher) destroyRetired() {
	if p.active != 0 || p.retired == nil {
		return
	}
	result := C.hiroz_publisher_destroy(p.retired)
	p.retired = nil
	if result != 0 {
		p.closeErr = fmt.Errorf("publisher close failed (rc=%d): %w", result, ErrCloseFailed)
	}
}

// Close prevents further publishing. If an operation is in progress, native
// destruction is deferred until it finishes. Close is safe from a local
// subscriber callback invoked by Publish.
func (p *Publisher) Close() error {
	p.closeOnce.Do(func() {
		p.mu.Lock()
		defer p.mu.Unlock()
		p.retired, p.handle = p.handle, nil
		p.destroyRetired()
		runtime.SetFinalizer(p, nil)
	})
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.closeErr
}
