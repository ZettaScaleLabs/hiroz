//go:build integration
// +build integration

package interop_tests

// Go ↔ Rust ros-z interop tests.
//
// These tests spawn compiled Rust ros-z example binaries and verify that the
// Go FFI layer interoperates correctly with the native Rust implementation —
// without going through the ROS 2 rmw layer.
//
// # Why this matters
//
// Go↔Go tests use the same FFI library on both sides, so a bug that affects
// both paths equally could go unnoticed.  Go↔Rust tests exercise the full
// serialization and protocol stack between two independent implementations.
//
// # Prerequisites
//
// The Rust examples must be built before running these tests:
//
//	just -f crates/ros-z-go/justfile build-rust
//	cargo build --release --example z_pubsub --example z_srvcli
//
// # How Rust processes are configured
//
// The Rust binaries use ZContextBuilder, which reads the ROSZ_CONFIG_OVERRIDE
// environment variable (key=value;key=value format) to override config keys on
// top of the default ROS session config.  Each test injects this variable to
// point the Rust process at the per-test Zenoh router.

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/ZettaScaleLabs/ros-z/crates/ros-z-go/generated/example_interfaces"
	"github.com/ZettaScaleLabs/ros-z/crates/ros-z-go/generated/std_msgs"
	"github.com/ZettaScaleLabs/ros-z/crates/ros-z-go/rosz"
)

// rustExampleBinary returns the path to a compiled Rust example binary.
// Returns ("", false) if the binary does not exist — tests skip rather than fail
// when Rust examples have not been built.
func rustExampleBinary(name string) (string, bool) {
	// Walk up from the interop_tests directory to find the workspace root.
	// The package lives at crates/ros-z-go/interop_tests/, so workspace root
	// is three levels up.
	dir, err := filepath.Abs(".")
	if err != nil {
		return "", false
	}
	// go test sets CWD to the package directory
	root := filepath.Join(dir, "../../..")
	bin := filepath.Join(root, "target/release/examples", name)
	info, err := os.Stat(bin)
	if err != nil || info.IsDir() {
		return "", false
	}
	return bin, true
}

// rustEnv returns the environment for a Rust ros-z process, injecting
// ROSZ_CONFIG_OVERRIDE so it connects to the per-test Zenoh router.
func rustEnv(router *ZenohRouter) []string {
	override := fmt.Sprintf(
		`mode="client";connect/endpoints=["tcp/127.0.0.1:%d"];scouting/multicast/enabled=false`,
		router.port,
	)
	env := os.Environ()
	// Replace any existing ROSZ_CONFIG_OVERRIDE
	filtered := env[:0]
	for _, e := range env {
		if !strings.HasPrefix(e, "ROSZ_CONFIG_OVERRIDE=") {
			filtered = append(filtered, e)
		}
	}
	return append(filtered, "ROSZ_CONFIG_OVERRIDE="+override)
}

// TestGoPublisherToRustSubscriber publishes from Go and verifies a Rust
// subscriber (z_pubsub --role listener) receives the messages.
func TestGoPublisherToRustSubscriber(t *testing.T) {
	bin, ok := rustExampleBinary("z_pubsub")
	if !ok {
		t.Skip("z_pubsub binary not found — run: cargo build --release --example z_pubsub")
	}

	router := startZenohRouter(t)
	time.Sleep(500 * time.Millisecond)

	// Start Rust subscriber (client mode so it routes through the test router)
	rustCmd := exec.Command(bin,
		"--role", "listener",
		"--topic", "/chatter",
		"--mode", "client",
		"--endpoint", fmt.Sprintf("tcp/127.0.0.1:%d", router.port),
	)
	rustCmd.Env = rustEnv(router)

	var rustOut strings.Builder
	rustCmd.Stdout = &rustOut
	rustCmd.Stderr = &rustOut

	if err := rustCmd.Start(); err != nil {
		t.Fatalf("Failed to start z_pubsub listener: %v", err)
	}
	defer func() {
		rustCmd.Process.Kill()
		rustCmd.Wait()
	}()

	time.Sleep(time.Second) // wait for Rust subscriber to start and discover

	// Create Go publisher
	ctx, err := rosz.NewContext().
		WithConnectEndpoints(router.Endpoint()).DisableMulticastScouting().
		Build()
	if err != nil {
		t.Fatalf("Failed to create context: %v", err)
	}
	defer ctx.Close()

	node, err := ctx.CreateNode("go_publisher_rust_sub_test").Build()
	if err != nil {
		t.Fatalf("Failed to create node: %v", err)
	}
	defer node.Close()

	pub, err := node.CreatePublisher("/chatter").Build(&std_msgs.String{})
	if err != nil {
		t.Fatalf("Failed to create publisher: %v", err)
	}
	defer pub.Close()

	time.Sleep(500 * time.Millisecond) // discovery

	// Publish messages
	const wantMsg = "hello-from-go-to-rust"
	for i := 0; i < 5; i++ {
		msg := &std_msgs.String{Data: wantMsg}
		if err := pub.Publish(msg); err != nil {
			t.Errorf("Publish failed: %v", err)
		}
		time.Sleep(200 * time.Millisecond)
	}

	time.Sleep(500 * time.Millisecond) // let last messages flush

	// Kill the Rust process and collect output
	rustCmd.Process.Kill()
	rustCmd.Wait()

	output := rustOut.String()
	t.Logf("Rust subscriber output:\n%s", output)

	if !strings.Contains(output, wantMsg) {
		t.Errorf("Rust subscriber did not receive %q. Output:\n%s", wantMsg, output)
	}
}

