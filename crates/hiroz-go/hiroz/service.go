package hiroz

/*
#include <stdlib.h>
#include "hiroz_ffi.h"

extern hiroz_ServiceCallback getServiceCallback();
*/
import "C"
import (
	"context"
	"fmt"
	"runtime"
	"runtime/cgo"
	"sync"
	"sync/atomic"
	"time"
	"unsafe"
)

// CGO opaque types are incomplete and cannot be used as type parameters.
// These thin wrappers make atomic.Pointer usable with CGO handles.
type cServiceClientHandle struct{ p *C.hiroz_service_client_t }
type cServiceServerHandle struct{ p *C.hiroz_service_server_t }

// Service represents a ROS 2 service (request/response pattern)
type Service interface {
	TypeName() string
	TypeHash() string
	// GetRequest returns the request message type
	GetRequest() Message
	// GetResponse returns the response message type
	GetResponse() Message
}

// ServiceClient calls ROS 2 services
type ServiceClient struct {
	mu         sync.RWMutex
	operations map[*serviceOperation]struct{}
	handle     atomic.Pointer[cServiceClientHandle]
	node       *Node
	service    string
	closeOnce  sync.Once
}

// ServiceServer responds to ROS 2 service requests
type ServiceServer struct {
	mu           sync.RWMutex
	callbackMode bool
	stop         context.CancelFunc
	handle       atomic.Pointer[cServiceServerHandle]
	node         *Node
	service      string
	closure      *serviceClosure
	closeOnce    sync.Once
}

// ServiceClientBuilder builds a ServiceClient
type ServiceClientBuilder struct {
	qos     *QosProfile
	node    *Node
	service string
}

// ServiceServerBuilder builds a ServiceServer
type ServiceServerBuilder struct {
	qos     *QosProfile
	node    *Node
	service string
}

// CreateServiceClient creates a new service client builder
func (n *Node) CreateServiceClient(service string) *ServiceClientBuilder {
	return &ServiceClientBuilder{
		node:    n,
		service: service,
	}
}

// CreateServiceServer creates a new service server builder
func (n *Node) CreateServiceServer(service string) *ServiceServerBuilder {
	return &ServiceServerBuilder{
		node:    n,
		service: service,
	}
}

// Build creates the service client
func (b *ServiceClientBuilder) Build(svc Service) (*ServiceClient, error) {
	node, release, err := b.node.acquire()
	if err != nil {
		return nil, err
	}
	defer release()
	serviceC := C.CString(b.service)
	defer C.free(unsafe.Pointer(serviceC))

	// Use the service-level TypeName/TypeHash (DDS format) so the key expression
	// matches rmw_zenoh_cpp and the Rust hiroz native API.
	svcTypeC := C.CString(svc.TypeName())
	defer C.free(unsafe.Pointer(svcTypeC))

	svcHashC := C.CString(svc.TypeHash())
	defer C.free(unsafe.Pointer(svcHashC))

	var cQos *C.hiroz_qos_profile_t
	if b.qos != nil {
		q := b.qos.toCQos()
		cQos = &q
	}
	handle := C.hiroz_service_client_create_with_qos(
		node,
		serviceC,
		svcTypeC, svcHashC,
		svcTypeC, svcHashC,
		cQos,
	)

	if handle == nil {
		return nil, fmt.Errorf("%w: service client for %s", ErrBuildFailed, b.service)
	}

	client := &ServiceClient{
		node:    b.node,
		service: b.service,
	}
	client.handle.Store(&cServiceClientHandle{p: handle})
	runtime.SetFinalizer(client, (*ServiceClient).Close)

	return client, nil
}

// DefaultServiceTimeout is the default timeout used by Call.
const DefaultServiceTimeout = 5 * time.Second

// callRaw makes a synchronous service call with raw CDR bytes.
// Returns the raw CDR response bytes.
func (c *ServiceClient) callRaw(requestBytes []byte, timeoutMs uint64) ([]byte, error) {
	if timeoutMs > uint64((1<<63-1)/int64(time.Millisecond)) {
		return nil, fmt.Errorf("timeout exceeds time.Duration")
	}
	return c.CallRawWithTimeout(requestBytes, time.Duration(timeoutMs)*time.Millisecond)
}

func (c *ServiceClient) call(request Message) ([]byte, error) { return c.Call(request) }
func (c *ServiceClient) callWithTimeout(request Message, timeout time.Duration) ([]byte, error) {
	return c.CallWithTimeout(request, timeout)
}

// WaitForService waits for graph discovery. Zero performs a nonblocking check;
// a negative duration is invalid. Use WaitForServiceWithContext for cancellation.
func (c *ServiceClient) WaitForService(timeout time.Duration) error {
	if timeout < 0 {
		return fmt.Errorf("service timeout must not be negative")
	}
	if timeout == 0 {
		return c.checkService(0)
	}
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	err := c.WaitForServiceWithContext(ctx)
	if err == context.DeadlineExceeded {
		return newHirozError(ErrorCodeServiceTimeout, "service readiness timed out")
	}
	return err
}

