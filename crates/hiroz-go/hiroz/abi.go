package hiroz

/*
#include "hiroz_ffi.h"
*/
import "C"

// ABIVersion returns the C ABI version of the linked libhiroz.
//
// The library must export hiroz_abi_version().
// Linking against a library that lacks it fails with an undefined symbol,
// so this function never returns 0.
func ABIVersion() uint32 {
	return uint32(C.hiroz_abi_version())
}
