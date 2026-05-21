package rosz

/*
#include <stdlib.h>
#include "ros_z_ffi.h"
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
	handle    *C.ros_z_publisher_t
	node      *Node
	topic     string
	closeOnce sync.Once
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
	topicC := C.CString(b.topic)
	defer C.free(unsafe.Pointer(topicC))

	typeNameC := C.CString(msg.TypeName())
	defer C.free(unsafe.Pointer(typeNameC))

	typeHashC := C.CString(msg.TypeHash())
	defer C.free(unsafe.Pointer(typeHashC))

	var handle *C.ros_z_publisher_t
	if b.qos != nil {
		cQos := b.qos.toCQos()
		handle = C.ros_z_publisher_create_with_qos(b.node.handle, topicC, typeNameC, typeHashC, &cQos)
	} else {
		handle = C.ros_z_publisher_create(b.node.handle, topicC, typeNameC, typeHashC)
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
	if p.handle == nil {
		return fmt.Errorf("publisher is closed")
	}

	data, err := msg.SerializeCDR()
	if err != nil {
		return fmt.Errorf("serialization failed: %w", err)
	}

	if len(data) == 0 {
		return fmt.Errorf("empty message")
	}

	// Pin the data to prevent GC relocation during the C call
	pinner := &runtime.Pinner{}
	defer pinner.Unpin()
	pinner.Pin(&data[0])

	result := C.ros_z_publisher_publish(
		p.handle,
		(*C.uint8_t)(unsafe.Pointer(&data[0])),
		C.size_t(len(data)),
	)

	logger.Debug("ros_z_publisher_publish", "topic", p.topic, "len", len(data), "rc", int(result))

	if result != 0 {
		return newRoszError(ErrorCodePublishFailed,
			fmt.Sprintf("publisher[%s] publish failed (rc=%d)", p.topic, result))
	}
	return nil
}

// Close destroys the publisher
func (p *Publisher) Close() error {
	var err error
	p.closeOnce.Do(func() {
		if p.handle == nil {
			return
		}
		result := C.ros_z_publisher_destroy(p.handle)
		p.handle = nil
		if result != 0 {
			err = fmt.Errorf("publisher close failed (rc=%d): %w", result, ErrCloseFailed)
		}
	})
	return err
}
