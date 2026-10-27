package hiroz

/*
#include <stdlib.h>
#include "hiroz_ffi.h"
extern hiroz_SampleCallback getSampleCallback(void);
extern hiroz_SubscriberDropCallback getSampleDropCallback(void);
*/
import "C"
import (
	"context"
	"encoding/json"
	"fmt"
	"runtime"
	"runtime/cgo"
	"time"
	"unsafe"
)

// Locality restricts subscriber delivery by Zenoh session, not by Go node.
type Locality int32

const (
	LocalityAny Locality = iota
	LocalitySessionLocal
	LocalityRemote
)

// WithLocality restricts live delivery. Restricted locality with transient-local
// QoS returns an error at Build because Zenoh history cannot enforce it.
func (b *SubscriberBuilder) WithLocality(locality Locality) *SubscriberBuilder {
	b.locality = locality
	return b
}

// Sample owns a serialized payload and transport metadata. Nil Attachment means
// no attachment was received. SourceTimestamp is ROS source time in nanoseconds;
// Timestamp is the exact Zenoh timestamp string (empty when absent).
// Metadata pointers are nil if no valid ROS attachment was present.
type Sample struct {
	Data            []byte
	Attachment      []byte
	Encoding        string
	Timestamp       string
	SequenceNumber  *int64
	SourceTimestamp *int64
	PublisherGID    *[16]byte
	ReceivedAt      time.Time
}
type SampleHandler func(Sample)

// WithEncoding sets a transport MIME encoding. PublishSerialized/PublishSample
// accept bytes in that encoding; Publish(Message) always invokes SerializeCDR.
// This setting labels bytes and does not convert between formats.
func (b *PublisherBuilder) WithEncoding(encoding string) *PublisherBuilder {
	b.encoding = encoding
	return b
}

// WithoutSharedMemory disables application SHM allocation for this publisher.
// Transport-level SHM negotiated independently by Zenoh can still be active.
func (b *PublisherBuilder) WithoutSharedMemory() *PublisherBuilder { b.shmMode = 1; return b }

// WithSharedMemory allocates a private SHM pool. Payloads at or above threshold
// use the pool when allocation succeeds, otherwise ordinary memory. Go bytes
// are copied into SHM; receiving callbacks also copy into Go-owned slices.
func (b *PublisherBuilder) WithSharedMemory(poolBytes, threshold uint64) *PublisherBuilder {
	b.shmMode = 2
	b.shmPool = poolBytes
	b.shmThreshold = threshold
	return b
}

func (p *Publisher) PublishSerialized(data []byte) error { return p.PublishSample(Sample{Data: data}) }

// PublishSample preserves nonnil Attachment, Encoding and Timestamp. When no
// attachment is supplied, native code generates sequence/time/publisher identity.
func (p *Publisher) PublishSample(sample Sample) error {
	handle, release, err := p.acquire()
	if err != nil {
		return err
	}
	defer release()
	var data, attachment *C.uint8_t
	if len(sample.Data) > 0 {
		data = (*C.uint8_t)(unsafe.Pointer(&sample.Data[0]))
	}
	if sample.Attachment != nil {
		if len(sample.Attachment) > 0 {
			attachment = (*C.uint8_t)(unsafe.Pointer(&sample.Attachment[0]))
		} else {
			var zero byte
			attachment = (*C.uint8_t)(unsafe.Pointer(&zero))
		}
	}
	var encoding, timestamp *C.char
	if sample.Encoding != "" {
		encoding = C.CString(sample.Encoding)
		defer C.free(unsafe.Pointer(encoding))
	}
	if sample.Timestamp != "" {
		timestamp = C.CString(sample.Timestamp)
		defer C.free(unsafe.Pointer(timestamp))
	}
	rc := C.hiroz_publisher_publish_sample(handle, data, C.uintptr_t(len(sample.Data)), attachment, C.uintptr_t(len(sample.Attachment)), encoding, timestamp)
	if rc != 0 {
		return newHirozError(ErrorCode(rc), "serialized publication failed")
	}
	return nil
}

