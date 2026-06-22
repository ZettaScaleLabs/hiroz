use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration;

use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use zenoh::Wait;
use zenoh::config::WhatAmI;

/// Helper to manage background processes with automatic cleanup
pub struct ProcessGuard {
    pub child: Option<Child>,
    name: String,
}

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
            let pgid = Pid::from_raw(-pid);

            println!("Stopping process group: {}", self.name);

            // Send SIGINT to process group
            if let Err(e) = signal::kill(pgid, Signal::SIGINT) {
                eprintln!("Failed to send SIGINT to group {}: {}", self.name, e);
                let _ = child.kill();
            }

            // Wait for graceful shutdown with timeout
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
                            let _ = signal::kill(pgid, Signal::SIGKILL);
                            let _ = child.wait();
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
    pub port: u16,
    pub endpoint: String,
    _session: zenoh::Session,
}

impl TestRouter {
    /// Start a new Zenoh router session on a free OS-assigned port.
    ///
    /// Uses bind(:0) to avoid PID-derived port collisions when multiple test
    /// binaries run in parallel (e.g. hiroz-tests and hiroz-console).
    pub fn new() -> Self {
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
        panic!("Failed to start Zenoh router after 5 attempts");
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

/// Check if ros2 CLI is available
pub fn check_ros2_available() -> bool {
    Command::new("ros2").arg("--version").output().is_ok()
}

/// Spawn ros2 topic pub command
pub fn spawn_ros2_topic_pub(
    topic: &str,
    msg_type: &str,
    data: &str,
    router_port: u16,
) -> ProcessGuard {
    use std::os::unix::process::CommandExt;

    let env_override = format!(
        "connect/endpoints=[\"tcp/127.0.0.1:{}\"];scouting/multicast/enabled=false",
        router_port
    );

    let mut child = Command::new("ros2")
        .args(["topic", "pub", topic, msg_type, data])
        .env("RMW_IMPLEMENTATION", "rmw_zenoh_cpp")
        .env("ZENOH_CONFIG_OVERRIDE", &env_override)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .expect("Failed to spawn ros2 topic pub");

    thread::sleep(Duration::from_secs(2));

    // Log any stderr output so CI failures are diagnosable
    if let Some(stderr) = child.stderr.take() {
        let topic_name = topic.to_string();
        thread::spawn(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(stderr).lines().flatten() {
                eprintln!("[ros2 topic pub {}] {}", topic_name, line);
            }
        });
    }

    ProcessGuard::new(child, &format!("ros2_topic_pub_{}", topic))
}
