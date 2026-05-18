use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration;

use hiroz::Builder;
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use zenoh::Wait;
use zenoh::config::WhatAmI;

/// Helper to manage background processes with automatic cleanup
#[allow(dead_code)]
pub struct ProcessGuard {
    pub child: Option<Child>,
    name: String,
}

#[allow(dead_code)]
impl ProcessGuard {
    pub fn new(child: Child, name: &str) -> Self {
        println!("Started process: {}", name);
        Self {
            child: Some(child),
            name: name.to_string(),
        }
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let pid = child.id() as i32;
            // Negative PID targets the process group
            let pgid = Pid::from_raw(-pid);

            println!("Stopping process group: {}", self.name);

            // 1. Send SIGINT to the whole process group
            // This ensures both the ros2 CLI wrapper and the actual node receive the signal
            if let Err(e) = signal::kill(pgid, Signal::SIGINT) {
                eprintln!("Failed to send SIGINT to group {}: {}", self.name, e);
                // Fallback: try killing just the parent handle we have
                let _ = child.kill();
            }

            // 2. Wait for graceful shutdown with a timeout
            let start = std::time::Instant::now();
            let timeout = Duration::from_secs(5);

            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        println!(
                            "Process {} exited gracefully with status: {:?}",
                            self.name, status
                        );
                        return;
                    }
                    Ok(None) => {
                        if start.elapsed() > timeout {
                            eprintln!(
                                "Timeout reached for {}, sending SIGKILL to group",
                                self.name
                            );
                            // 3. Force kill the group if it's still running
                            let _ = signal::kill(pgid, Signal::SIGKILL);
                            let _ = child.wait(); // Clean up zombie
                            break;
                        }
                        thread::sleep(Duration::from_millis(100));
                    }
                    Err(e) => {
                        eprintln!("Error waiting for process {}: {}", self.name, e);
                        let _ = signal::kill(pgid, Signal::SIGKILL);
                        let _ = child.wait();
                        break;
                    }
                }
            }
        }
    }
}

/// Per-test Zenoh router configuration
pub struct TestRouter {
    #[allow(dead_code)]
    pub port: u16,
    pub endpoint: String,
    _session: zenoh::Session,
}

impl TestRouter {
    /// Start a new Zenoh router session on a free OS-assigned port.
    ///
    /// Binds a TCP listener to `127.0.0.1:0`, reads back the assigned port,
    /// then drops the listener before handing the port to Zenoh. This avoids
    /// PID-derived port collisions when multiple test binaries run in parallel.
    pub fn new() -> Self {
        // Ask the OS for a free port, release it, then let Zenoh bind it.
        // There is an inherent TOCTOU race between dropping the listener and
        // Zenoh binding the same port. We retry up to 5 times to handle the
        // rare case where another process wins the race.
        for attempt in 0..5u32 {
            let port = {
                let listener =
                    std::net::TcpListener::bind("127.0.0.1:0").expect("Failed to bind port 0");
                listener.local_addr().unwrap().port()
            };

            let endpoint = format!("tcp/127.0.0.1:{}", port);
            println!(
                "Starting Zenoh router on port {} (attempt {})...",
                port,
                attempt + 1
            );

            let mut config = zenoh::Config::default();
            config.set_mode(Some(WhatAmI::Router)).unwrap();
            config
                .insert_json5("listen/endpoints", &format!("[\"{}\"]", endpoint))
                .unwrap();
            config
                .insert_json5("scouting/multicast/enabled", "false")
                .unwrap();
            // Disable gateway.south so the router doesn't apply the South-region
            // optimization that sets subscriber_interest_finalized on publisher faces.
            // With gateway.south:auto (the zenoh 1.9.0 default), the router classifies
            // all connecting sessions as South and uses client-hat routing which can
            // suppress routing from zenoh-c 1.6.2 publishers to 1.9.0 client subscribers.
            let _ = config.insert_json5("gateway/south", "null");

            match zenoh::open(config).wait() {
                Ok(session) => {
                    thread::sleep(Duration::from_millis(500));
                    println!("Zenoh router ready on {}", endpoint);
                    return Self {
                        port,
                        endpoint,
                        _session: session,
                    };
                }
                Err(e) => {
                    println!("Port {} unavailable ({}), retrying...", port, e);
                }
            }
        }
        panic!("Failed to open Zenoh router session after 5 attempts");
    }

