use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
                    // Poll until the router's TCP listener accepts a connection,
                    // up to a ~2s budget (40 * 50ms). This replaces a blind fixed
                    // sleep and usually proceeds much sooner; if the budget
                    // elapses without a successful connect, proceed anyway
                    // (best-effort).
                    let probe_addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
                    for _ in 0..40 {
                        // connect_timeout caps each probe at 50ms; a plain
                        // TcpStream::connect could block far longer on the OS
                        // connect timeout and blow the ~2s budget.
                        if std::net::TcpStream::connect_timeout(
                            &probe_addr,
                            Duration::from_millis(50),
                        )
                        .is_ok()
                        {
                            break;
                        }
                        thread::sleep(Duration::from_millis(50));
                    }
                    // TCP-accept only proves the listener is bound; the zenoh
                    // router may still be finishing its routing/liveliness init.
                    // A short floor (far below the old blind 500ms) covers that
                    // window without paying the full fixed cost on every test.
                    thread::sleep(Duration::from_millis(150));
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

/// A background producer kept alive for exactly the lifetime of this guard.
///
/// On drop it flips a stop flag and detaches the producer thread (no join, so a
/// producer wedged in a blocking recv can't hang teardown); the thread drops all
/// held Zenoh entities (and their session) cleanly once it observes the flag.
/// Prefer this over a detached thread that sleeps a fixed duration: too short a
/// sleep lets the entity vanish
/// before `hu` reads it; too long leaves the producer's client session
/// reconnect-spinning after the test's `TestRouter` drops, stealing CPU from
/// later serial tests. Here the hold is exactly the test's scope — no guessed
/// duration.
#[allow(dead_code)]
#[must_use = "binding must be kept alive (e.g. `let _producer = ...`); dropping it immediately tears the producer down"]
pub struct ProducerGuard {
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Drop for ProducerGuard {
    fn drop(&mut self) {
        // Signal the producer to stop holding; it then exits its loop and drops
        // its entities + Zenoh session on its own thread. We deliberately do NOT
        // join: dropping a Zenoh session can block (async close during Tokio
        // runtime teardown), and blocking the test thread on that risks a hang.
        // Detaching is enough for the goal — the producer stops its active work
        // immediately (no lingering 40s hold, no reconnect-spin), and the
        // teardown completes in the background (or is reaped at process exit).
        self.stop.store(true, Ordering::Relaxed);
        // Drop the JoinHandle without joining, detaching the thread.
        self.handle.take();
    }
}

/// Spawn a producer running `body` on a fresh Tokio runtime. `body` receives a
/// stop flag it must poll: hold entities alive with
/// `while !stop.load(Ordering::Relaxed) { tokio::time::sleep(..).await }`, or
/// check it inside a service loop. The producer exits (dropping its entities)
/// when the returned guard drops. `body` is `async` — it awaits rather than
/// blocks — so any tasks it spawns (e.g. an action-server handler) keep running
/// while it holds.
#[allow(dead_code)]
pub fn spawn_producer<Fut>(
    body: impl FnOnce(Arc<AtomicBool>) -> Fut + Send + 'static,
) -> ProducerGuard
where
    Fut: std::future::Future<Output = ()>,
{
    let stop = Arc::new(AtomicBool::new(false));
    let s = stop.clone();
    let handle = thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(body(s));
    });
    ProducerGuard {
        stop,
        handle: Some(handle),
    }
}

/// Convenience for a passive producer: build entities up front, then hold them
/// alive until the guard drops. `build` runs inside the Tokio runtime.
#[allow(dead_code)]
pub fn spawn_holder<T: Send + 'static>(
    build: impl FnOnce() -> T + Send + 'static,
) -> ProducerGuard {
    spawn_producer(|stop| async move {
        let _held = build();
        while !stop.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
}
