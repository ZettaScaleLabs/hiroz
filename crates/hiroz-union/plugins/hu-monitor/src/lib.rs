wit_bindgen::generate!({
    world: "hu-cli-plugin",
    path: "wit/hu-plugin.wit",
});

use hu::plugin::{
    graph, render, ros,
    types::{EventKind, Permission},
};

// ─── Plugin state ─────────────────────────────────────────────────────────────

struct HuMonitor {
    mode: Mode,
    json: bool,
}

enum Mode {
    Init,
    /// Stream graph events by polling the graph snapshot each tick
    Watch {
        prev_topics: Vec<String>,
        prev_nodes: Vec<String>,
        prev_services: Vec<String>,
    },
    /// Subscribe to /rosout and print log messages
    Log {
        sub: Option<ros::Subscription>,
        count: usize,
        printed: usize,
    },
    /// Get/set node log level via rcl_interfaces service
    LogLevel {
        node: String,
        level: Option<String>,
        done: bool,
    },
    Done,
}

impl HuMonitor {
    fn new() -> Self {
        HuMonitor {
            mode: Mode::Init,
            json: false,
        }
    }

    fn startup(&mut self, args: Vec<String>) {
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
            render::println("Usage: hu monitor <subcommand> [args]");
            render::println("  watch            stream graph change events");
            render::println("  graph            show current graph snapshot");
            render::println("  log [--count <n>]  subscribe to /rosout");
            render::println("  log-level <node> [<level>]  get or set log level");
            render::exit(1);
            self.mode = Mode::Done;
            return;
        };

