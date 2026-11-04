package hiroz

import "testing"

func TestABIVersion(t *testing.T) {
	// Update this literal in the same change as any bump of the library's ABI_VERSION.
	// Zero is reserved and never a real version.
	const want = 1
	if v := ABIVersion(); v != want {
		t.Fatalf("ABIVersion() = %d, want %d", v, want)
	}
}