// TestRustPublisherToGoSubscriber starts a Rust publisher (z_pubsub --role talker)
// and verifies the Go subscriber receives messages.
func TestRustPublisherToGoSubscriber(t *testing.T) {
	bin, ok := rustExampleBinary("z_pubsub")
	if !ok {
		t.Skip("z_pubsub binary not found — run: cargo build --release --example z_pubsub")
	}

	router := startZenohRouter(t)
	time.Sleep(500 * time.Millisecond)

	// Create Go subscriber first
	ctx, err := rosz.NewContext().
		WithConnectEndpoints(router.Endpoint()).DisableMulticastScouting().
		Build()
	if err != nil {
		t.Fatalf("Failed to create context: %v", err)
	}
	defer ctx.Close()

	node, err := ctx.CreateNode("go_subscriber_rust_pub_test").Build()
	if err != nil {
		t.Fatalf("Failed to create node: %v", err)
	}
	defer node.Close()

	var mu sync.Mutex
	var received []string
	done := make(chan struct{})

	_, err = node.CreateSubscriber("/chatter").BuildWithCallback(&std_msgs.String{}, func(data []byte) {
		msg := &std_msgs.String{}
		if err := msg.DeserializeCDR(data); err != nil {
			t.Logf("deserialize error: %v", err)
			return
		}
		mu.Lock()
		defer mu.Unlock()
		received = append(received, msg.Data)
		t.Logf("Go received from Rust: %s", msg.Data)
		if len(received) >= 3 {
			select {
			case <-done:
			default:
				close(done)
			}
		}
	})
	if err != nil {
		t.Fatalf("Failed to create subscriber: %v", err)
	}

	time.Sleep(time.Second) // subscriber ready

	// Start Rust publisher
	rustCmd := exec.Command(bin,
		"--role", "talker",
		"--topic", "/chatter",
		"--mode", "client",
		"--endpoint", fmt.Sprintf("tcp/127.0.0.1:%d", router.port),
		"--data", "hello-from-rust-to-go",
		"--period", "0.3",
	)
	rustCmd.Env = rustEnv(router)
	rustCmd.Stdout = os.Stderr // forward to test output
	rustCmd.Stderr = os.Stderr

	if err := rustCmd.Start(); err != nil {
		t.Fatalf("Failed to start z_pubsub talker: %v", err)
	}
	defer func() {
		rustCmd.Process.Kill()
		rustCmd.Wait()
	}()

	select {
	case <-done:
		t.Logf("Received %d messages from Rust publisher", len(received))
	case <-time.After(15 * time.Second):
		mu.Lock()
		count := len(received)
		mu.Unlock()
		t.Errorf("Timeout: received %d/3 messages from Rust publisher", count)
	}

	mu.Lock()
	defer mu.Unlock()
	for _, msg := range received {
		if !strings.Contains(msg, "hello-from-rust-to-go") {
			t.Errorf("Unexpected message content: %q", msg)
		}
	}
}

