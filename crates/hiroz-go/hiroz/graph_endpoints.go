package hiroz

/*
#include <stdlib.h>
#include "hiroz_ffi.h"
*/
import "C"
import (
	"context"
	"encoding/json"
	"fmt"
	"sort"
	"time"
	"unsafe"
)

// EndpointKind selects publishers, subscriptions, service servers or clients.
type EndpointKind int32

const (
	EndpointPublisher EndpointKind = iota
	EndpointSubscription
	EndpointService
	EndpointClient
)

// DiscoveredQoS contains policies represented in native graph discovery tokens.
// Deadline, lifespan and liveliness are not retained by the native graph.
type DiscoveredQoS struct {
	Reliability  QosReliability
	Durability   QosDurability
	History      QosHistory
	HistoryDepth int
}

// EndpointNode identifies the node and Zenoh session owning an endpoint.
type EndpointNode struct{ Name, Namespace, Enclave, SessionID string }

// EndpointInfo describes a discovered endpoint. Node/GID may be absent for
// transports whose discovery records do not provide node identity.
type EndpointInfo struct {
	ID                       uint64
	Kind                     EndpointKind
	Name, TypeName, TypeHash string
	GID                      *[16]byte
	Node                     *EndpointNode
	QoS                      DiscoveredQoS
}

// ActionInfo describes an action advertised through its feedback endpoint.
type ActionInfo struct{ Name, TypeName string }

// NodeInfoWithEnclave includes the security enclave advertised by a node.
type NodeInfoWithEnclave struct{ Name, Namespace, Enclave string }

func decodeNativeJSON(ptr *C.char, dest any) error {
	if ptr == nil {
		return fmt.Errorf("native inspection failed")
	}
	defer C.hiroz_json_destroy(ptr)
	return json.Unmarshal([]byte(C.GoString(ptr)), dest)
}

// GetEndpoints returns endpoints of kind for a fully qualified topic or service.
func (c *Context) GetEndpoints(kind EndpointKind, name string) ([]EndpointInfo, error) {
	return c.endpoints(kind, name, "", "")
}

// GetEndpointsByNode returns endpoints belonging to a named node.
func (c *Context) GetEndpointsByNode(kind EndpointKind, name, namespace string) ([]EndpointInfo, error) {
	if name == "" {
		return nil, fmt.Errorf("node name cannot be empty")
	}
	return c.endpoints(kind, "", name, namespace)
}
func (c *Context) endpoints(kind EndpointKind, name, node, namespace string) ([]EndpointInfo, error) {
	handle, release, err := c.acquire()
	if err != nil {
		return nil, err
	}
	defer release()
	if kind < EndpointPublisher || kind > EndpointClient {
		return nil, fmt.Errorf("invalid endpoint kind: %d", kind)
	}
	a, b, d := C.CString(name), C.CString(node), C.CString(namespace)
	defer C.free(unsafe.Pointer(a))
	defer C.free(unsafe.Pointer(b))
	defer C.free(unsafe.Pointer(d))
	var result []EndpointInfo
	err = decodeNativeJSON(C.hiroz_graph_endpoints_json(handle, C.int32_t(kind), a, b, d), &result)
	sort.Slice(result, func(i, j int) bool {
		a, _ := json.Marshal(result[i])
		b, _ := json.Marshal(result[j])
		return string(a) < string(b)
	})
	return result, err
}

// CountEndpoints counts discovered endpoints, including endpoints with a different
// type or incompatible QoS. Discovery is asynchronous, not a delivery guarantee.
func (c *Context) CountEndpoints(kind EndpointKind, name string) (int, error) {
	es, err := c.GetEndpoints(kind, name)
	return len(es), err
}
func (c *Context) details(kind int, name, namespace string, dest any) error {
	handle, release, err := c.acquire()
	if err != nil {
		return err
	}
	defer release()
	a, b := C.CString(name), C.CString(namespace)
	defer C.free(unsafe.Pointer(a))
	defer C.free(unsafe.Pointer(b))
	return decodeNativeJSON(C.hiroz_graph_details_json(handle, C.int32_t(kind), a, b), dest)
}
func (c *Context) GetActionNamesAndTypes() ([]ActionInfo, error) {
	var r []ActionInfo
	err := c.details(0, "", "", &r)
	return r, err
}
func (c *Context) GetActionClientNamesAndTypesByNode(name, namespace string) ([]ActionInfo, error) {
	var r []ActionInfo
	err := c.details(1, name, namespace, &r)
	return r, err
}
func (c *Context) GetActionServerNamesAndTypesByNode(name, namespace string) ([]ActionInfo, error) {
	var r []ActionInfo
	err := c.details(2, name, namespace, &r)
	return r, err
}
func (c *Context) GetNodeNamesWithEnclaves() ([]NodeInfoWithEnclave, error) {
	var r []NodeInfoWithEnclave
	err := c.details(3, "", "", &r)
	return r, err
}

// EndpointEvent is a coalesced discovery snapshot. It is not a DDS deadline,
// liveliness, incompatible-QoS or delivery event. Initial is true for the first snapshot.
type EndpointEvent struct {
	Endpoints []EndpointInfo
	Initial   bool
	Err       error
}

// WatchEndpoints polls graph discovery every interval and emits changed snapshots.
// A slow consumer receives the latest snapshot; brief changes between polls may
// be coalesced. Cancel ctx before closing Context.
func (c *Context) WatchEndpoints(ctx context.Context, kind EndpointKind, name string, interval time.Duration) (<-chan EndpointEvent, error) {
	if ctx == nil || interval <= 0 {
		return nil, fmt.Errorf("context and positive interval required")
	}
	first, err := c.GetEndpoints(kind, name)
	if err != nil {
		return nil, err
	}
	out := make(chan EndpointEvent, 1)
	out <- EndpointEvent{Endpoints: first, Initial: true}
	go func() {
		defer close(out)
		timer := time.NewTicker(interval)
		defer timer.Stop()
		previous, _ := json.Marshal(first)
		for {
			select {
			case <-ctx.Done():
				return
			case <-timer.C:
				next, err := c.GetEndpoints(kind, name)
				encoded, _ := json.Marshal(next)
				if err == nil && string(encoded) == string(previous) {
					continue
				}
				previous = encoded
				event := EndpointEvent{Endpoints: next, Err: err}
				select {
				case out <- event:
				default:
					select {
					case <-out:
					default:
					}
					select {
					case out <- event:
					case <-ctx.Done():
						return
					}
				}
				if err != nil {
					return
				}
			}
		}
	}()
	return out, nil
}
