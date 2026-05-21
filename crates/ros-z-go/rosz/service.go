package rosz

/*
#include <stdlib.h>
#include "ros_z_ffi.h"

extern ros_z_ServiceCallback getServiceCallback();
*/
import "C"
import (
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
type cServiceClientHandle struct{ p *C.ros_z_service_client_t }
type cServiceServerHandle struct{ p *C.ros_z_service_server_t }

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
	handle    atomic.Pointer[cServiceClientHandle]
	node      *Node
	service   string
	closeOnce sync.Once
}

// ServiceServer responds to ROS 2 service requests
type ServiceServer struct {
	handle    atomic.Pointer[cServiceServerHandle]
	node      *Node
	service   string
	closure   *serviceClosure
	closeOnce sync.Once
}

// ServiceClientBuilder builds a ServiceClient
type ServiceClientBuilder struct {
	node    *Node
	service string
}

// ServiceServerBuilder builds a ServiceServer
type ServiceServerBuilder struct {
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
	serviceC := C.CString(b.service)
	defer C.free(unsafe.Pointer(serviceC))

	// Use the service-level TypeName/TypeHash (DDS format) so the key expression
	// matches rmw_zenoh_cpp and the Rust ros-z native API.
	svcTypeC := C.CString(svc.TypeName())
	defer C.free(unsafe.Pointer(svcTypeC))

	svcHashC := C.CString(svc.TypeHash())
	defer C.free(unsafe.Pointer(svcHashC))

	handle := C.ros_z_service_client_create(
		b.node.handle,
		serviceC,
		svcTypeC, svcHashC,
		svcTypeC, svcHashC,
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
	hw := c.handle.Load()
	if hw == nil {
		return nil, fmt.Errorf("service client is closed")
	}
	h := hw.p

	if len(requestBytes) == 0 {
		return nil, fmt.Errorf("empty request")
	}

	// Pin the request data to prevent GC relocation during the C call
	pinner := &runtime.Pinner{}
	defer pinner.Unpin()
	pinner.Pin(&requestBytes[0])

	var respPtr *C.uint8_t
	var respLen C.uintptr_t

	result := C.ros_z_service_client_call(
		h,
		(*C.uint8_t)(unsafe.Pointer(&requestBytes[0])),
		C.uintptr_t(len(requestBytes)),
		&respPtr,
		&respLen,
		C.uint64_t(timeoutMs),
	)

	logger.Debug("ros_z_service_client_call",
		"service", c.service, "req_len", len(requestBytes),
		"resp_len", int(respLen), "rc", int(result))

	if result != 0 {
		code := ErrorCode(result)
		if code == ErrorCodeServiceTimeout {
			return nil, newRoszError(ErrorCodeServiceTimeout,
				fmt.Sprintf("service[%s] call timed out", c.service))
		}
		return nil, newRoszError(ErrorCodeServiceCallFailed,
			fmt.Sprintf("service[%s] call failed (rc=%d)", c.service, result))
	}

	respBytes := C.GoBytes(unsafe.Pointer(respPtr), C.int(respLen))
	C.ros_z_free_bytes((*C.uint8_t)(respPtr), C.uintptr_t(respLen))

	return respBytes, nil
}

// call makes a synchronous service call with a default timeout of DefaultServiceTimeout.
// The request is serialized via CDR, sent, and the raw response bytes are returned.
// For typed responses use rosz.CallTyped(client, req, &resp).
func (c *ServiceClient) call(request Message) ([]byte, error) {
	reqBytes, err := request.SerializeCDR()
	if err != nil {
		return nil, fmt.Errorf("failed to serialize request: %w", err)
	}

	return c.callRaw(reqBytes, uint64(DefaultServiceTimeout.Milliseconds()))
}

// callWithTimeout makes a synchronous service call with a custom timeout.
// The request is serialized via CDR, sent, and the raw response bytes are returned.
func (c *ServiceClient) callWithTimeout(request Message, timeout time.Duration) ([]byte, error) {
	reqBytes, err := request.SerializeCDR()
	if err != nil {
		return nil, fmt.Errorf("failed to serialize request: %w", err)
	}

	timeoutMs := uint64(timeout.Milliseconds())
	if timeoutMs == 0 {
		timeoutMs = 1
	}
	return c.callRaw(reqBytes, timeoutMs)
}

// WaitForService blocks until at least one matching service server is visible
// in the ROS graph, or `timeout` elapses. Returns nil when the service becomes
// ready, an error wrapping ErrorCodeServiceTimeout on timeout, or another error
// if the client is closed.
//
// Mirrors rclcpp's `Client::wait_for_service`. Use this instead of fixed sleeps
// to coordinate readiness between a service client and a freshly-created
// service server (the graph announce is asynchronous over zenoh liveliness).
func (c *ServiceClient) WaitForService(timeout time.Duration) error {
	hw := c.handle.Load()
	if hw == nil {
		return fmt.Errorf("service client is closed")
	}

	timeoutMs := uint64(timeout.Milliseconds())
	if timeoutMs == 0 && timeout > 0 {
		timeoutMs = 1
	}

	rc := C.ros_z_service_client_wait_for_service(hw.p, C.uint64_t(timeoutMs))
	if rc == 0 {
		return nil
	}
	code := ErrorCode(rc)
	if code == ErrorCodeServiceTimeout {
		return newRoszError(ErrorCodeServiceTimeout,
			fmt.Sprintf("service[%s] not available after %s", c.service, timeout))
	}
	return newRoszError(code,
		fmt.Sprintf("service[%s] wait_for_service failed (rc=%d)", c.service, rc))
}

// Close destroys the service client
func (c *ServiceClient) Close() error {
	var err error
	c.closeOnce.Do(func() {
		hw := c.handle.Load()
		if hw == nil {
			return
		}
		c.handle.Store(nil)
		result := C.ros_z_service_client_destroy(hw.p)
		if result != 0 {
			err = fmt.Errorf("service client close failed (rc=%d): %w", result, ErrCloseFailed)
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
// Note: the callback is invoked on a C/Rust thread — avoid long blocking operations.
func (b *ServiceServerBuilder) Build(svc Service, callback func([]byte) ([]byte, error)) (*ServiceServer, error) {
	serviceC := C.CString(b.service)
	defer C.free(unsafe.Pointer(serviceC))

	// Use the service-level TypeName/TypeHash (DDS format) so the key expression
	// matches rmw_zenoh_cpp and the Rust ros-z native API.
	svcTypeC := C.CString(svc.TypeName())
	defer C.free(unsafe.Pointer(svcTypeC))

	svcHashC := C.CString(svc.TypeHash())
	defer C.free(unsafe.Pointer(svcHashC))

	// Create pinned closure
	closure := newServiceClosure(b.service, callback)

	handle := C.ros_z_service_server_create(
		b.node.handle,
		serviceC,
		svcTypeC, svcHashC,
		svcTypeC, svcHashC,
		C.getServiceCallback(),
		// closure.pinner.Pin(closure) guarantees the struct address is stable,
		// making this uintptr cast safe. Do not remove the Pin call above.
		C.uintptr_t(closure.selfHandle),
	)

	if handle == nil {
		closure.drop()
		return nil, fmt.Errorf("%w: service server for %s", ErrBuildFailed, b.service)
	}

	server := &ServiceServer{
		node:    b.node,
		service: b.service,
		closure: closure,
	}
	server.handle.Store(&cServiceServerHandle{p: handle})
	runtime.SetFinalizer(server, (*ServiceServer).Close)

	return server, nil
}

// Close destroys the service server
func (s *ServiceServer) Close() error {
	var err error
	s.closeOnce.Do(func() {
		hw := s.handle.Load()
		if hw == nil {
			return
		}
		s.handle.Store(nil)
		result := C.ros_z_service_server_destroy(hw.p)
		if s.closure != nil {
			s.closure.drop()
			s.closure = nil
		}
		if result != 0 {
			err = fmt.Errorf("service server close failed (rc=%d): %w", result, ErrCloseFailed)
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
