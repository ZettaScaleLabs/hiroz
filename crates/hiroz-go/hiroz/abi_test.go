package hiroz

import "testing"

func TestABIVersion(t *testing.T) {
	// Zero means the linked library predates hiroz_abi_version().
	if v := ABIVersion(); v < 1 {
		t.Fatalf("ABIVersion() = %d, want >= 1", v)
	}
}
