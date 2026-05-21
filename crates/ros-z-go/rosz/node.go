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

// Node represents a ROS 2 node
type Node struct {
	handle    *C.ros_z_node_t
	ctx       *Context
	closeOnce sync.Once
	// ownedSubs keeps callback-based subscribers alive for the node's lifetime.
	// Matches rmw_zenoh_cpp's NodeData::subs_ ownership pattern: the node is the
	// source of truth for subscription lifetime, not the caller's variable.
	ownedSubs []*Subscriber
	subsMu    sync.Mutex
}

// NodeBuilder builds a Node
type NodeBuilder struct {
	ctx                          *Context
	name                         string
	namespace                    string
	enableTypeDescriptionService bool
}

// WithNamespace sets the node namespace
func (b *NodeBuilder) WithNamespace(ns string) *NodeBuilder {
	b.namespace = ns
	return b
}

// WithTypeDescriptionService enables the type description service for this node.
// When enabled, `ros2 topic info --verbose` can query type descriptions from this node.
func (b *NodeBuilder) WithTypeDescriptionService() *NodeBuilder {
	b.enableTypeDescriptionService = true
	return b
}

// Build creates the node
func (b *NodeBuilder) Build() (*Node, error) {
	var handle *C.ros_z_node_t

	if b.enableTypeDescriptionService {
		// Use config-based creation
		cName := C.CString(b.name)
		defer C.free(unsafe.Pointer(cName))

		var cfg C.ros_z_node_config_t
		cfg.name = cName
		cfg.enable_type_description_service = C.bool(b.enableTypeDescriptionService)

		if b.namespace != "" {
			cNamespace := C.CString(b.namespace)
			defer C.free(unsafe.Pointer(cNamespace))
			cfg.namespace_ = cNamespace
		}

		handle = C.ros_z_node_create_with_config(b.ctx.handle, &cfg)
	} else {
		// Simple path
		cName := C.CString(b.name)
		defer C.free(unsafe.Pointer(cName))

		var cNamespace *C.char
		if b.namespace != "" {
			cNamespace = C.CString(b.namespace)
			defer C.free(unsafe.Pointer(cNamespace))
		}

		handle = C.ros_z_node_create(b.ctx.handle, cName, cNamespace)
	}

	if handle == nil {
		return nil, fmt.Errorf("%w: node %s", ErrBuildFailed, b.name)
	}

	node := &Node{
		handle: handle,
		ctx:    b.ctx,
	}
	runtime.SetFinalizer(node, (*Node).Close)

	return node, nil
}

// DestroySubscriber removes a callback-based subscriber from node ownership and closes it,
// undeclaring its Zenoh subscription immediately.
//
// Matches rclpy's Node.destroy_subscription(). After this call the subscriber is closed
// and must not be used again. Returns nil if sub was not found (already closed or not owned).
func (n *Node) DestroySubscriber(sub *Subscriber) error {
	n.subsMu.Lock()
	idx := -1
	for i, s := range n.ownedSubs {
		if s == sub { //nolint:gocritic // pointer comparison is intentional
			idx = i
			break
		}
	}
	if idx >= 0 {
		last := len(n.ownedSubs) - 1
		n.ownedSubs[idx] = n.ownedSubs[last]
		n.ownedSubs[last] = nil
		n.ownedSubs = n.ownedSubs[:last]
	}
	n.subsMu.Unlock()

	if idx >= 0 {
		return sub.Close()
	}
	return nil
}

// CreatePublisher creates a new publisher builder
func (n *Node) CreatePublisher(topic string) *PublisherBuilder {
	return &PublisherBuilder{
		node:  n,
		topic: topic,
	}
}

// CreateSubscriber creates a new subscriber builder
func (n *Node) CreateSubscriber(topic string) *SubscriberBuilder {
	return &SubscriberBuilder{
		node:  n,
		topic: topic,
	}
}

// Close destroys the node.
// Owned callback subscribers are closed first so their Zenoh subscriptions
// are undeclared before the node handle is destroyed, preventing callbacks
// from firing on the Zenoh/CGo thread after Close returns.
func (n *Node) Close() error {
	var err error
	n.closeOnce.Do(func() {
		if n.handle == nil {
			return
		}
		// Drain owned subscribers before destroying the node handle.
		n.subsMu.Lock()
		subs := n.ownedSubs
		n.ownedSubs = nil
		n.subsMu.Unlock()
		for _, sub := range subs {
			sub.Close()
		}

		result := C.ros_z_node_destroy(n.handle)
		n.handle = nil
		if result != 0 {
			err = fmt.Errorf("node close failed (rc=%d): %w", result, ErrCloseFailed)
		}
	})
	return err
}
