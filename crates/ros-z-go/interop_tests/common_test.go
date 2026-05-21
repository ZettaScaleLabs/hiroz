//go:build integration
// +build integration

package interop_tests

import (
	"context"
	"flag"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"testing"
	"time"
)

// ZenohRouter manages a Zenoh router instance for tests
type ZenohRouter struct {
	cmd  *exec.Cmd
	port int
}

// startZenohRouter starts a Zenoh router on a random port.
// Uses zenohd if available, falls back to the project's zenoh_router example.
func startZenohRouter(t *testing.T) *ZenohRouter {
	t.Helper()

	bin, kind := zenohRouterBin()
	if bin == "" {
		t.Fatal("No Zenoh router binary found. Run: cargo build --release --example zenoh_router")
	}

	// Find available port (use process PID for uniqueness)
	port := 7447 + (os.Getpid() % 1000)

	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	t.Cleanup(cancel)

	var cmd *exec.Cmd
	switch kind {
	case "zenohd":
		cmd = exec.CommandContext(ctx, bin,
			"--cfg", fmt.Sprintf("listen/endpoints=[\"tcp/127.0.0.1:%d\"]", port),
			"--cfg", "scouting/multicast/enabled=false")
	default: // zenoh_router example — uses --listen flag
		cmd = exec.CommandContext(ctx, bin,
			"--listen", fmt.Sprintf("tcp/127.0.0.1:%d", port))
	}

	if err := cmd.Start(); err != nil {
		t.Fatalf("Failed to start Zenoh router (%s): %v", kind, err)
	}

	router := &ZenohRouter{
		cmd:  cmd,
		port: port,
	}

	// Wait for router to be ready
	time.Sleep(500 * time.Millisecond)

	t.Cleanup(func() {
		router.Stop()
	})

	t.Logf("Started Zenoh router on port %d (PID: %d)", port, cmd.Process.Pid)
	return router
}

// Stop stops the Zenoh router
func (r *ZenohRouter) Stop() {
	if r.cmd != nil && r.cmd.Process != nil {
		r.cmd.Process.Kill()
		r.cmd.Wait()
	}
}

// Endpoint returns the router endpoint
func (r *ZenohRouter) Endpoint() string {
	return fmt.Sprintf("tcp/127.0.0.1:%d", r.port)
}

// Config returns the Zenoh config string for ros-z-go
func (r *ZenohRouter) Config() string {
	return fmt.Sprintf("connect/endpoints=[\"tcp/127.0.0.1:%d\"];scouting/multicast/enabled=false", r.port)
}

// EnvVar returns the environment variable for RMW Zenoh
func (r *ZenohRouter) EnvVar() string {
	return fmt.Sprintf("ZENOH_CONFIG_OVERRIDE=connect/endpoints=[\"tcp/127.0.0.1:%d\"];scouting/multicast/enabled=false", r.port)
}

// checkROS2Available checks if ros2 CLI is on PATH.
// Uses LookPath instead of running ros2 to avoid RMW initialisation
// failures when RMW_IMPLEMENTATION is set but no router is running yet.
func checkROS2Available() bool {
	_, err := exec.LookPath("ros2")
	return err == nil
}

// zenohRouterBin returns the path to the Zenoh router binary.
// It prefers the system zenohd, falling back to the project's zenoh_router example.
func zenohRouterBin() (string, string) {
	if exec.Command("zenohd", "--version").Run() == nil {
		return "zenohd", "zenohd"
	}
	// Fall back to the compiled zenoh_router example from the workspace
	dir, err := filepath.Abs(".")
	if err != nil {
		return "", ""
	}
	root := filepath.Join(dir, "../../..")
	bin := filepath.Join(root, "target/release/examples/zenoh_router")
	if _, err := os.Stat(bin); err == nil {
		return bin, "zenoh_router"
	}
	return "", ""
}

// checkZenohAvailable checks if a Zenoh router binary is available
func checkZenohAvailable() bool {
	bin, _ := zenohRouterBin()
	return bin != ""
}

// waitForProcess waits for a process to start and be ready
func waitForProcess(d time.Duration) {
	time.Sleep(d)
}

// TestMain sets up test environment
func TestMain(m *testing.M) {
	flag.Parse()

	// Check if Zenoh is available
	if !checkZenohAvailable() {
		fmt.Println("ERROR: zenohd not found. Install Zenoh before running interop tests.")
		os.Exit(1)
	}

	// Set environment for tests
	os.Setenv("RUST_LOG", "warn")

	// go test -v activates full debug output from the Go bindings.
	if testing.Verbose() {
		os.Setenv("ROSZ_LOG", "DEBUG")
	}

	// Run tests
	code := m.Run()
	os.Exit(code)
}

// getAvailablePort returns an available TCP port
func getAvailablePort() int {
	// Simple implementation: use PID-based offset
	return 7447 + (os.Getpid() % 1000)
}

// getROS2Env returns ROS2 environment setup
func getROS2Env(router *ZenohRouter) []string {
	return []string{
		"RMW_IMPLEMENTATION=rmw_zenoh_cpp",
		router.EnvVar(),
	}
}

// extractInt extracts an integer from a string
func extractInt(s string) int {
	for i := range s {
		if s[i] >= '0' && s[i] <= '9' {
			num := ""
			for j := i; j < len(s) && s[j] >= '0' && s[j] <= '9'; j++ {
				num += string(s[j])
			}
			if val, err := strconv.Atoi(num); err == nil {
				return val
			}
		}
	}
	return 0
}
