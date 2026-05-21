package rosz

/*
#include <stdlib.h>
#include "ros_z_ffi.h"
*/
import "C"
import (
	"fmt"
	"runtime"
	"unsafe"
)

// serializeMessage serializes raw bytes to CDR using Rust.
// This is an internal helper; message types implement SerializeCDR directly.
func serializeMessage(typeName string, raw []byte) ([]byte, error) {
	if len(raw) == 0 {
		return nil, fmt.Errorf("cannot serialize empty input for %s", typeName)
	}

	typeNameC := C.CString(typeName)
	defer C.free(unsafe.Pointer(typeNameC))

	// Pin raw[0] so the GC does not move it during the C call.
	pinner := &runtime.Pinner{}
	defer pinner.Unpin()
	pinner.Pin(&raw[0])

	var outPtr *C.uint8_t
	var outLen C.size_t

	result := C.ros_z_serialize(
		typeNameC,
		(*C.uint8_t)(unsafe.Pointer(&raw[0])),
		C.size_t(len(raw)),
		&outPtr,
		&outLen,
	)

	if result != 0 {
		return nil, fmt.Errorf("serialization failed with code %d", result)
	}

	// Copy to Go slice and free C memory
	goBytes := C.GoBytes(unsafe.Pointer(outPtr), C.int(outLen))
	C.ros_z_free_bytes(outPtr, outLen)

	return goBytes, nil
}

// deserializeMessage deserializes CDR bytes to raw format using Rust.
// This is an internal helper; message types implement DeserializeCDR directly.
func deserializeMessage(typeName string, cdr []byte) ([]byte, error) {
	if len(cdr) == 0 {
		return nil, fmt.Errorf("cannot deserialize empty CDR data for %s", typeName)
	}

	typeNameC := C.CString(typeName)
	defer C.free(unsafe.Pointer(typeNameC))

	// Pin cdr[0] so the GC does not move it during the C call.
	pinner := &runtime.Pinner{}
	defer pinner.Unpin()
	pinner.Pin(&cdr[0])

	var outPtr *C.uint8_t
	var outLen C.size_t

	result := C.ros_z_deserialize(
		typeNameC,
		(*C.uint8_t)(unsafe.Pointer(&cdr[0])),
		C.size_t(len(cdr)),
		&outPtr,
		&outLen,
	)

	if result != 0 {
		return nil, fmt.Errorf("deserialization failed with code %d", result)
	}

	goBytes := C.GoBytes(unsafe.Pointer(outPtr), C.int(outLen))
	C.ros_z_free_bytes(outPtr, outLen)

	return goBytes, nil
}
