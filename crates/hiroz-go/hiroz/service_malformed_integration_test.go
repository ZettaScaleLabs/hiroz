package hiroz

import (
	"bytes"
	"context"
	"fmt"
	"net"
	"os"
	"os/exec"
	"sync/atomic"
	"testing"
	"time"
)

// The fixture is a Rust integration-test executable, built with:
// cargo test -p hiroz --features ffi --test ffi_service_malformed_query --no-run
// Set HIROZ_SERVICE_MALFORMED_FIXTURE to its path before running this test.
func TestServiceCallbackSurvivesMalformedEnvelope(t *testing.T) {
	fixture := os.Getenv("HIROZ_SERVICE_MALFORMED_FIXTURE")
	if fixture == "" {
		t.Skip("set HIROZ_SERVICE_MALFORMED_FIXTURE to the native wire fixture executable")
	}
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	endpoint := "tcp/" + listener.Addr().String()
	listener.Close()
	configuration := fmt.Sprintf(`{"listen/endpoints":[%q],"connect/endpoints":[],"scouting/gossip/enabled":false}`, endpoint)
	ctx, err := NewContext().WithMode(ModeRouter).DisableMulticastScouting().WithJSON(configuration).Build()
	if err != nil {
		t.Fatal(err)
	}
	defer ctx.Close()
	node, err := ctx.CreateNode("go_malformed_service").Build()
	if err != nil {
		t.Fatal(err)
	}
	defer node.Close()
	var callbacks atomic.Int32
	server, err := node.CreateServiceServer("/malformed_service").Build(&liveService{}, func(data []byte) ([]byte, error) {
		callbacks.Add(1)
		return data, nil
	})
	if err != nil {
		t.Fatal(err)
	}
	defer server.Close()
	client, err := node.CreateServiceClient("/malformed_service").Build(&liveService{})
	if err != nil {
		t.Fatal(err)
	}
	defer client.Close()
	if err := client.WaitForService(2 * time.Second); err != nil {
		t.Fatal(err)
	}

	fixtureCtx, cancel := context.WithTimeout(context.Background(), 12*time.Second)
	defer cancel()
	command := exec.CommandContext(fixtureCtx, fixture, "--exact", "send_malformed_service_query", "--nocapture")
	command.Env = append(os.Environ(),
		"HIROZ_SERVICE_MALFORMED_ENDPOINT="+endpoint,
		"HIROZ_SERVICE_MALFORMED_SELECTOR=0/malformed_service/test::srv::dds_::Raw_/RIHS01_0000000000000000000000000000000000000000000000000000000000000000",
	)
	if output, err := command.CombinedOutput(); err != nil {
		t.Fatalf("native malformed-query fixture failed: %v\n%s", err, output)
	}
	data := []byte{0, 1, 0, 0, 7}
	response, err := client.CallRawWithTimeout(data, 2*time.Second)
	if err != nil || !bytes.Equal(response, data) {
		t.Fatalf("valid request after malformed envelope: response=%v error=%v", response, err)
	}
	if got := callbacks.Load(); got != 1 {
		t.Fatalf("callbacks=%d, want only the valid request", got)
	}
}