    /// Get the endpoint string for this router
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Get environment variable override for RMW Zenoh
    /// Uses key=value format expected by rmw_zenoh_cpp (NOT JSON5)
    #[allow(dead_code)]
    pub fn rmw_zenoh_env(&self) -> String {
        format!(
            "connect/endpoints=[\"tcp/127.0.0.1:{}\"];scouting/multicast/enabled=false",
            self.port
        )
    }
}

/// Create a hiroz context configured to connect to a specific Zenoh router
#[allow(dead_code)]
pub fn create_hiroz_context_with_router(
    router: &TestRouter,
) -> hiroz::Result<hiroz::context::ZContext> {
    create_hiroz_context_with_endpoint(router.endpoint())
}

/// Create a hiroz context configured to connect to a specific endpoint
pub fn create_hiroz_context_with_endpoint(
    endpoint: &str,
) -> hiroz::Result<hiroz::context::ZContext> {
    use hiroz::{Builder, context::ZContextBuilder};

    ZContextBuilder::default()
        .disable_multicast_scouting()
        .with_connect_endpoints([endpoint])
        .with_mode("client")
        .with_logging_enabled()
        .build()
}

/// Helper to wait for nodes to be ready
#[allow(dead_code)]
pub fn wait_for_ready(duration: Duration) {
    thread::sleep(duration);
}

/// Deterministically wait for a service to be ready by polling with test requests
#[allow(dead_code)]
pub fn wait_for_service_ready(
    ctx: &hiroz::context::ZContext,
    service_name: &str,
    timeout: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let start_time = std::time::Instant::now();

    loop {
        // Try to create a client and send a test request
        if let Ok(node) = ctx.create_node("service_readiness_checker").build()
            && let Ok(client) = node
                .create_client::<protobuf_demo::Calculate>(service_name)
                .build()
        {
            // Try a simple test request (add 1 + 1 = 2)
            let test_request = protobuf_demo::CalculateRequest {
                a: 1.0,
                b: 1.0,
                operation: "add".to_string(),
            };

            let rt = tokio::runtime::Runtime::new()?;
            let result = rt.block_on(async {
                client
                    .call_with_timeout(&test_request, Duration::from_millis(500))
                    .await
            });

            if result.is_ok() {
                println!("Service '{}' is ready", service_name);
                return Ok(());
            }
        }

        // Check timeout
        if start_time.elapsed() >= timeout {
            return Err(format!(
                "Service '{}' did not become ready within {:?}",
                service_name, timeout
            )
            .into());
        }

        // Wait a bit before retrying
        thread::sleep(Duration::from_millis(100));
    }
}

/// Check if ros2 CLI is available
#[allow(dead_code)]
pub fn check_ros2_available() -> bool {
    Command::new("ros2").arg("--version").output().is_ok()
}

