package hiroz

/*
#include "hiroz_ffi.h"
*/
import "C"

// ABIVersion returns the C ABI version of the linked libhiroz.
//
// Zero means the library predates hiroz_abi_version().
func ABIVersion() uint32 {
	return uint32(C.hiroz_abi_version())
}
