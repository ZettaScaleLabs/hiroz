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

// Node represents a ROS 2 node
type Node struct {
	handle    *C.hiroz_node_t
	ctx       *Context
	closeOnce sync.Once
	mu        sync.Mutex
	active    int
	retired   *C.hiroz_node_t
	closeErr  error
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
	ctx, release, err := b.ctx.acquire()
	if err != nil {
		return nil, err
	}
	defer release()
	var handle *C.hiroz_node_t

	if b.enableTypeDescriptionService {
		// Use config-based creation
		cName := C.CString(b.name)
		defer C.free(unsafe.Pointer(cName))

		var cfg C.hiroz_node_config_t
		cfg.name = cName
		cfg.enable_type_description_service = C.bool(b.enableTypeDescriptionService)

		if b.namespace != "" {
			cNamespace := C.CString(b.namespace)
			defer C.free(unsafe.Pointer(cNamespace))
			cfg.namespace_ = cNamespace
		}

		handle = C.hiroz_node_create_with_config(ctx, &cfg)
	} else {
		// Simple path
		cName := C.CString(b.name)
		defer C.free(unsafe.Pointer(cName))

		var cNamespace *C.char
		if b.namespace != "" {
			cNamespace = C.CString(b.namespace)
			defer C.free(unsafe.Pointer(cNamespace))
		}

		handle = C.hiroz_node_create(ctx, cName, cNamespace)
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

// acquire retains the native node for an operation without holding a lock over
// callbacks. This also permits Close from transient-local replay during Build.
func (n *Node) acquire() (*C.hiroz_node_t, func(), error) {
	n.mu.Lock()
	defer n.mu.Unlock()
	if n.handle == nil {
		return nil, nil, fmt.Errorf("node is closed")
	}
	n.active++
	return n.handle, func() { n.mu.Lock(); defer n.mu.Unlock(); n.active--; n.destroyRetired() }, nil
}
func (n *Node) destroyRetired() {
	if n.active != 0 || n.retired == nil {
		return
	}
	rc := C.hiroz_node_destroy(n.retired)
	n.retired = nil
	if rc != 0 {
		n.closeErr = fmt.Errorf("node close failed (rc=%d): %w", rc, ErrCloseFailed)
	}
}

// ownSubscriber registers a completed build atomically against Node.Close.
func (n *Node) ownSubscriber(sub *Subscriber) error {
	n.mu.Lock()
	defer n.mu.Unlock()
	if n.handle == nil {
		return fmt.Errorf("node closed during subscriber construction")
	}
	n.subsMu.Lock()
	n.ownedSubs = append(n.ownedSubs, sub)
	n.subsMu.Unlock()
	return nil
}

// Close stops owned subscriptions and prevents new operations. Native destruction
// is deferred until active calls finish; in-flight callbacks may finish later.
func (n *Node) Close() error {
	n.closeOnce.Do(func() {
		n.mu.Lock()
		n.retired, n.handle = n.handle, nil
		n.mu.Unlock()
		n.subsMu.Lock()
		subs := n.ownedSubs
		n.ownedSubs = nil
		n.subsMu.Unlock()
		for _, sub := range subs {
			_ = sub.Close()
		}
		n.mu.Lock()
		n.destroyRetired()
		n.mu.Unlock()
		runtime.SetFinalizer(n, nil)
	})
	n.mu.Lock()
	defer n.mu.Unlock()
	return n.closeErr
}