// Close prevents new calls and cancels outstanding calls. Independent C query
// handles retain their session until their caller has drained and destroyed them.
func (c *ServiceClient) Close() error {
	var err error
	c.closeOnce.Do(func() {
		c.mu.Lock()
		hw := c.handle.Swap(nil)
		for operation := range c.operations {
			operation.cancel()
		}
		c.mu.Unlock()
		if hw != nil {
			if rc := C.hiroz_service_client_destroy(hw.p); rc != 0 {
				err = fmt.Errorf("service client close failed (rc=%d): %w", rc, ErrCloseFailed)
			}
		}
	})
	return err
}

// serviceClosure wraps a service callback with pinning for safe C access
type serviceClosure struct {
	name       string // service name, for logging
	callback   func([]byte) ([]byte, error)
	handle     cgo.Handle
	selfHandle cgo.Handle // passed as userData to C; recover via cgo.Handle(userData).Value()
	pinner     *runtime.Pinner
}

// newServiceClosure creates a pinned service closure
func newServiceClosure(name string, callback func([]byte) ([]byte, error)) *serviceClosure {
	sc := &serviceClosure{
		name:     name,
		callback: callback,
		handle:   cgo.NewHandle(callback),
	}
	sc.pinner = &runtime.Pinner{}
	sc.pinner.Pin(sc)
	sc.selfHandle = cgo.NewHandle(sc)
	return sc
}

// drop cleans up the service closure
func (sc *serviceClosure) drop() {
	sc.handle.Delete()
	sc.selfHandle.Delete()
	sc.pinner.Unpin()
}

// Build creates the service server with callback.
// The callback receives raw request bytes and must return raw response bytes.
// Callbacks run serially on a Go goroutine. Use BuildQueued for deferred or
// concurrent processing. A callback may close its own server safely.
func (b *ServiceServerBuilder) Build(svc Service, callback func([]byte) ([]byte, error)) (*ServiceServer, error) {
	if callback == nil {
		return nil, fmt.Errorf("service callback cannot be nil")
	}
	server, err := b.BuildQueued(svc)
	if err != nil {
		return nil, err
	}
	server.callbackMode = true
	ctx, cancel := context.WithCancel(context.Background())
	server.stop = cancel
	// User callbacks run in Go; there are no pinned callbacks owned by C.
	// Each request owns its reply handle independently of the server lifetime.
	go func() {
		for {
			request, err := server.takeRequest(ctx)
			if err != nil {
				if native, ok := err.(HirozError); ok && native.Code() == ErrorCodeDeserializationFailed {
					// A malformed peer request must not stop this service's worker.
					continue
				}
				return
			}
			_ = safeCall(func() error {
				defer request.Close()
				response, err := callback(request.Data)
				if err != nil {
					return err
				}
				return request.ReplyRaw(response)
			})
		}
	}()
	return server, nil
}

// Close stops new requests. Already delivered requests remain valid until replied
// to or closed, so callbacks can safely close their own server.
func (s *ServiceServer) Close() error {
	var err error
	s.closeOnce.Do(func() {
		s.mu.Lock()
		hw := s.handle.Swap(nil)
		if s.stop != nil {
			s.stop()
		}
		s.mu.Unlock()
		if hw != nil {
			if rc := C.hiroz_service_server_destroy(hw.p); rc != 0 {
				err = fmt.Errorf("service server close failed (rc=%d): %w", rc, ErrCloseFailed)
			}
		}
	})
	return err
}

//export goServiceCallback
func goServiceCallback(userData C.uintptr_t, reqData *C.uint8_t, reqLen C.size_t, respData **C.uint8_t, respLen *C.size_t) (rc C.int32_t) {
	// Cast userData back to serviceClosure pointer
	closure := cgo.Handle(userData).Value().(*serviceClosure)

	// Copy request data to Go before entering safeCall.
	goReqData := C.GoBytes(unsafe.Pointer(reqData), C.int(reqLen))

	logger.Debug("goServiceCallback", "service", closure.name, "req_len", int(reqLen))

	err := safeCall(func() error {
		// Call user callback
		goRespData, err := closure.callback(goReqData)
		if err != nil {
			logger.Error("service callback error", "service", closure.name, "err", err)
			return err
		}

		if len(goRespData) == 0 {
			logger.Error("service callback returned empty response", "service", closure.name)
			return fmt.Errorf("empty response")
		}

		// Allocate response in C memory (will be freed by Rust)
		*respLen = C.size_t(len(goRespData))
		*respData = (*C.uint8_t)(C.CBytes(goRespData))
		return nil
	})

	if err != nil {
		return -1
	}
	return 0
}
