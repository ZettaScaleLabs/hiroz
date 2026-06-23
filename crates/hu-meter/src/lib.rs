wit_bindgen::generate!({
    world: "hu-plugin",
    path: "../wit/hu-plugin.wit",
});

use exports::hu::plugin::types::{Guest, PluginEvent, PluginManifest};
use hu::plugin::{graph, render, ros};

// ─── Plugin state ─────────────────────────────────────────────────────────────

struct HuMeter {
    mode: Mode,
    json: bool,
    // Ticks elapsed (used for duration tracking at tick_ms = 1000 ms)
    ticks: u32,
    duration_ticks: u32,
}

enum Mode {
    /// Waiting for startup event (initial state)
    Init,
    /// Measure publish rate on a topic
    Hz {
        topic: String,
        sub: Option<ros::Subscription>,
    },
    /// Measure bandwidth on a topic
    Bw {
        topic: String,
        sub: Option<ros::Subscription>,
    },
    /// Echo messages
    Echo {
        topic: String,
        sub: Option<ros::Subscription>,
        count: usize,
        printed: usize,
    },
    /// Delay measurement (header stamp vs receive time) — requires JSON with header.stamp
    Delay {
        topic: String,
        sub: Option<ros::Subscription>,
    },
    /// One-shot commands that finish in startup
    Done,
}

impl HuMeter {
    fn new() -> Self {
        HuMeter {
            mode: Mode::Init,
            json: false,
            ticks: 0,
            duration_ticks: 0,
        }
    }

    fn startup(&mut self, args: Vec<String>) {
        // Parse --json flag
        let args: Vec<String> = args
            .into_iter()
            .filter(|a| {
                if a == "--json" {
                    self.json = true;
                    false
                } else {
                    true
                }
            })
            .collect();

        let Some(subcmd) = args.first() else {
            render::println("Usage: hu meter <subcommand> [args]");
            render::println("  hz <topic> [--duration <s>] [--window <n>]");
            render::println("  bw <topic> [--duration <s>]");
            render::println("  echo <topic> [--count <n>]");
            render::println("  delay <topic>");
            render::println("  list topics|nodes|services|actions");
            render::println("  info topic|node|service <name>");
            render::exit(1);
            self.mode = Mode::Done;
            return;
        };

        match subcmd.as_str() {
            "hz" => self.cmd_hz(&args[1..]),
            "bw" => self.cmd_bw(&args[1..]),
            "echo" => self.cmd_echo(&args[1..]),
            "delay" => self.cmd_delay(&args[1..]),
            "list" => {
                self.cmd_list(&args[1..]);
                self.mode = Mode::Done;
            }
            "info" => {
                self.cmd_info(&args[1..]);
                self.mode = Mode::Done;
            }
            "pub" => {
                self.cmd_pub(&args[1..]);
                self.mode = Mode::Done;
            }
            "service" => {
                render::println("service subcommand: not yet implemented in WASM plugin");
                render::exit(1);
                self.mode = Mode::Done;
            }
            "param" => {
                render::println("param subcommand: not yet implemented in WASM plugin");
                render::exit(1);
                self.mode = Mode::Done;
            }
            "action" => {
                render::println("action subcommand: not yet implemented in WASM plugin");
                render::exit(1);
                self.mode = Mode::Done;
            }
            other => {
                render::println(&format!("unknown subcommand: {other}"));
                render::exit(1);
                self.mode = Mode::Done;
            }
        }
    }

    fn cmd_hz(&mut self, args: &[String]) {
        let (topic, duration_ticks, _window) = parse_topic_duration_window(args);
        let Some(topic) = topic else {
            render::println("Usage: hu meter hz <topic> [--duration <s>] [--window <n>]");
            render::exit(1);
            self.mode = Mode::Done;
            return;
        };
        self.duration_ticks = duration_ticks;
        let sub = match ros::subscribe(&topic) {
            Ok(s) => s,
            Err(e) => {
                render::println(&format!("Failed to subscribe to {topic}: {e}"));
                render::exit(1);
                self.mode = Mode::Done;
                return;
            }
        };
        self.mode = Mode::Hz {
            topic,
            sub: Some(sub),
        };
    }