/// Check if demo_nodes_cpp package is available
#[allow(dead_code)]
pub fn check_demo_nodes_cpp_available() -> bool {
    Command::new("ros2")
        .args(["pkg", "prefix", "demo_nodes_cpp"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Check if action_tutorials_cpp package is available
#[allow(dead_code)]
pub fn check_action_tutorials_cpp_available() -> bool {
    Command::new("ros2")
        .args(["pkg", "prefix", "action_tutorials_cpp"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

// ============================================================================
// Python Interop Helpers
// ============================================================================

#[cfg(feature = "python-interop")]
use std::path::PathBuf;

/// Get the path to the Python executable in hiroz-py venv
#[cfg(feature = "python-interop")]
fn python_executable() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("hiroz-py/.venv/bin/python")
}

/// Get the path to a Python example script
#[cfg(feature = "python-interop")]
fn example_script(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("hiroz-py/examples")
        .join(name)
}

/// Check if Python venv is available for interop tests
#[cfg(feature = "python-interop")]
#[allow(dead_code)]
pub fn check_python_venv_available() -> bool {
    python_executable().exists()
}

/// Spawn Python topic_demo.py as talker (publisher)
#[cfg(feature = "python-interop")]
#[allow(dead_code)]
pub fn spawn_python_talker(endpoint: &str, topic: &str, count: u32) -> ProcessGuard {
    use std::os::unix::process::CommandExt;

    let child = Command::new(python_executable())
        .arg(example_script("topic_demo.py"))
        .args(["-r", "talker"])
        .args(["-e", endpoint])
        .args(["-t", topic])
        .args(["-c", &count.to_string()])
        .args(["--interval", "0.3"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .expect("Failed to spawn Python talker");

    ProcessGuard::new(child, "python_talker")
}

/// Spawn Python topic_demo.py as listener (subscriber)
#[cfg(feature = "python-interop")]
#[allow(dead_code)]
pub fn spawn_python_listener(endpoint: &str, topic: &str, timeout_sec: f32) -> ProcessGuard {
    use std::os::unix::process::CommandExt;

    let child = Command::new(python_executable())
        .arg(example_script("topic_demo.py"))
        .args(["-r", "listener"])
        .args(["-e", endpoint])
        .args(["-t", topic])
        .args(["--timeout", &timeout_sec.to_string()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .expect("Failed to spawn Python listener");

    ProcessGuard::new(child, "python_listener")
}

/// Spawn Python service_demo.py as server
#[cfg(feature = "python-interop")]
#[allow(dead_code)]
pub fn spawn_python_service_server(
    endpoint: &str,
    service_name: &str,
    max_requests: u32,
) -> ProcessGuard {
    use std::os::unix::process::CommandExt;

    let child = Command::new(python_executable())
        .arg(example_script("service_demo.py"))
        .args(["-r", "server"])
        .args(["-e", endpoint])
        .args(["-s", service_name])
        .args(["-c", &max_requests.to_string()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .expect("Failed to spawn Python service server");

    ProcessGuard::new(child, "python_service_server")
}

/// Spawn Python service_demo.py as client
#[cfg(feature = "python-interop")]
#[allow(dead_code)]
pub fn spawn_python_service_client(
    endpoint: &str,
    service_name: &str,
    a: i64,
    b: i64,
) -> ProcessGuard {
    use std::os::unix::process::CommandExt;

    let child = Command::new(python_executable())
        .arg(example_script("service_demo.py"))
        .args(["-r", "client"])
        .args(["-e", endpoint])
        .args(["-s", service_name])
        .args(["-a", &a.to_string()])
        .args(["-b", &b.to_string()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .expect("Failed to spawn Python service client");

    ProcessGuard::new(child, "python_service_client")
}

// ============================================================================
// ros2dds Backend Interop Helpers (with zenoh-bridge-ros2dds)
// ============================================================================

/// Check if zenoh-bridge-ros2dds is available
#[cfg(feature = "ros2dds-interop")]
#[allow(dead_code)]
pub fn check_zenoh_bridge_ros2dds_available() -> bool {
    Command::new("zenoh-bridge-ros2dds")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Spawn zenoh-bridge-ros2dds process
///
/// The bridge will:
/// - Act as a Zenoh router (default behavior)
/// - Bridge DDS traffic from ROS 2 nodes using CycloneDDS/FastDDS
/// - Listen on the default Zenoh port (7447) for peer connections
#[cfg(feature = "ros2dds-interop")]
#[allow(dead_code)]
pub fn spawn_zenoh_bridge_ros2dds() -> ProcessGuard {
    use std::os::unix::process::CommandExt;

    let child = Command::new("zenoh-bridge-ros2dds")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .expect("Failed to spawn zenoh-bridge-ros2dds");

    // Give the bridge time to start up
    thread::sleep(Duration::from_secs(2));

    ProcessGuard::new(child, "zenoh-bridge-ros2dds")
}

/// Spawn zenoh-bridge-ros2dds with a specific listen endpoint
#[cfg(feature = "ros2dds-interop")]
#[allow(dead_code)]
pub fn spawn_zenoh_bridge_ros2dds_with_endpoint(listen_endpoint: &str) -> ProcessGuard {
    use std::os::unix::process::CommandExt;

    let child = Command::new("zenoh-bridge-ros2dds")
        .args(["-l", listen_endpoint])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .expect("Failed to spawn zenoh-bridge-ros2dds");

    // Give the bridge time to start up
    thread::sleep(Duration::from_secs(2));

    ProcessGuard::new(child, "zenoh-bridge-ros2dds")
}

/// Spawn a ROS 2 demo_nodes_cpp talker using CycloneDDS (default DDS)
#[cfg(feature = "ros2dds-interop")]
#[allow(dead_code)]
pub fn spawn_ros2_cyclone_talker() -> ProcessGuard {
    use std::os::unix::process::CommandExt;

    let child = Command::new("ros2")
        .args(["run", "demo_nodes_cpp", "talker"])
        .env("RMW_IMPLEMENTATION", "rmw_cyclonedds_cpp")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .expect("Failed to spawn ROS 2 CycloneDDS talker");

    // Wait for the node to be ready
    thread::sleep(Duration::from_secs(2));

    ProcessGuard::new(child, "ros2_cyclone_talker")
}

/// Spawn a ROS 2 demo_nodes_cpp listener using CycloneDDS (default DDS)
#[cfg(feature = "ros2dds-interop")]
#[allow(dead_code)]
pub fn spawn_ros2_cyclone_listener() -> ProcessGuard {
    use std::os::unix::process::CommandExt;

    let child = Command::new("ros2")
        .args(["run", "demo_nodes_cpp", "listener"])
        .env("RMW_IMPLEMENTATION", "rmw_cyclonedds_cpp")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .expect("Failed to spawn ROS 2 CycloneDDS listener");

    // Wait for the node to be ready
    thread::sleep(Duration::from_secs(2));

    ProcessGuard::new(child, "ros2_cyclone_listener")
}

/// Create a hiroz context for ros2dds backend
///
/// This configures hiroz to use peer mode, which will discover
/// zenoh-bridge-ros2dds via multicast scouting.
#[cfg(feature = "ros2dds-interop")]
#[allow(dead_code)]
pub fn create_hiroz_context_ros2dds() -> hiroz::Result<hiroz::context::ZContext> {
    use hiroz::context::ZContextBuilder;

    ZContextBuilder::default()
        .with_mode("peer")
        .with_logging_enabled()
        .build()
}

/// Create a hiroz context for ros2dds backend with a specific endpoint
#[cfg(feature = "ros2dds-interop")]
#[allow(dead_code)]
pub fn create_hiroz_context_ros2dds_with_endpoint(
    endpoint: &str,
) -> hiroz::Result<hiroz::context::ZContext> {
    use hiroz::context::ZContextBuilder;

    ZContextBuilder::default()
        .with_mode("client")
        .with_connect_endpoints([endpoint])
        .disable_multicast_scouting()
        .with_logging_enabled()
        .build()
}

/// Spawn a ROS 2 add_two_ints_server using CycloneDDS
#[cfg(feature = "ros2dds-interop")]
#[allow(dead_code)]
pub fn spawn_ros2_cyclone_add_two_ints_server() -> ProcessGuard {
    use std::os::unix::process::CommandExt;

    let child = Command::new("ros2")
        .args(["run", "demo_nodes_cpp", "add_two_ints_server"])
        .env("RMW_IMPLEMENTATION", "rmw_cyclonedds_cpp")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .expect("Failed to spawn ROS 2 CycloneDDS add_two_ints_server");

    // Wait for the service server to be ready
    thread::sleep(Duration::from_secs(2));

    ProcessGuard::new(child, "ros2_cyclone_add_two_ints_server")
}

/// Spawn a ROS 2 add_two_ints_client using CycloneDDS
#[cfg(feature = "ros2dds-interop")]
#[allow(dead_code)]
pub fn spawn_ros2_cyclone_add_two_ints_client(a: i64, b: i64) -> ProcessGuard {
    use std::os::unix::process::CommandExt;

    let child = Command::new("ros2")
        .args(["run", "demo_nodes_cpp", "add_two_ints_client"])
        .args([&a.to_string(), &b.to_string()])
        .env("RMW_IMPLEMENTATION", "rmw_cyclonedds_cpp")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .expect("Failed to spawn ROS 2 CycloneDDS add_two_ints_client");

    ProcessGuard::new(child, "ros2_cyclone_add_two_ints_client")
}

// ============================================================================
// Humble ↔ Jazzy Bridge Test Helpers
// ============================================================================

#[cfg(feature = "humble-jazzy-bridge-tests")]
#[allow(dead_code)]
mod humble_jazzy {
    use super::*;

    /// Return the path to the `humble-ros2` wrapper binary.
    ///
    /// The wrapper is provided by the `ros-bridge-interop` nix dev shell and its
    /// path is exported as `HUMBLE_ROS2`.  Tests must run inside that shell.
    fn humble_ros2_bin() -> String {
        std::env::var("HUMBLE_ROS2").expect(
            "HUMBLE_ROS2 env var not set — run tests inside the `ros-bridge-interop` nix shell",
        )
    }

    /// Build the ZENOH_CONFIG_OVERRIDE string for a rmw_zenoh node to connect to `endpoint`.
    fn rmw_zenoh_override(endpoint: &str) -> String {
        format!("connect/endpoints=[\"{endpoint}\"];scouting/multicast/enabled=false")
    }

    /// Spawn a process using the `humble-ros2` wrapper with extra environment variables.
    fn spawn_humble(args: &[&str], env: &[(&str, &str)]) -> ProcessGuard {
        use std::os::unix::process::CommandExt;

        let bin = humble_ros2_bin();
        let name = args.join(" ");
        let mut command = Command::new(&bin);
        command
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        for (k, v) in env {
            command.env(k, v);
        }
        let child = command
            .spawn()
            .unwrap_or_else(|e| panic!("Failed to spawn humble-ros2 {name}: {e}"));
        ProcessGuard::new(child, &format!("humble-ros2 {name}"))
    }

    /// Spawn a ROS 2 Humble demo_nodes_cpp talker using rmw_zenoh.
    ///
    /// The talker publishes `std_msgs/String` on the given `topic`.
    pub fn spawn_humble_ros2_talker(endpoint: &str, topic: &str) -> ProcessGuard {
        spawn_humble(
            &[
                "run",
                "demo_nodes_cpp",
                "talker",
                "--ros-args",
                "-r",
                "__ns:=/",
                "-r",
                &format!("chatter:={topic}"),
            ],
            &[
                ("RMW_IMPLEMENTATION", "rmw_zenoh_cpp"),
                ("ZENOH_CONFIG_OVERRIDE", &rmw_zenoh_override(endpoint)),
            ],
        )
    }

    /// Spawn a ROS 2 Humble demo_nodes_cpp listener using rmw_zenoh.
    pub fn spawn_humble_ros2_listener(endpoint: &str, topic: &str) -> ProcessGuard {
        spawn_humble(
            &[
                "run",
                "demo_nodes_cpp",
                "listener",
                "--ros-args",
                "-r",
                &format!("chatter:={topic}"),
            ],
            &[
                ("RMW_IMPLEMENTATION", "rmw_zenoh_cpp"),
                ("ZENOH_CONFIG_OVERRIDE", &rmw_zenoh_override(endpoint)),
            ],
        )
    }

    /// Spawn a ROS 2 Humble add_two_ints_server using rmw_zenoh.
    pub fn spawn_humble_ros2_service_server(endpoint: &str) -> ProcessGuard {
        spawn_humble(
            &["run", "demo_nodes_cpp", "add_two_ints_server"],
            &[
                ("RMW_IMPLEMENTATION", "rmw_zenoh_cpp"),
                ("ZENOH_CONFIG_OVERRIDE", &rmw_zenoh_override(endpoint)),
            ],
        )
    }

    /// Spawn a ROS 2 Humble fibonacci action server using rmw_zenoh.
    pub fn spawn_humble_ros2_action_server(endpoint: &str) -> ProcessGuard {
        spawn_humble(
            &["run", "action_tutorials_cpp", "fibonacci_action_server"],
            &[
                ("RMW_IMPLEMENTATION", "rmw_zenoh_cpp"),
                ("ZENOH_CONFIG_OVERRIDE", &rmw_zenoh_override(endpoint)),
            ],
        )
    }

    /// Run `ros2 topic list` in the Jazzy environment connected to `endpoint`.
    ///
    /// Returns the list of topic names (e.g. `["/chatter", "/rosout"]`).
    /// Lines are filtered to keep only valid ROS topic names (start with `/`,
    /// no spaces, no dots).
    pub fn jazzy_topic_list(endpoint: &str) -> Vec<String> {
        let override_str = rmw_zenoh_override(endpoint);
        let output = Command::new("ros2")
            .args(["topic", "list", "--spin-time", "5", "--no-daemon"])
            .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
            .env("ZENOH_CONFIG_OVERRIDE", &override_str)
            .output()
            .expect("failed to run `ros2 topic list` — is ros2cli available in PATH?");
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| l.starts_with('/') && !l.contains(' ') && !l.contains('.'))
            .collect()
    }

    /// Run `ros2 topic list` in the Humble environment connected to `endpoint`.
    ///
    /// Returns the list of topic names.
    pub fn humble_topic_list(endpoint: &str) -> Vec<String> {
        let override_str = rmw_zenoh_override(endpoint);
        let bin = humble_ros2_bin();
        let output = Command::new(&bin)
            .args(["topic", "list", "--spin-time", "5", "--no-daemon"])
            .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
            .env("ZENOH_CONFIG_OVERRIDE", &override_str)
            .output()
            .expect("failed to run humble `ros2 topic list`");
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| l.starts_with('/') && !l.contains(' ') && !l.contains('.'))
            .collect()
    }

    /// Call the add_two_ints service once from a Humble node via rmw_zenoh, then exit.
    pub fn spawn_humble_ros2_service_client(endpoint: &str) -> ProcessGuard {
        spawn_humble(
            &[
                "service",
                "call",
                "/add_two_ints",
                "example_interfaces/srv/AddTwoInts",
                "{a: 3, b: 7}",
            ],
            &[
                ("RMW_IMPLEMENTATION", "rmw_zenoh_cpp"),
                ("ZENOH_CONFIG_OVERRIDE", &rmw_zenoh_override(endpoint)),
            ],
        )
    }

    /// Spawn the `hiroz-bridge` binary connecting both endpoints to the same router.
    ///
    /// Both `--humble-endpoint` and `--jazzy-endpoint` are set to `endpoint` since
    /// the test router is shared between both distros.
    pub fn spawn_bridge(endpoint: &str) -> ProcessGuard {
        use std::os::unix::process::CommandExt;

        // The bridge binary is built as part of the test workspace.
        let bridge_bin = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent() // crates/hiroz-tests -> crates
            .unwrap()
            .parent() // crates -> workspace root
            .unwrap()
            .join("target/debug/hiroz-bridge");

        let child = Command::new(&bridge_bin)
            .args(["--humble-endpoint", endpoint])
            .args(["--jazzy-endpoint", endpoint])
            .env("RUST_LOG", std::env::var("RUST_LOG").unwrap_or_default())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .process_group(0)
            .spawn()
            .unwrap_or_else(|e| {
                panic!(
                    "Failed to spawn hiroz-bridge ({}): {e}",
                    bridge_bin.display()
                )
            });

        // Give the bridge time to start discovery.
        thread::sleep(Duration::from_millis(500));
        ProcessGuard::new(child, "hiroz-bridge")
    }
}

#[cfg(feature = "humble-jazzy-bridge-tests")]
pub use humble_jazzy::*;