// TestGoServiceClientToRustServer creates a Rust service server (z_srvcli --mode server)
// and calls it from a Go service client.
func TestGoServiceClientToRustServer(t *testing.T) {
	bin, ok := rustExampleBinary("z_srvcli")
	if !ok {
		t.Skip("z_srvcli binary not found — run: cargo build --release --example z_srvcli")
	}

	router := startZenohRouter(t)

	// Start Rust service server, connected to the test router via --endpoint
	rustCmd := exec.Command(bin,
		"--mode", "server",
		"--zenoh-mode", "client",
		"--endpoint", router.Endpoint(),
	)
	rustCmd.Stdout = os.Stderr
	rustCmd.Stderr = os.Stderr

	if err := rustCmd.Start(); err != nil {
		t.Fatalf("Failed to start z_srvcli server: %v", err)
	}
	defer func() {
		rustCmd.Process.Kill()
		rustCmd.Wait()
	}()

	// Create Go client context pointing at the test router
	goCtx, err := rosz.NewContext().
		WithConnectEndpoints(router.Endpoint()).DisableMulticastScouting().
		Build()
	if err != nil {
		t.Fatalf("Failed to create context: %v", err)
	}
	defer goCtx.Close()

	goNode, err := goCtx.CreateNode("go_service_client_rust_test").Build()
	if err != nil {
		t.Fatalf("Failed to create node: %v", err)
	}
	defer goNode.Close()

	svc := &example_interfaces.AddTwoInts{}
	client, err := goNode.CreateServiceClient("add_two_ints").Build(svc)
	if err != nil {
		t.Fatalf("Failed to create service client: %v", err)
	}
	defer client.Close()

	// Retry until Rust server is reachable (deterministic readiness check)
	req := &example_interfaces.AddTwoIntsRequest{A: 12, B: 30}
	deadline := time.Now().Add(30 * time.Second)
	var resp example_interfaces.AddTwoIntsResponse
	for {
		err = rosz.CallTyped(client, req, &resp)
		if err == nil {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("Service call failed after 30s: %v", err)
		}
		time.Sleep(200 * time.Millisecond)
	}

	if resp.Sum != 42 {
		t.Errorf("Expected sum=42, got %d", resp.Sum)
	}
	t.Logf("Go called Rust server: 12 + 30 = %d", resp.Sum)
}

// TestRustServiceClientToGoServer creates a Go service server and calls it from
// a Rust client (z_srvcli --mode client).
func TestRustServiceClientToGoServer(t *testing.T) {
	bin, ok := rustExampleBinary("z_srvcli")
	if !ok {
		t.Skip("z_srvcli binary not found — run: cargo build --release --example z_srvcli")
	}

	router := startZenohRouter(t)

	// Create Go service server context pointing at the test router
	goCtx, err := rosz.NewContext().
		WithConnectEndpoints(router.Endpoint()).DisableMulticastScouting().
		Build()
	if err != nil {
		t.Fatalf("Failed to create context: %v", err)
	}
	defer goCtx.Close()

	goNode, err := goCtx.CreateNode("go_service_server_rust_client_test").Build()
	if err != nil {
		t.Fatalf("Failed to create node: %v", err)
	}
	defer goNode.Close()

	svc := &example_interfaces.AddTwoInts{}
	server, err := goNode.CreateServiceServer("add_two_ints").Build(svc,
		func(reqBytes []byte) ([]byte, error) {
			var req example_interfaces.AddTwoIntsRequest
			if err := req.DeserializeCDR(reqBytes); err != nil {
				return nil, err
			}
			t.Logf("Go server received: %d + %d", req.A, req.B)
			resp := &example_interfaces.AddTwoIntsResponse{Sum: req.A + req.B}
			return resp.SerializeCDR()
		},
	)
	if err != nil {
		t.Fatalf("Failed to create service server: %v", err)
	}
	defer server.Close()

	// Verify server is ready with a Go self-call before invoking Rust binary
	selfClient, err := goNode.CreateServiceClient("add_two_ints").Build(svc)
	if err != nil {
		t.Fatalf("Failed to create self-check client: %v", err)
	}
	defer selfClient.Close()

	deadline := time.Now().Add(15 * time.Second)
	for {
		err := rosz.CallTyped(selfClient, &example_interfaces.AddTwoIntsRequest{A: 1, B: 1}, &example_interfaces.AddTwoIntsResponse{})
		if err == nil {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("Go service server not ready after 15s: %v", err)
		}
		time.Sleep(100 * time.Millisecond)
	}

	// Run Rust client with --a 7 --b 8, connected to the test router via --endpoint
	rustCmd := exec.Command(bin,
		"--mode", "client",
		"--zenoh-mode", "client",
		"--endpoint", router.Endpoint(),
		"--a", "7", "--b", "8",
	)
	out, err := rustCmd.CombinedOutput()
	if err != nil {
		t.Fatalf("Rust client failed: %v\nOutput: %s", err, out)
	}

	output := string(out)
	t.Logf("Rust client output: %s", output)

	// z_srvcli prints "Received response: <sum>"
	if !strings.Contains(output, "15") {
		t.Errorf("Expected Rust client to print sum=15, got: %s", output)
	}
}