    fn cmd_bw(&mut self, args: &[String]) {
        let (topic, duration_ticks, _window) = parse_topic_duration_window(args);
        let Some(topic) = topic else {
            render::println("Usage: hu meter bw <topic> [--duration <s>]");
            render::exit(1);
            self.mode = Mode::Done;
            return;
        };
        self.duration_ticks = duration_ticks;
        let sub = match ros::subscribe(&topic) {
            Ok(s) => s,
            Err(e) => {
                render::println(&format!("Failed to subscribe to {topic}: {e}"));
                render::exit(1);
                self.mode = Mode::Done;
                return;
            }
        };
        self.mode = Mode::Bw {
            topic,
            sub: Some(sub),
        };
    }

    fn cmd_echo(&mut self, args: &[String]) {
        let Some(topic) = args.first().cloned() else {
            render::println("Usage: hu meter echo <topic> [--count <n>]");
            render::exit(1);
            self.mode = Mode::Done;
            return;
        };
        let count = flag_value(args, "--count")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0usize);
        let sub = match ros::subscribe(&topic) {
            Ok(s) => s,
            Err(e) => {
                render::println(&format!("Failed to subscribe to {topic}: {e}"));
                render::exit(1);
                self.mode = Mode::Done;
                return;
            }
        };
        self.mode = Mode::Echo {
            topic,
            sub: Some(sub),
            count,
            printed: 0,
        };
    }

    fn cmd_delay(&mut self, args: &[String]) {
        let Some(topic) = args.first().cloned() else {
            render::println("Usage: hu meter delay <topic>");
            render::exit(1);
            self.mode = Mode::Done;
            return;
        };
        let sub = match ros::subscribe(&topic) {
            Ok(s) => s,
            Err(e) => {
                render::println(&format!("Failed to subscribe to {topic}: {e}"));
                render::exit(1);
                self.mode = Mode::Done;
                return;
            }
        };
        self.mode = Mode::Delay {
            topic,
            sub: Some(sub),
        };
    }

    fn cmd_list(&self, args: &[String]) {
        let what = args.first().map(|s| s.as_str()).unwrap_or("topics");
        let show_all = args.contains(&"--all".to_string());

        match what {
            "topics" => {
                let topics = graph::list_topics();
                let topics: Vec<_> = if show_all {
                    topics
                } else {
                    topics.into_iter().filter(|t| !is_hidden(&t.name)).collect()
                };
                if self.json {
                    render::println(&format!(
                        "[{}]",
                        topics
                            .iter()
                            .map(|t| format!(
                                "{{\"name\":\"{}\",\"type\":\"{}\"}}",
                                t.name, t.type_name
                            ))
                            .collect::<Vec<_>>()
                            .join(",")
                    ));
                } else {
                    for t in &topics {
                        render::println(&format!("{}\t[{}]", t.name, t.type_name));
                    }
                }
                render::exit(0);
            }
            "nodes" => {
                let nodes = graph::list_nodes();
                let nodes: Vec<_> = if show_all {
                    nodes
                } else {
                    nodes.into_iter().filter(|n| !is_hidden(&n.name)).collect()
                };
                if self.json {
                    render::println(&format!(
                        "[{}]",
                        nodes
                            .iter()
                            .map(|n| format!(
                                "{{\"namespace\":\"{}\",\"name\":\"{}\"}}",
                                n.namespace, n.name
                            ))
                            .collect::<Vec<_>>()
                            .join(",")
                    ));
                } else {
                    for n in &nodes {
                        let full = if n.namespace == "/" {
                            format!("/{}", n.name)
                        } else {
                            format!("{}/{}", n.namespace, n.name)
                        };
                        render::println(&full);
                    }
                }
                render::exit(0);
            }
            "services" => {
                let services = graph::list_services();
                let services: Vec<_> = if show_all {
                    services
                } else {
                    services
                        .into_iter()
                        .filter(|s| !is_hidden(&s.name))
                        .collect()
                };
                if self.json {
                    render::println(&format!(
                        "[{}]",
                        services
                            .iter()
                            .map(|s| format!(
                                "{{\"name\":\"{}\",\"type\":\"{}\"}}",
                                s.name, s.type_name
                            ))
                            .collect::<Vec<_>>()
                            .join(",")
                    ));
                } else {
                    for s in &services {
                        render::println(&format!("{}\t[{}]", s.name, s.type_name));
                    }
                }
                render::exit(0);
            }
            other => {
                render::println(&format!("unknown list target: {other}"));
                render::println("Usage: hu meter list topics|nodes|services");
                render::exit(1);
            }
        }
    }

    fn cmd_info(&self, args: &[String]) {
        let kind = args.first().map(|s| s.as_str()).unwrap_or("");
        let name = args.get(1).cloned().unwrap_or_default();

        match kind {
            "topic" => {
                let topics = graph::list_topics();
                let Some(topic) = topics.into_iter().find(|t| t.name == name) else {
                    render::println(&format!("topic not found: {name}"));
                    render::exit(1);
                    return;
                };
                if self.json {
                    render::println(&format!(
                        "{{\"name\":\"{}\",\"type\":\"{}\",\"publishers\":{},\"subscribers\":{}}}",
                        topic.name, topic.type_name, topic.publishers, topic.subscribers
                    ));
                } else {
                    render::println(&format!("Type: {}", topic.type_name));
                    render::println(&format!("Publishers:  {}", topic.publishers));
                    render::println(&format!("Subscribers: {}", topic.subscribers));
                }
                render::exit(0);
            }
            "node" => {
                let nodes = graph::list_nodes();
                let Some(node) = nodes.into_iter().find(|n| {
                    n.name == name || {
                        let full = if n.namespace == "/" {
                            format!("/{}", n.name)
                        } else {
                            format!("{}/{}", n.namespace, n.name)
                        };
                        full == name
                    }
                }) else {
                    render::println(&format!("node not found: {name}"));
                    render::exit(1);
                    return;
                };
                if self.json {
                    render::println(&format!(
                        "{{\"namespace\":\"{}\",\"name\":\"{}\"}}",
                        node.namespace, node.name
                    ));
                } else {
                    render::println(&format!("Node: {}/{}", node.namespace, node.name));
                }
                render::exit(0);
            }
            "service" => {
                let services = graph::list_services();
                let Some(svc) = services.into_iter().find(|s| s.name == name) else {
                    render::println(&format!("service not found: {name}"));
                    render::exit(1);
                    return;
                };
                if self.json {
                    render::println(&format!(
                        "{{\"name\":\"{}\",\"type\":\"{}\",\"servers\":{}}}",
                        svc.name, svc.type_name, svc.servers
                    ));
                } else {
                    render::println(&format!("Type: {}", svc.type_name));
                    render::println(&format!("Servers: {}", svc.servers));
                }
                render::exit(0);
            }
            other => {
                render::println(&format!("unknown info kind: {other}"));
                render::println("Usage: hu meter info topic|node|service <name>");
                render::exit(1);
            }
        }
    }

    fn cmd_pub(&self, args: &[String]) {
        let Some(topic) = args.first().cloned() else {
            render::println("Usage: hu meter pub <topic> --msg-type <type> --yaml <yaml>");
            render::exit(1);
            return;
        };
        let msg_type = flag_value(args, "--msg-type").unwrap_or_default();
        let yaml = flag_value(args, "--yaml").unwrap_or_else(|| "{}".to_string());

        if msg_type.is_empty() {
            render::println("--msg-type is required");
            render::exit(1);
            return;
        }

        let cdr = match ros::encode_yaml_to_cdr(&yaml, &msg_type) {
            Ok(b) => b,
            Err(e) => {
                render::println(&format!("encode error: {e}"));
                render::exit(1);
                return;
            }
        };

        let sess = match hu::plugin::session::get_session("default") {
            Ok(s) => s,
            Err(e) => {
                render::println(&format!("failed to get default session: {e}"));
                render::exit(1);
                return;
            }
        };
        let pub_ = match sess.raw_publisher(&topic) {
            Ok(p) => p,
            Err(e) => {
                render::println(&format!("failed to declare publisher on {topic}: {e}"));
                render::exit(1);
                return;
            }
        };
        if let Err(e) = pub_.publish(cdr) {
            render::println(&format!("publish error: {e}"));
            render::exit(1);
        } else {
            if self.json {
                render::println(&format!("{{\"published\":true,\"topic\":\"{topic}\"}}"));
            } else {
                render::println(&format!("Published to {topic}"));
            }
            render::exit(0);
        }
    }

    fn on_tick(&mut self) {
        self.ticks += 1;
        let done = self.duration_ticks > 0 && self.ticks >= self.duration_ticks;

        match &mut self.mode {
            Mode::Hz { topic, sub } => {
                let window_ms = 1000u32;
                match ros::measure_hz(topic, window_ms) {
                    Ok(hz) => {
                        let t = topic.clone();
                        if self.json {
                            render::println(&format!("{{\"topic\":\"{t}\",\"rate_hz\":{hz:.3}}}"));
                        } else {
                            render::println(&format!("{t}: {hz:.3} Hz"));
                        }
                    }
                    Err(e) => render::println(&format!("measure-hz error: {e}")),
                }
                let _ = sub; // keep subscription alive
                if done {
                    render::exit(0);
                    self.mode = Mode::Done;
                }
            }
            Mode::Bw { topic, sub } => {
                let window_ms = 1000u32;
                match ros::measure_bw(topic, window_ms) {
                    Ok(kbps) => {
                        let t = topic.clone();
                        if self.json {
                            render::println(&format!(
                                "{{\"topic\":\"{t}\",\"bandwidth_kbps\":{kbps:.3}}}"
                            ));
                        } else {
                            render::println(&format!("{t}: {kbps:.3} KB/s"));
                        }
                    }
                    Err(e) => render::println(&format!("measure-bw error: {e}")),
                }
                let _ = sub;
                if done {
                    render::exit(0);
                    self.mode = Mode::Done;
                }
            }
            Mode::Echo {
                topic,
                sub,
                count,
                printed,
            } => {
                let Some(s) = sub.as_ref() else {
                    return;
                };
                while let Some(json_msg) = s.try_recv() {
                    let t = topic.clone();
                    *printed += 1;
                    render::println(&format!("[{t}] {json_msg}"));
                    if *count > 0 && *printed >= *count {
                        render::exit(0);
                        self.mode = Mode::Done;
                        return;
                    }
                }
            }
            Mode::Delay { topic, sub } => {
                let Some(s) = sub.as_ref() else {
                    return;
                };
                while let Some(json_msg) = s.try_recv() {
                    // Try to extract header.stamp.sec + nanosec from JSON
                    let delay_note = extract_delay_note(&json_msg);
                    let t = topic.clone();
                    render::println(&format!("[{t}] {delay_note}"));
                }
            }
            Mode::Done | Mode::Init => {}
        }
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == flag {
            return it.next().cloned();
        }
        if let Some(val) = a.strip_prefix(&format!("{flag}=")) {
            return Some(val.to_string());
        }
    }
    None
}

