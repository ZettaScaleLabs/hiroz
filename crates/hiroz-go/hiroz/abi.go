package hiroz

/*
#include "hiroz_ffi.h"
*/
import "C"

// ABIVersion returns the breaking-change counter of the linked libhiroz.
//
// The value changes only when an existing C symbol, struct layout or error code value changes incompatibly.
// An addition never changes it. Detect an added symbol by probing for it, not by comparing numbers.
// The library must export hiroz_abi_version().
// Linking against a library that lacks it fails with an undefined symbol,
// so this function never returns 0.
func ABIVersion() uint32 {
	return uint32(C.hiroz_abi_version())
}
