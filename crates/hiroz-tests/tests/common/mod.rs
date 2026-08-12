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

/// Concurrently drains a child's piped `stdout` and `stderr` so they can be
/// reported when the child fails.
///
/// A test that spawns an external process and discards its diagnostics can only
/// report *that* the process failed, never why. `ros2 run` writes its reason to
/// stderr, so a test which asserts on an exit status must capture both streams
/// and surface them in the failure message.
///
/// **Draining concurrently is what makes piping safe.** A pipe nobody reads
/// fills, and the child then blocks writing to it — which would convert a fast
/// failure into whatever timeout the caller's wait loop uses. One reader thread
/// per stream removes that coupling: they run until EOF, which arrives when the
/// child exits.
pub struct OutputCapture {
    stdout: Arc<std::sync::Mutex<String>>,
    stderr: Arc<std::sync::Mutex<String>>,
    readers: Vec<thread::JoinHandle<()>>,
}

#[allow(dead_code)]
impl OutputCapture {
    /// Takes the child's piped handles and starts draining them immediately.
    ///
    /// Call directly after `spawn`, before any wait loop. A stream that was not
    /// piped contributes nothing.
    pub fn start(child: &mut Child) -> Self {
        use std::io::{BufRead, BufReader, Read};

        fn drain<R: Read + Send + 'static>(
            stream: Option<R>,
            sink: Arc<std::sync::Mutex<String>>,
        ) -> Option<thread::JoinHandle<()>> {
            let stream = stream?;
            Some(thread::spawn(move || {
                // Append line by line rather than reading to EOF in one call, so
                // [`OutputCapture::snapshot`] can observe a still-running child.
                // A process that never exits on its own — a subscriber, say —
                // would otherwise yield nothing until it was killed.
                //
                // A read error is not worth failing over: the caller is already
                // reporting a failure and this is supplementary detail.
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    if let Ok(mut sink) = sink.lock() {
                        sink.push_str(&line);
                        sink.push('\n');
                    }
                }
            }))
        }

        let stdout = Arc::new(std::sync::Mutex::new(String::new()));
        let stderr = Arc::new(std::sync::Mutex::new(String::new()));
        let readers = [
            drain(child.stdout.take(), stdout.clone()),
            drain(child.stderr.take(), stderr.clone()),
        ]
        .into_iter()
        .flatten()
        .collect();

        Self {
            stdout,
            stderr,
            readers,
        }
    }

    /// Everything captured on `stdout` so far, without waiting for the child.
    ///
    /// For a process that does not exit on its own — a subscriber that runs
    /// until it is signalled — this is the only way to assert on what it
    /// printed. [`Self::finish`] would block until the reader threads see EOF.
    pub fn stdout_snapshot(&self) -> String {
        self.stdout.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Joins the reader threads and renders both streams as a printable block.
    ///
    /// Call only once the child has exited, so the readers have reached EOF.
    pub fn finish(self) -> String {
        for reader in self.readers {
            let _ = reader.join();
        }
        let mut block = String::new();
        for (label, sink) in [("stdout", &self.stdout), ("stderr", &self.stderr)] {
            let text = sink.lock().map(|s| s.clone()).unwrap_or_default();
            if text.trim().is_empty() {
                block.push_str(&format!("--- child {label}: <empty> ---\n"));
            } else {
                block.push_str(&format!("--- child {label} ---\n{}\n", text.trim_end()));
            }
        }
        block
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
                    // Poll the router's TCP listener (40 * 50ms, ~2s budget)
                    // instead of a blind fixed sleep; proceed anyway if it
                    // never accepts (best-effort).
                    let probe_addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
                    for _ in 0..40 {
                        // connect_timeout caps each probe at 50ms; plain connect
                        // could block on the OS timeout and blow the budget.
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
                    // TCP-accept only proves the listener is bound; short floor
                    // covers the router's remaining routing/liveliness init.
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

/// Wait until a ROS node named `node_name` is visible in the graph, or `timeout`
/// elapses. Deterministic replacement for a blind `wait_for_ready` sleep before
/// interacting with a just-spawned node: it returns as soon as the node is
/// discoverable (proceeds early on the fast path) instead of always sleeping a
/// fixed time. Returns whether the node appeared — callers may proceed either
/// way, since the following operation carries its own discovery timeout.
#[allow(dead_code)]
pub fn wait_for_ros_node(node_name: &str, router: &TestRouter, timeout: Duration) -> bool {
    let ctx = create_hiroz_context_with_router(router).expect("Failed to create probe context");
    let start = std::time::Instant::now();
    loop {
        if ctx
            .graph()
            .get_node_names()
            .iter()
            .any(|(name, _ns)| name == node_name)
        {
            println!("Node '{node_name}' discovered after {:?}", start.elapsed());
            return true;
        }
        if start.elapsed() >= timeout {
            eprintln!("Node '{node_name}' not visible after {timeout:?}; proceeding");
            return false;
        }
        thread::sleep(Duration::from_millis(50));
    }
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

/// Holds a background producer (Zenoh entities) alive until this guard drops.
/// Drop signals a stop flag and detaches the thread (no join, so a producer
/// blocked in recv can't hang teardown) — so teardown is best-effort, not
/// synchronous: the entities may briefly outlive the drop. Preferred over a
/// fixed-duration sleep: too short and the entity vanishes before `hu` reads it;
/// too long and the producer's client session reconnect-spins after `TestRouter`
/// drops, stealing CPU from later serial tests.
#[allow(dead_code)]
#[must_use = "binding must be kept alive (e.g. `let _producer = ...`); dropping it immediately tears the producer down"]
pub struct ProducerGuard {
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Drop for ProducerGuard {
    fn drop(&mut self) {
        // Signal stop, then detach: dropping a Zenoh session can block (async
        // close during Tokio teardown), so joining here risks hanging the test
        // thread. Detaching still stops the producer's active work immediately.
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            // Normally still running until we set `stop`. If already finished,
            // the producer exited early (usually a panic) — surface it: join()
            // on a finished thread returns immediately, and the panic would
            // otherwise be swallowed and misdiagnosed as "entity never appeared".
            if handle.is_finished()
                && let Err(panic) = handle.join()
            {
                let msg = panic
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "<non-string panic payload>".to_string());
                eprintln!(
                    "WARNING: test producer thread exited early (before teardown) \
                     with a panic: {msg}. Downstream 'entity not discovered' \
                     failures in this test are likely caused by this."
                );
            }
            // Otherwise detach: the still-running thread's session teardown can
            // block, and must not block the test thread.
        }
    }
}

/// Spawn a producer running `body` on a fresh Tokio runtime. `body` must poll
/// the stop flag to hold entities alive (e.g.
/// `while !stop.load(Ordering::Relaxed) { tokio::time::sleep(..).await }`) and
/// exits, dropping them, when the returned guard drops. `body` is async, so
/// tasks it spawns (e.g. an action-server handler) keep running while it holds.
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