fn parse_topic_duration_window(args: &[String]) -> (Option<String>, u32, usize) {
    let topic = args.first().filter(|a| !a.starts_with('-')).cloned();
    let duration_s: f64 = flag_value(args, "--duration")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0);
    // Convert duration seconds to ticks at tick_ms = 1000 ms
    let duration_ticks = if duration_s > 0.0 {
        duration_s.ceil() as u32
    } else {
        0
    };
    let window = flag_value(args, "--window")
        .and_then(|v| v.parse().ok())
        .unwrap_or(100usize);
    (topic, duration_ticks, window)
}

fn is_hidden(name: &str) -> bool {
    name.split('/').any(|seg| seg.starts_with('_'))
}

fn extract_delay_note(json: &str) -> String {
    // Naive: look for "sec" and "nanosec" fields in the JSON string.
    // A real impl would parse the JSON; here we just report the raw message.
    format!("(raw) {json}")
}

// ─── Plugin entry points ──────────────────────────────────────────────────────

static mut STATE: Option<HuMeter> = None;

fn state() -> &'static mut HuMeter {
    unsafe {
        if STATE.is_none() {
            STATE = Some(HuMeter::new());
        }
        STATE.as_mut().unwrap()
    }
}

struct Plugin;

impl Guest for Plugin {
    fn manifest() -> PluginManifest {
        PluginManifest {
            name: "meter".to_string(),
            version: "0.1.0".to_string(),
            description: "Rate, bandwidth, echo, list, and info for ROS 2".to_string(),
            bindings: vec![],
            tick_ms: 1000,
            sessions: vec![],
        }
    }

    fn on_event(event: PluginEvent) {
        match event {
            PluginEvent::Startup(args) => state().startup(args),
            PluginEvent::Tick => state().on_tick(),
            PluginEvent::KeyAction(cmd) => {
                if cmd == "interrupt" {
                    render::exit(130);
                }
            }
            PluginEvent::TopicSelected(_) => {}
        }
    }
}

export!(Plugin);
