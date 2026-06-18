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
            // Disable gateway.south: set to empty custom list so no sessions are
            // classified as South. With the default "auto" preset, the router
            // classifies all connecting sessions as South and applies client-hat
            // routing, which suppresses routing from zenoh-c 1.6.2 publishers to
            // zenoh-rs 1.9.0 client subscribers. Setting to null falls back to the
            // default (unwrap_or_default → Auto), so we must use [] instead.
            config.insert_json5("gateway/south", "[]").unwrap();

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

/// Per-test rmw_zenohd (zenoh-c 1.6.2 router) for RCL↔hiroz interop tests.
///
/// The in-process TestRouter uses zenoh-rs 1.9.0, which may not route traffic
/// from zenoh-c 1.6.2 (rmw_zenoh_cpp) sessions correctly. rmw_zenohd is the
/// reference router for rmw_zenoh_cpp and is known to work with it. Both the
/// RCL processes (zenoh-c 1.6.2) and hiroz sessions (zenoh-rs 1.9.0) connect
/// to this shared daemon.
#[allow(dead_code)]
pub struct RmwZenohDaemon {
    pub port: u16,
    pub endpoint: String,
    _process: ProcessGuard,
}

#[allow(dead_code)]
impl RmwZenohDaemon {
    /// Find the rmw_zenohd binary path via `ros2 pkg prefix rmw_zenoh_cpp`.
    fn find_binary() -> Option<std::path::PathBuf> {
        let output = Command::new("ros2")
            .args(["pkg", "prefix", "rmw_zenoh_cpp"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let prefix = String::from_utf8(output.stdout).ok()?;
        let path = std::path::PathBuf::from(prefix.trim()).join("lib/rmw_zenoh_cpp/rmw_zenohd");
        path.exists().then_some(path)
    }

    /// Start a new rmw_zenohd process on a free OS-assigned port.
    ///
    /// Uses `ZENOH_CONFIG_OVERRIDE` to override the listen endpoint so each
    /// test gets its own isolated router. Retries up to 5 times if the
    /// randomly chosen port is taken before rmw_zenohd binds it.
    pub fn new() -> Self {
        use std::os::unix::process::CommandExt;

        let binary =
            Self::find_binary().expect("rmw_zenohd not found — ensure rmw_zenoh_cpp is installed");

        for attempt in 0..5u32 {
            // Ask the OS for a free port, release it, then let rmw_zenohd bind it.
            let port = {
                let listener =
                    std::net::TcpListener::bind("127.0.0.1:0").expect("Failed to bind port 0");
                listener.local_addr().unwrap().port()
            };

            let endpoint = format!("tcp/127.0.0.1:{}", port);
            println!(
                "Starting rmw_zenohd on port {} (attempt {})...",
                port,
                attempt + 1
            );

            // Use ZENOH_CONFIG_OVERRIDE to bind to our chosen port and disable
            // multicast so each test's daemon is fully isolated.
            let override_val = format!(
                "listen/endpoints=[\"{}\"];connect/endpoints=[];scouting/multicast/enabled=false",
                endpoint
            );

            let child = Command::new(&binary)
                .env("ZENOH_CONFIG_OVERRIDE", &override_val)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .process_group(0)
                .spawn()
                .expect("Failed to spawn rmw_zenohd");

            let pid = child.id();

            // Wait for rmw_zenohd to confirm it is listening. Read stderr lines
            // until we see a "Listening" line or a port-in-use error.
            // We wrap the child in a ProcessGuard only after confirming startup.
            let mut guard = ProcessGuard::new(child, "rmw_zenohd");

            // Give the process a moment to bind the port, then probe it.
            thread::sleep(Duration::from_millis(200));

            // Verify the daemon is alive and bound by attempting a TCP connect.
            let ready = std::net::TcpStream::connect_timeout(
                &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
                Duration::from_millis(300),
            )
            .is_ok();

            if ready {
                println!("rmw_zenohd ready on {} (pid {})", endpoint, pid);
                // Give it an extra moment to fully initialize before returning.
                thread::sleep(Duration::from_millis(300));
                return Self {
                    port,
                    endpoint,
                    _process: guard,
                };
            }

            // Port is not up — daemon may have failed to bind (TOCTOU race).
            // ProcessGuard::drop() will SIGINT it.
            println!("rmw_zenohd not ready on port {}, retrying...", port);
            drop(guard);
            thread::sleep(Duration::from_millis(100));
        }

        panic!("Failed to start rmw_zenohd after 5 attempts");
    }

    /// The TCP endpoint string for this daemon.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// `ZENOH_CONFIG_OVERRIDE` value for RCL processes — directs them to
    /// connect to this daemon instead of the default `tcp/localhost:7447`.
    pub fn rmw_zenoh_env(&self) -> String {
        format!(
            "connect/endpoints=[\"{}\"];scouting/multicast/enabled=false",
            self.endpoint
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

/// Create a hiroz context configured to connect to an RmwZenohDaemon
#[allow(dead_code)]
pub fn create_hiroz_context_with_daemon(
    daemon: &RmwZenohDaemon,
) -> hiroz::Result<hiroz::context::ZContext> {
    create_hiroz_context_with_endpoint(daemon.endpoint())
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