        match subcmd.as_str() {
            "watch" => {
                let topics: Vec<String> =
                    graph::list_topics().into_iter().map(|t| t.name).collect();
                let nodes: Vec<String> = graph::list_nodes()
                    .into_iter()
                    .map(|n| {
                        if n.namespace == "/" {
                            format!("/{}", n.name)
                        } else {
                            format!("{}/{}", n.namespace, n.name)
                        }
                    })
                    .collect();
                let services: Vec<String> =
                    graph::list_services().into_iter().map(|s| s.name).collect();
                self.mode = Mode::Watch {
                    prev_topics: topics,
                    prev_nodes: nodes,
                    prev_services: services,
                };
            }
            "graph" => {
                print_graph_snapshot(self.json);
                render::exit(0);
                self.mode = Mode::Done;
            }
            "log" => {
                // An absent `--count` defaults to 0 (unbounded streaming). A
                // present-but-unparseable `--count` is a user error — fail fast
                // instead of silently streaming forever.
                let count = match parse_count_flag(&args) {
                    Ok(c) => c,
                    Err(msg) => {
                        render::eprintln(&format!("ERROR: {msg}"));
                        render::exit(1);
                        self.mode = Mode::Done;
                        return;
                    }
                };
                let sub = match ros::subscribe("/rosout") {
                    Ok(s) => Some(s),
                    Err(e) => {
                        render::eprintln(&format!("ERROR: failed to subscribe to /rosout: {e}"));
                        render::exit(1);
                        self.mode = Mode::Done;
                        return;
                    }
                };
                self.mode = Mode::Log {
                    sub,
                    count,
                    printed: 0,
                };
            }
            "log-level" => {
                let node = match args.get(1) {
                    Some(n) => n.clone(),
                    None => {
                        render::eprintln("ERROR: log-level requires a node name");
                        render::exit(1);
                        self.mode = Mode::Done;
                        return;
                    }
                };
                let level = args.get(2).cloned();
                self.mode = Mode::LogLevel {
                    node,
                    level,
                    done: false,
                };
            }
            other => {
                render::eprintln(&format!("ERROR: unknown subcommand '{other}'"));
                render::exit(1);
                self.mode = Mode::Done;
            }
        }
    }

    fn tick(&mut self) {
        match &mut self.mode {
            Mode::Watch {
                prev_topics,
                prev_nodes,
                prev_services,
            } => {
                let cur_topics: Vec<String> =
                    graph::list_topics().into_iter().map(|t| t.name).collect();
                let cur_nodes: Vec<String> = graph::list_nodes()
                    .into_iter()
                    .map(|n| {
                        if n.namespace == "/" {
                            format!("/{}", n.name)
                        } else {
                            format!("{}/{}", n.namespace, n.name)
                        }
                    })
                    .collect();
                let cur_services: Vec<String> =
                    graph::list_services().into_iter().map(|s| s.name).collect();

                // Set-based membership so each tick's diff is O(n) rather than
                // O(n^2) from repeated Vec::contains scans.
                use std::collections::HashSet;
                let prev_topic_set: HashSet<&String> = prev_topics.iter().collect();
                let cur_topic_set: HashSet<&String> = cur_topics.iter().collect();
                let prev_node_set: HashSet<&String> = prev_nodes.iter().collect();
                let cur_node_set: HashSet<&String> = cur_nodes.iter().collect();
                let prev_service_set: HashSet<&String> = prev_services.iter().collect();
                let cur_service_set: HashSet<&String> = cur_services.iter().collect();

                for t in &cur_topics {
                    if !prev_topic_set.contains(t) {
                        render::println(&format!("topic appeared:   {t}"));
                    }
                }
                for t in prev_topics.iter() {
                    if !cur_topic_set.contains(t) {
                        render::println(&format!("topic removed:    {t}"));
                    }
                }
                for n in &cur_nodes {
                    if !prev_node_set.contains(n) {
                        render::println(&format!("node appeared:    {n}"));
                    }
                }
                for n in prev_nodes.iter() {
                    if !cur_node_set.contains(n) {
                        render::println(&format!("node removed:     {n}"));
                    }
                }
                for s in &cur_services {
                    if !prev_service_set.contains(s) {
                        render::println(&format!("service appeared: {s}"));
                    }
                }
                for s in prev_services.iter() {
                    if !cur_service_set.contains(s) {
                        render::println(&format!("service removed:  {s}"));
                    }
                }
                *prev_topics = cur_topics;
                *prev_nodes = cur_nodes;
                *prev_services = cur_services;
            }
            Mode::Log {
                sub,
                count,
                printed,
            } => {
                if let Some(s) = sub {
                    while let Some(msg) = s.try_recv() {
                        render::println(&msg);
                        *printed += 1;
                        if *count > 0 && *printed >= *count {
                            render::exit(0);
                            self.mode = Mode::Done;
                            return;
                        }
                    }
                }
            }
            Mode::LogLevel { node, level, done } => {
                if *done {
                    return;
                }
                *done = true;
                let node_name = node.clone();
                let set_level = level.clone();
                let svc_name = format!("{node_name}/get_logger_levels");
                let mut failed = false;
                match ros::connect_service(&svc_name, "rcl_interfaces/srv/GetLoggerLevels") {
                    Ok(client) => {
                        let req = serde_json::json!({ "names": [node_name] }).to_string();
                        match client.call(&req, 5000) {
                            Ok(resp) => render::println(&format!("log levels: {resp}")),
                            Err(e) => {
                                render::eprintln(&format!("ERROR: {e}"));
                                failed = true;
                            }
                        }
                    }
                    Err(e) => {
                        render::eprintln(&format!("ERROR: connect service: {e}"));
                        failed = true;
                    }
                }
                if let Some(lvl) = set_level {
                    let set_svc = format!("{node_name}/set_logger_levels");
                    match ros::connect_service(&set_svc, "rcl_interfaces/srv/SetLoggerLevels") {
                        Ok(client) => {
                            let lvl_num = log_level_to_int(&lvl);
                            let req = format!(
                                r#"{{"levels": [{{"name": "{node_name}", "level": {lvl_num}}}]}}"#
                            );
                            match client.call(&req, 5000) {
                                Ok(_) => render::println(&format!("set log level to {lvl}")),
                                Err(e) => {
                                    render::eprintln(&format!("ERROR: set level: {e}"));
                                    failed = true;
                                }
                            }
                        }
                        Err(e) => {
                            render::eprintln(&format!("ERROR: connect set-level service: {e}"));
                            failed = true;
                        }
                    }
                }
                render::exit(if failed { 1 } else { 0 });
                self.mode = Mode::Done;
            }
            _ => {}
        }
    }
}

