package hiroz

/*
#include <stdlib.h>
#include "hiroz_ffi.h"
*/
import "C"
import (
	"context"
	"fmt"
	"runtime"
	"sync"
	"time"
	"unsafe"
)

// ServiceRequestID identifies a request across clients and sequence numbers.
type ServiceRequestID struct {
	SequenceNumber int64
	WriterGID      [16]byte
}

// ServiceRequest owns one deferred reply. Data is a Go-owned CDR payload.
// Reply, ReplyRaw or Close releases the query. A delivered request remains valid
// after the server is closed while its context/session is still alive.
type ServiceRequest struct {
	ID     ServiceRequestID
	Data   []byte
	mu     sync.Mutex
	handle *C.hiroz_service_request_t
}

// BuildQueued creates a service whose requests are consumed with TakeRequest or
// TryTakeRequest. Applications can send each request to a worker and reply later.
func (b *ServiceServerBuilder) BuildQueued(svc Service) (*ServiceServer, error) {
	node, release, err := b.node.acquire()
	if err != nil {
		return nil, err
	}
	defer release()
	service := C.CString(b.service)
	defer C.free(unsafe.Pointer(service))
	name := C.CString(svc.TypeName())
	defer C.free(unsafe.Pointer(name))
	hash := C.CString(svc.TypeHash())
	defer C.free(unsafe.Pointer(hash))
	var qos *C.hiroz_qos_profile_t
	if b.qos != nil {
		value := b.qos.toCQos()
		qos = &value
	}
	handle := C.hiroz_service_server_create_queued(node, service, name, hash, qos)
	if handle == nil {
		return nil, fmt.Errorf("%w: service server for %s", ErrBuildFailed, b.service)
	}
	server := &ServiceServer{node: b.node, service: b.service}
	server.handle.Store(&cServiceServerHandle{p: handle})
	runtime.SetFinalizer(server, (*ServiceServer).Close)
	return server, nil
}

func (s *ServiceServer) tryTakeRequest(wait time.Duration) (*ServiceRequest, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	hw := s.handle.Load()
	if hw == nil {
		return nil, fmt.Errorf("service server is closed")
	}
	var request *C.hiroz_service_request_t
	var data *C.uint8_t
	var size C.uintptr_t
	var id C.hiroz_service_request_id_t
	rc := C.hiroz_service_server_take_request(hw.p, C.uint64_t(durationMillis(wait)), &request, &data, &size, &id)
	if rc == 1 {
		return nil, nil
	}
	if rc != 0 {
		return nil, newHirozError(ErrorCode(rc), "failed to take service request")
	}
	payload := C.GoBytes(unsafe.Pointer(data), C.int(size))
	C.hiroz_free_bytes(data, size)
	result := &ServiceRequest{Data: payload, handle: request, ID: ServiceRequestID{SequenceNumber: int64(id.sequence_number)}}
	for i := range result.ID.WriterGID {
		result.ID.WriterGID[i] = byte(id.writer_gid[i])
	}
	runtime.SetFinalizer(result, (*ServiceRequest).Close)
	return result, nil
}

// TryTakeRequest returns nil, nil when no request is queued. Only available on
// servers created by BuildQueued; callback servers own their request consumer.
func (s *ServiceServer) TryTakeRequest() (*ServiceRequest, error) {
	if s.callbackMode {
		return nil, fmt.Errorf("callback service server owns its request consumer")
	}
	return s.tryTakeRequest(0)
}

// TakeRequest blocks until a request is available, the context is canceled, or
// the server closes. Only available on servers created with BuildQueued.
func (s *ServiceServer) TakeRequest(ctx context.Context) (*ServiceRequest, error) {
	if s.callbackMode {
		return nil, fmt.Errorf("callback service server owns its request consumer")
	}
	return s.takeRequest(ctx)
}

func (s *ServiceServer) takeRequest(ctx context.Context) (*ServiceRequest, error) {
	for {
		if err := ctx.Err(); err != nil {
			return nil, err
		}
		wait := 20 * time.Millisecond
		if deadline, ok := ctx.Deadline(); ok {
			if left := time.Until(deadline); left < wait {
				if left <= 0 {
					return nil, context.DeadlineExceeded
				}
				wait = left
			}
		}
		request, err := s.tryTakeRequest(wait)
		if err != nil {
			return nil, err
		}
		if request != nil {
			return request, nil
		}
	}
}

// Decode deserializes this request into an allocated message destination.
func (r *ServiceRequest) Decode(message Message) error { return message.DeserializeCDR(r.Data) }

// Reply serializes and sends the response, then releases the request handle.
func (r *ServiceRequest) Reply(response Message) error {
	data, err := response.SerializeCDR()
	if err != nil {
		return fmt.Errorf("serialize response: %w", err)
	}
	return r.ReplyRaw(data)
}

// ReplyRaw sends one CDR response and releases this request. Repeated replies
// return an error. Serialization failures leave the request available for retry.
func (r *ServiceRequest) ReplyRaw(data []byte) error {
	if len(data) == 0 {
		return fmt.Errorf("empty service response")
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.handle == nil {
		return fmt.Errorf("service request is closed or already answered")
	}
	handle := r.handle
	r.handle = nil
	defer C.hiroz_service_request_destroy(handle)
	rc := C.hiroz_service_request_reply(handle, (*C.uint8_t)(unsafe.Pointer(&data[0])), C.uintptr_t(len(data)))
	runtime.KeepAlive(data)
	if rc != 0 {
		return newHirozError(ErrorCode(rc), "failed to reply to service request")
	}
	return nil
}

// Close abandons an unanswered request. It is idempotent and safe with Reply.
func (r *ServiceRequest) Close() error {
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.handle == nil {
		return nil
	}
	handle := r.handle
	r.handle = nil
	rc := C.hiroz_service_request_destroy(handle)
	if rc != 0 {
		return fmt.Errorf("service request close failed: %w", ErrCloseFailed)
	}
	return nil
}
