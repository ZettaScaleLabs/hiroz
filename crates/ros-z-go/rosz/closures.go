package rosz

// #include <stdint.h>
import "C"
import (
	"runtime"
	"runtime/cgo"
)

// closureContext holds a pinned Go closure that can be safely passed to C callbacks.
// The context is pinned using runtime.Pinner to prevent GC relocation, and callbacks
// are stored as cgo.Handle values for type-safe GC-integrated reference management.
//
// This pattern is based on zenoh-go's closure system and replaces the manual
// callback registry (sync.RWMutex + map) with Go 1.17+ cgo.Handle.
type closureContext[T any] struct {
	onCall     C.uintptr_t // cgo.Handle storing func(T)
	onDrop     C.uintptr_t // cgo.Handle storing func() (optional)
	pinner     C.uintptr_t // cgo.Handle storing *runtime.Pinner
	selfHandle C.uintptr_t // cgo.Handle storing *closureContext[T], used as userData in C callbacks
}

// call invokes the stored callback function with the given value.
// This is called from C via the trampoline functions.
func (context *closureContext[T]) call(value T) {
	cgo.Handle(context.onCall).Value().(func(T))(value)
}

// drop cleans up the closure context.
// Calls the optional drop function, deletes all cgo.Handles, and unpins the context.
// This is called from C when the entity is destroyed.
func (context *closureContext[T]) drop() {
	if C.uintptr_t(context.onDrop) != 0 {
		cgo.Handle(context.onDrop).Value().(func())()
		cgo.Handle(context.onDrop).Delete()
	}
	cgo.Handle(context.onCall).Delete()
	cgo.Handle(context.selfHandle).Delete()
	cgo.Handle(context.pinner).Value().(*runtime.Pinner).Unpin()
	cgo.Handle(context.pinner).Delete()
}

// newClosure creates a new closure context with the given callback and optional drop function.
// The context is pinned to prevent GC relocation, and the callback/drop functions are stored
// as cgo.Handle values for safe cross-language reference management.
//
// The returned pointer is safe to pass to C and will remain valid until drop() is called.
func newClosure[T any](callback func(T), drop func()) *closureContext[T] {
	closure := closureContext[T]{}
	closure.onCall = C.uintptr_t(cgo.NewHandle(callback))
	if drop != nil {
		closure.onDrop = C.uintptr_t(cgo.NewHandle(drop))
	}
	contextPinner := &runtime.Pinner{}
	contextPinner.Pin(&closure)
	closure.pinner = C.uintptr_t(cgo.NewHandle(contextPinner))
	// selfHandle is used as userData in C callbacks; recover via cgo.Handle(userData).Value()
	closure.selfHandle = C.uintptr_t(cgo.NewHandle(&closure))
	return &closure
}