fn print_graph_snapshot(json: bool) {
    let topics = graph::list_topics();
    let nodes = graph::list_nodes();
    let services = graph::list_services();

    if json {
        // Serialize via serde_json so string fields (topic/service names,
        // namespaces, type names) are properly escaped — a name containing
        // `"` or `\` must not corrupt the output.
        let out = serde_json::json!({
            "topics": topics
                .iter()
                .map(|t| serde_json::json!({
                    "name": t.name,
                    "type": t.type_name,
                    "publishers": t.publishers,
                    "subscribers": t.subscribers,
                }))
                .collect::<Vec<_>>(),
            "nodes": nodes
                .iter()
                .map(|n| serde_json::json!({
                    "namespace": n.namespace,
                    "name": n.name,
                }))
                .collect::<Vec<_>>(),
            "services": services
                .iter()
                .map(|s| serde_json::json!({
                    "name": s.name,
                    "type": s.type_name,
                    "servers": s.servers,
                }))
                .collect::<Vec<_>>(),
        });
        render::println(&out.to_string());
        return;
    }

    render::println("Topics:");
    if topics.is_empty() {
        render::println("  (none)");
    }
    for t in &topics {
        render::println(&format!(
            "  {} [{}]  pub:{} sub:{}",
            t.name, t.type_name, t.publishers, t.subscribers
        ));
    }

    render::println("Nodes:");
    if nodes.is_empty() {
        render::println("  (none)");
    }
    for n in &nodes {
        let full = if n.namespace == "/" {
            format!("/{}", n.name)
        } else {
            format!("{}/{}", n.namespace, n.name)
        };
        render::println(&format!("  {}", full));
    }

    render::println("Services:");
    if services.is_empty() {
        render::println("  (none)");
    }
    for s in &services {
        render::println(&format!(
            "  {} [{}]  servers:{}",
            s.name, s.type_name, s.servers
        ));
    }
}

/// Parse the `--count` flag for the `log` subcommand.
///
/// Returns `Ok(0)` when the flag is entirely absent (0 == unbounded streaming,
/// the historical default). Returns `Err` when the flag is present but its
/// value is missing or not a valid `usize`, so the caller can fail fast rather
/// than silently streaming forever.
fn parse_count_flag(args: &[String]) -> Result<usize, String> {
    let Some(pos) = args.iter().position(|a| a == "--count") else {
        return Ok(0);
    };
    match args.get(pos + 1) {
        Some(v) => v
            .parse()
            .map_err(|_| format!("invalid --count value '{v}'")),
        None => Err("--count requires a value".to_string()),
    }
}

fn log_level_to_int(level: &str) -> u32 {
    match level.to_uppercase().as_str() {
        "DEBUG" => 10,
        "INFO" => 20,
        "WARN" | "WARNING" => 30,
        "ERROR" => 40,
        "FATAL" => 50,
        _ => 20,
    }
}

// ─── Plugin entry points ──────────────────────────────────────────────────────
//
// WASM components are single-threaded — there are no threads, so Sync is trivially safe.
// The wit-bindgen generated resource handles (ros::Subscription, session handles, etc.)
// do not implement Sync, but this is a false negative for single-threaded WASM.
// Use OnceCell<RefCell<T>> to avoid unsafe static mut while staying no-std-safe.

use std::cell::{OnceCell, RefCell};

struct AssertSync<T>(T);
// SAFETY: WASM components run on a single thread; no concurrent access to the
// wrapped `static STATE` is possible, so asserting `Sync` cannot introduce a
// data race. If the host ever runs plugins on multiple threads this must be
// revisited (STATE would then need real synchronization).
unsafe impl<T> Sync for AssertSync<T> {}

static STATE: AssertSync<OnceCell<RefCell<HuMonitor>>> = AssertSync(OnceCell::new());

fn state() -> std::cell::RefMut<'static, HuMonitor> {
    STATE
        .0
        .get_or_init(|| RefCell::new(HuMonitor::new()))
        .borrow_mut()
}

struct Plugin;

impl Guest for Plugin {
    fn manifest() -> PluginManifest {
        PluginManifest {
            name: "monitor".to_string(),
            version: "0.1.0".to_string(),
            description: "Graph watch, log, and log-level for ROS 2".to_string(),
            bindings: vec![],
            tick_ms: 1000,
            sessions: vec![],
            subscribed_events: vec![EventKind::Startup, EventKind::Tick],
            required_permissions: vec![Permission::SubscribeTopic, Permission::CallService],
        }
    }

    fn on_event(event: CliEvent) {
        match event {
            CliEvent::Startup(args) => state().startup(args),
            CliEvent::Tick => state().tick(),
            CliEvent::Interrupt => render::exit(130),
        }
    }
}

export!(Plugin);
