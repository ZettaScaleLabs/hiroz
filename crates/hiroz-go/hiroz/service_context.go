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
	"time"
	"unsafe"
)

// ErrorCodeOperationCanceled indicates cancellation of a native operation.
const ErrorCodeOperationCanceled ErrorCode = -18

type serviceOperation struct{ cancel context.CancelFunc }

// A context's timer can be delayed while native code or the Go scheduler is busy.
// Check its absolute deadline as well as its cancellation signal at API boundaries.
func serviceContextError(ctx context.Context) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	if deadline, ok := ctx.Deadline(); ok && !time.Now().Before(deadline) {
		return context.DeadlineExceeded
	}
	return nil
}

// WithQoS sets the service client's endpoint QoS.
func (b *ServiceClientBuilder) WithQoS(qos QosProfile) *ServiceClientBuilder { b.qos = &qos; return b }

// WithQoS sets the service server's QoS, including its request queue depth.
func (b *ServiceServerBuilder) WithQoS(qos QosProfile) *ServiceServerBuilder { b.qos = &qos; return b }

// Call sends a message and returns raw CDR using DefaultServiceTimeout.
func (c *ServiceClient) Call(request Message) ([]byte, error) {
	return c.CallWithTimeout(request, DefaultServiceTimeout)
}

// CallWithTimeout applies a positive timeout to the transport query and wait.
func (c *ServiceClient) CallWithTimeout(request Message, timeout time.Duration) ([]byte, error) {
	if timeout <= 0 {
		return nil, fmt.Errorf("service timeout must be positive")
	}
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	data, err := c.CallWithContext(ctx, request)
	if err == context.DeadlineExceeded {
		return nil, newHirozError(ErrorCodeServiceTimeout, "service call timed out")
	}
	return data, err
}

// CallWithContext sends a message with a cancelable deadline. Canceling the wait
// cancels the transport query; it does not undo work already started by the server.
func (c *ServiceClient) CallWithContext(ctx context.Context, request Message) ([]byte, error) {
	if err := serviceContextError(ctx); err != nil {
		return nil, err
	}
	data, err := request.SerializeCDR()
	if err != nil {
		return nil, fmt.Errorf("serialize request: %w", err)
	}
	return c.CallRawWithContext(ctx, data)
}

// CallRaw sends already serialized CDR bytes using DefaultServiceTimeout.
func (c *ServiceClient) CallRaw(request []byte) ([]byte, error) {
	return c.CallRawWithTimeout(request, DefaultServiceTimeout)
}

// CallRawWithTimeout applies a positive timeout to an already serialized request.
func (c *ServiceClient) CallRawWithTimeout(request []byte, timeout time.Duration) ([]byte, error) {
	if timeout <= 0 {
		return nil, fmt.Errorf("service timeout must be positive")
	}
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	data, err := c.CallRawWithContext(ctx, request)
	if err == context.DeadlineExceeded {
		return nil, newHirozError(ErrorCodeServiceTimeout, "service call timed out")
	}
	return data, err
}

func durationMillis(d time.Duration) uint64 {
	n := uint64(d / time.Millisecond)
	if d%time.Millisecond != 0 {
		n++
	}
	return n
}

// CallRawWithContext sends CDR without a serialization adapter. With no context
// deadline, no application timeout is imposed. Cancellation is checked at most
// every 20ms and releases the underlying Zenoh query.
func (c *ServiceClient) CallRawWithContext(parent context.Context, request []byte) ([]byte, error) {
	if err := serviceContextError(parent); err != nil {
		return nil, err
	}
	if len(request) == 0 {
		return nil, fmt.Errorf("empty request")
	}
	ctx, cancel := context.WithCancel(parent)
	defer cancel()
	op := &serviceOperation{cancel: cancel}
	c.mu.Lock()
	if err := serviceContextError(ctx); err != nil {
		c.mu.Unlock()
		return nil, err
	}
	timeout := time.Duration(1<<63 - 1)
	if deadline, ok := ctx.Deadline(); ok {
		timeout = time.Until(deadline)
		if timeout <= 0 {
			c.mu.Unlock()
			return nil, context.DeadlineExceeded
		}
	}
	hw := c.handle.Load()
	if hw == nil {
		c.mu.Unlock()
		return nil, fmt.Errorf("service client is closed")
	}
	if c.operations == nil {
		c.operations = make(map[*serviceOperation]struct{})
	}
	c.operations[op] = struct{}{}
	var call *C.hiroz_service_call_t
	rc := C.hiroz_service_client_start_call(hw.p, (*C.uint8_t)(unsafe.Pointer(&request[0])), C.uintptr_t(len(request)), C.uint64_t(durationMillis(timeout)), &call)
	c.mu.Unlock()
	runtime.KeepAlive(request)
	defer func() { c.mu.Lock(); delete(c.operations, op); c.mu.Unlock() }()
	if rc != 0 {
		return nil, newHirozError(ErrorCode(rc), "failed to start service call")
	}
	defer C.hiroz_service_call_destroy(call)
	for {
		if err := serviceContextError(ctx); err != nil {
			return nil, err
		}
		var data *C.uint8_t
		var size C.uintptr_t
		rc = C.hiroz_service_call_poll(call, 20, &data, &size)
		if rc == 1 {
			continue
		}
		if rc != 0 {
			if err := serviceContextError(ctx); err != nil {
				return nil, err
			}
			return nil, newHirozError(ErrorCode(rc), "service call failed")
		}
		result := C.GoBytes(unsafe.Pointer(data), C.int(size))
		C.hiroz_free_bytes(data, size)
		if err := serviceContextError(ctx); err != nil {
			return nil, err
		}
		return result, nil
	}
}

func (c *ServiceClient) checkService(wait time.Duration) error {
	c.mu.RLock()
	defer c.mu.RUnlock()
	hw := c.handle.Load()
	if hw == nil {
		return fmt.Errorf("service client is closed")
	}
	rc := C.hiroz_service_client_wait_for_service(hw.p, C.uint64_t(durationMillis(wait)))
	if rc != 0 {
		return newHirozError(ErrorCode(rc), "service is not available")
	}
	return nil
}

// WaitForServiceWithContext waits for discovery or cancellation, without fixed sleeps.
func (c *ServiceClient) WaitForServiceWithContext(ctx context.Context) error {
	for {
		if err := serviceContextError(ctx); err != nil {
			return err
		}
		wait := 20 * time.Millisecond
		if deadline, ok := ctx.Deadline(); ok {
			if left := time.Until(deadline); left < wait {
				if left <= 0 {
					return context.DeadlineExceeded
				}
				wait = left
			}
		}
		err := c.checkService(wait)
		if contextErr := serviceContextError(ctx); contextErr != nil {
			return contextErr
		}
		if err == nil {
			return nil
		}
		if e, ok := err.(HirozError); !ok || e.Code() != ErrorCodeServiceTimeout {
			return err
		}
	}
}

// CallTypedWithContext deserializes a context-aware service response into an allocated destination.
func CallTypedWithContext[Resp Message](ctx context.Context, client *ServiceClient, request Message, response Resp) error {
	data, err := client.CallWithContext(ctx, request)
	if err != nil {
		return err
	}
	if err = response.DeserializeCDR(data); err != nil {
		return newHirozError(ErrorCodeDeserializationFailed, fmt.Sprintf("deserialize response: %v", err))
	}
	return nil
}