func (b *SubscriberBuilder) BuildWithSampleCallback(msg Message, handler SampleHandler) (*Subscriber, error) {
	return b.buildWithSampleCallback(msg, handler, nil)
}
func (b *SubscriberBuilder) buildWithSampleCallback(msg Message, handler SampleHandler, onClose func()) (*Subscriber, error) {
	node, release, err := b.node.acquire()
	if err != nil {
		return nil, err
	}
	defer release()
	if handler == nil || msg == nil {
		return nil, fmt.Errorf("message and callback required")
	}
	if b.locality < LocalityAny || b.locality > LocalityRemote {
		return nil, fmt.Errorf("invalid locality")
	}
	if b.locality != LocalityAny && b.qos != nil && b.qos.Durability == DurabilityTransientLocal {
		return nil, fmt.Errorf("restricted locality is unsupported with transient-local history")
	}
	if err := b.node.registerMessageSchema(msg); err != nil {
		return nil, err
	}
	a, bn, c := C.CString(b.topic), C.CString(msg.TypeName()), C.CString(msg.TypeHash())
	defer C.free(unsafe.Pointer(a))
	defer C.free(unsafe.Pointer(bn))
	defer C.free(unsafe.Pointer(c))
	closure := newClosure(func(s Sample) { handler(s) }, nil)
	var qos *C.hiroz_qos_profile_t
	if b.qos != nil {
		value := b.qos.toCQos()
		qos = &value
	}
	handle := C.hiroz_subscriber_create_samples(node, a, bn, c, C.getSampleCallback(), closure.selfHandle, qos, C.int32_t(b.locality), C.getSampleDropCallback())
	if handle == nil {
		return nil, fmt.Errorf("%w: subscriber for %s", ErrBuildFailed, b.topic)
	}
	sub := &Subscriber{handle: handle, node: b.node, onClose: onClose}
	runtime.SetFinalizer(sub, (*Subscriber).Close)
	if err := b.node.ownSubscriber(sub); err != nil {
		_ = sub.Close()
		return nil, err
	}
	return sub, nil
}

//export goSampleCallback
func goSampleCallback(userData C.uintptr_t, data *C.uint8_t, length C.uintptr_t, attachment *C.uint8_t, attachmentLen C.uintptr_t, metadata *C.char) {
	_ = safeCall(func() error {
		var sample Sample
		if err := json.Unmarshal([]byte(C.GoString(metadata)), &sample); err != nil {
			return err
		}
		sample.Data = append([]byte{}, unsafe.Slice((*byte)(unsafe.Pointer(data)), int(length))...)
		if attachment != nil {
			sample.Attachment = append([]byte{}, unsafe.Slice((*byte)(unsafe.Pointer(attachment)), int(attachmentLen))...)
		}
		sample.ReceivedAt = time.Now()
		cgo.Handle(userData).Value().(*closureContext[Sample]).call(sample)
		return nil
	})
}

//export goSampleDropCallback
func goSampleDropCallback(userData C.uintptr_t) {
	_ = safeCall(func() error { cgo.Handle(userData).Value().(*closureContext[Sample]).drop(); return nil })
}

type endpointState struct {
	Endpoint  EndpointInfo
	PeerCount int
}

func (p *Publisher) endpointState() (endpointState, error) {
	var state endpointState
	h, release, err := p.acquire()
	if err != nil {
		return state, err
	}
	defer release()
	err = decodeNativeJSON(C.hiroz_publisher_info_json(h), &state)
	return state, err
}
func (s *Subscriber) endpointState() (endpointState, error) {
	var state endpointState
	h, release, err := s.acquire()
	if err != nil {
		return state, err
	}
	defer release()
	err = decodeNativeJSON(C.hiroz_subscriber_info_json(h), &state)
	return state, err
}
func (p *Publisher) Info() (EndpointInfo, error) { s, err := p.endpointState(); return s.Endpoint, err }
func (s *Subscriber) Info() (EndpointInfo, error) {
	v, err := s.endpointState()
	return v.Endpoint, err
}

// SubscriptionCount counts discovered subscriptions; type/QoS compatibility and
// transport delivery are not implied by graph presence.
func (p *Publisher) SubscriptionCount() (int, error) {
	s, err := p.endpointState()
	return s.PeerCount, err
}
func (s *Subscriber) PublisherCount() (int, error) {
	v, err := s.endpointState()
	return v.PeerCount, err
}
func waitForEndpoint(ctx context.Context, count func() (int, error)) error {
	if ctx == nil {
		return fmt.Errorf("context is required")
	}
	timer := time.NewTicker(10 * time.Millisecond)
	defer timer.Stop()
	for {
		if err := ctx.Err(); err != nil {
			return err
		}
		n, err := count()
		if err != nil {
			return err
		}
		if n > 0 {
			return nil
		}
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-timer.C:
		}
	}
}
func (p *Publisher) WaitForSubscriptionContext(ctx context.Context) error {
	return waitForEndpoint(ctx, p.SubscriptionCount)
}
func (s *Subscriber) WaitForPublisherContext(ctx context.Context) error {
	return waitForEndpoint(ctx, s.PublisherCount)
}
func (p *Publisher) WaitForSubscription(timeout time.Duration) error {
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	return p.WaitForSubscriptionContext(ctx)
}
func (s *Subscriber) WaitForPublisher(timeout time.Duration) error {
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	return s.WaitForPublisherContext(ctx)
}
