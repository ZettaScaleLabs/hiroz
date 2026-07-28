wit_bindgen::generate!({
    world: "hu-cli-plugin",
    path: "wit/hu-plugin.wit",
});


use hu::plugin::session::RawPublisher;
use hu::plugin::types::{EventKind, Permission};
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
        field: Option<String>,
    },
    /// Echo raw CDR bytes (hex), bypassing schema-based decode
    EchoRaw {
        topic: String,
        sub: Option<hu::plugin::raw_transport::RawSubscription>,
        count: usize,
        printed: usize,
    },
    /// Delay measurement (header stamp vs receive time) — requires JSON with header.stamp
    Delay {
        topic: String,
        sub: Option<ros::Subscription>,
    },
    /// Repeated publish with rate/times support
    Pub {
        pub_: RawPublisher,
        cdr: Vec<u8>,
        topic: String,
        interval_ticks: u32,
        times_remaining: u32,
        ticks_since_last: u32,
    },
    /// Subscribe to action feedback topic
    ActionEcho {
        sub: Option<ros::Subscription>,
        count: usize,
        printed: usize,
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
            render::println("  hz <topic> [--duration <s>]");
            render::println("  bw <topic> [--duration <s>]");
            render::println("  echo <topic> [--count <n>] [--field <path>]");
            render::println("  delay <topic> [--duration <s>]");
            render::println("  pub <topic> --msg-type <type> --yaml <yaml> [--rate <Hz>] [--times <n>]");
            render::println("  list topics|nodes|services [--find <type>]");
            render::println("  info topic|node|service <name>");
            render::println("  service list|find|type|call <name> [--yaml <yaml>|--payload <hex>]");
            render::println("  param list|get|set|dump|describe|load <node> [<param>] [<value>]");
            render::println("  action list|info|send-goal|echo <name> [args]");
            render::exit(1);
            self.mode = Mode::Done;
            return;
        };

        match subcmd.as_str() {
            "hz" => self.cmd_hz(&args[1..]),
            "bw" => self.cmd_bw(&args[1..]),
            "echo" => self.cmd_echo(&args[1..]),
            "delay" => self.cmd_delay(&args[1..]),
            "pub" => {
                self.cmd_pub(&args[1..]);
                // cmd_pub sets mode itself (Done for one-shot, or Pub for repeated).
            }
            "list" => {
                self.cmd_list(&args[1..]);
                self.mode = Mode::Done;
            }
            "info" => {
                self.cmd_info(&args[1..]);
                self.mode = Mode::Done;
            }
            "service" => {
                self.cmd_service(&args[1..]);
                self.mode = Mode::Done;
            }
            "param" => {
                self.cmd_param(&args[1..]);
                self.mode = Mode::Done;
            }
            "action" => {
                self.cmd_action(&args[1..]);
                // cmd_action sets mode itself for "echo"; the one-shot arms
                // (list/info/send-goal) finish in startup. Only set Done if
                // mode is still Init (i.e. a one-shot arm completed or errored).
                if matches!(self.mode, Mode::Init) {
                    self.mode = Mode::Done;
                }
            }
            other => {
                render::eprintln(&format!("unknown subcommand: {other}"));
                render::exit(1);
                self.mode = Mode::Done;
            }
        }
    }

    fn cmd_hz(&mut self, args: &[String]) {
        let (topic, duration_ticks) = parse_topic_duration(args);
        let Some(topic) = topic else {
            render::println("Usage: hu meter hz <topic> [--duration <s>]");
            render::exit(1);
            self.mode = Mode::Done;
            return;
        };
        self.duration_ticks = duration_ticks;
        let sub = match ros::subscribe(&topic) {
            Ok(s) => s,
            Err(e) => {
                render::eprintln(&format!("Failed to subscribe to {topic}: {e}"));
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
        let (topic, duration_ticks) = parse_topic_duration(args);
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
                render::eprintln(&format!("Failed to subscribe to {topic}: {e}"));
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
            render::println(
                "Usage: hu meter echo <topic> [--count <n>] [--field <path>] [--timeout <s>] [--raw]",
            );
            render::exit(1);
            self.mode = Mode::Done;
            return;
        };
        let count = flag_value(args, "--count")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0usize);
        let field = flag_value(args, "--field");
        // Explicit --timeout always applies. Absent that, fall back to a safety
        // bound so `echo` can never hang forever waiting for messages that
        // never arrive (matches hz/bw's duration-based exit).
        const DEFAULT_ECHO_TIMEOUT_TICKS: u32 = 30;
        self.duration_ticks = flag_value(args, "--timeout")
            .and_then(|v| v.parse::<f64>().ok())
            .map(|s| s.ceil().max(1.0) as u32)
            .unwrap_or(DEFAULT_ECHO_TIMEOUT_TICKS);

        if args.iter().any(|a| a == "--raw") {
            let ke = match ros::resolve_topic_ke(&topic) {
                Ok(ke) => ke,
                Err(e) => {
                    render::eprintln(&format!("Failed to resolve key expression for {topic}: {e}"));
                    render::exit(1);
                    self.mode = Mode::Done;
                    return;
                }
            };
            let sess = match hu::plugin::session::get_session("default") {
                Ok(s) => s,
                Err(e) => {
                    render::eprintln(&format!("failed to get default session: {e}"));
                    render::exit(1);
                    self.mode = Mode::Done;
                    return;
                }
            };
            let sub = match sess.raw_subscribe(&ke) {
                Ok(s) => s,
                Err(e) => {
                    render::eprintln(&format!("Failed to raw-subscribe to {topic}: {e}"));
                    render::exit(1);
                    self.mode = Mode::Done;
                    return;
                }
            };
            self.mode = Mode::EchoRaw {
                topic,
                sub: Some(sub),
                count,
                printed: 0,
            };
            return;
        }

        let sub = match ros::subscribe(&topic) {
            Ok(s) => s,
            Err(e) => {
                render::eprintln(&format!("Failed to subscribe to {topic}: {e}"));
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
            field,
        };
    }

    fn cmd_delay(&mut self, args: &[String]) {
        let Some(topic) = args.first().cloned() else {
            render::println("Usage: hu meter delay <topic> [--duration <s>]");
            render::exit(1);
            self.mode = Mode::Done;
            return;
        };
        // Optional self-exit, same as hz/bw/echo: with no --duration this stays
        // 0 (never done), matching the always-running behavior.
        self.duration_ticks = flag_value(args, "--duration")
            .and_then(|v| v.parse::<f64>().ok())
            .map(|s| s.ceil().max(1.0) as u32)
            .unwrap_or(0);
        let sub = match ros::subscribe(&topic) {
            Ok(s) => s,
            Err(e) => {
                render::eprintln(&format!("Failed to subscribe to {topic}: {e}"));
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

    fn cmd_pub(&mut self, args: &[String]) {
        let Some(topic) = args.first().cloned() else {
            render::println(
                "Usage: hu meter pub <topic> --msg-type <type> --yaml <yaml> [--rate <Hz>] [--times <n>]",
            );
            render::exit(1);
            self.mode = Mode::Done;
            return;
        };
        let msg_type = flag_value(args, "--msg-type").unwrap_or_default();
        let yaml = flag_value(args, "--yaml").unwrap_or_else(|| "{}".to_string());
        let rate: f64 = flag_value(args, "--rate")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0);
        let times: u32 = flag_value(args, "--times")
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);

        if msg_type.is_empty() {
            render::eprintln("--msg-type is required");
            render::exit(1);
            self.mode = Mode::Done;
            return;
        }

        let cdr = match ros::encode_yaml_to_cdr(&yaml, &msg_type) {
            Ok(b) => b,
            Err(e) => {
                render::println(&format!("encode error: {e}"));
                render::exit(1);
                self.mode = Mode::Done;
                return;
            }
        };

        let sess = match hu::plugin::session::get_session("default") {
            Ok(s) => s,
            Err(e) => {
                render::eprintln(&format!("failed to get default session: {e}"));
                render::exit(1);
                self.mode = Mode::Done;
                return;
            }
        };
        let pub_ = match sess.raw_publisher(&topic) {
            Ok(p) => p,
            Err(e) => {
                render::eprintln(&format!("failed to declare publisher on {topic}: {e}"));
                render::exit(1);
                self.mode = Mode::Done;
                return;
            }
        };

        // With --rate or --times>1, publish repeatedly via ticks; otherwise one-shot.
        if rate > 0.0 || times > 1 {
            // tick_ms = 1000 ms; interval_ticks = max(1, round(1.0 / rate)) when rate > 0.
            let interval_ticks = if rate > 0.0 {
                let t = (1.0 / rate).round() as u32;
                if t == 0 {
                    1
                } else {
                    t
                }
            } else {
                0 // no rate limit: publish each tick up to times_remaining
            };
            self.mode = Mode::Pub {
                pub_,
                cdr,
                topic,
                interval_ticks,
                times_remaining: times,
                ticks_since_last: interval_ticks, // ready to publish on the first tick
            };
        } else {
            let cdr_len = cdr.len();
            if let Err(e) = pub_.publish(&cdr) {
                render::println(&format!("publish error: {e}"));
                render::exit(1);
            } else {
                if self.json {
                    render::println(&format!(
                        "{{\"published\":1,\"bytes\":{cdr_len},\"topic\":\"{topic}\"}}"
                    ));
                } else {
                    render::println(&format!("Published to {topic}"));
                }
                render::exit(0);
            }
            self.mode = Mode::Done;
        }
    }

    fn cmd_list(&self, args: &[String]) {
        let what = args.first().map(|s| s.as_str()).unwrap_or("topics");
        let show_all = args.contains(&"--all".to_string());
        // `list <kind> --find <substr>` and the `list find-<kind> <substr>` sugar
        // both filter — the latter takes its filter as a bare positional arg.
        let (what, positional_filter) = match what {
            "find-topics" => ("topics", args.get(1).cloned()),
            "find-services" => ("services", args.get(1).cloned()),
            "find-nodes" => ("nodes", args.get(1).cloned()),
            other => (other, None),
        };
        let filter = positional_filter.or_else(|| flag_value(args, "--find"));
        let count_limit: Option<usize> = flag_value(args, "--count").and_then(|v| v.parse().ok());

        match what {
            "topics" => {
                let topics = graph::list_topics();
                let topics: Vec<_> = if show_all {
                    topics
                } else {
                    topics.into_iter().filter(|t| !is_hidden(&t.name)).collect()
                };
                let mut topics: Vec<_> = if let Some(ref f) = filter {
                    topics
                        .into_iter()
                        .filter(|t| t.type_name.contains(f.as_str()) || t.name.contains(f.as_str()))
                        .collect()
                } else {
                    topics
                };
                if let Some(n) = count_limit {
                    topics.truncate(n);
                }
                if self.json {
                    render::println(&format!(
                        "[{}]",
                        topics
                            .iter()
                            .map(|t| format!(
                                "{{\"name\":{},\"type\":{}}}",
                                json_str(&t.name),
                                json_str(&t.type_name)
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
                let mut nodes: Vec<_> = if let Some(ref f) = filter {
                    nodes.into_iter().filter(|n| n.name.contains(f.as_str())).collect()
                } else {
                    nodes
                };
                if let Some(n) = count_limit {
                    nodes.truncate(n);
                }
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
                let mut services: Vec<_> = if let Some(ref f) = filter {
                    services
                        .into_iter()
                        .filter(|s| s.type_name.contains(f.as_str()) || s.name.contains(f.as_str()))
                        .collect()
                } else {
                    services
                };
                if let Some(n) = count_limit {
                    services.truncate(n);
                }
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
                render::eprintln(&format!("unknown list target: {other}"));
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
                    render::eprintln(&format!("topic not found: {name}"));
                    render::exit(1);
                    return;
                };
                if self.json {
                    render::println(&format!(
                        "{{\"name\":\"{}\",\"type\":\"{}\",\"publisher_count\":{},\"subscriber_count\":{}}}",
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
                    render::eprintln(&format!("node not found: {name}"));
                    render::exit(1);
                    return;
                };
                let detail = graph::describe_node(&node.namespace, &node.name);
                let found = detail.is_some();
                let detail = detail.unwrap_or(graph::NodeDetail {
                    publishers: Vec::new(),
                    subscribers: Vec::new(),
                });
                if self.json {
                    let fmt_eps = |eps: &[graph::NodeEndpoint]| {
                        eps.iter()
                            .map(|e| {
                                format!(
                                    "{{\"name\":\"{}\",\"type\":\"{}\"}}",
                                    e.name, e.type_name
                                )
                            })
                            .collect::<Vec<_>>()
                            .join(",")
                    };
                    render::println(&format!(
                        "{{\"found\":{},\"namespace\":\"{}\",\"name\":\"{}\",\"publishers\":[{}],\"subscribers\":[{}]}}",
                        found,
                        node.namespace,
                        node.name,
                        fmt_eps(&detail.publishers),
                        fmt_eps(&detail.subscribers),
                    ));
                } else {
                    render::println(&format!("Node: {}/{}", node.namespace, node.name));
                    render::println(&format!("Publishers ({}):", detail.publishers.len()));
                    for p in &detail.publishers {
                        render::println(&format!("  {}  [{}]", p.name, p.type_name));
                    }
                    render::println(&format!("Subscribers ({}):", detail.subscribers.len()));
                    for s in &detail.subscribers {
                        render::println(&format!("  {}  [{}]", s.name, s.type_name));
                    }
                }
                render::exit(0);
            }
            "service" => {
                let services = graph::list_services();
                let Some(svc) = services.into_iter().find(|s| s.name == name) else {
                    render::eprintln(&format!("service not found: {name}"));
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
                render::eprintln(&format!("unknown info kind: {other}"));
                render::println("Usage: hu meter info topic|node|service <name>");
                render::exit(1);
            }
        }
    }

    fn cmd_service(&self, args: &[String]) {
        // hu meter service list [--json]
        // hu meter service find <substr>
        // hu meter service type <name>
        // hu meter service call <name> [--yaml <yaml> --msg-type <type> | --payload <hex>] [--timeout <s>]
        let Some(subcmd) = args.first() else {
            render::println("Usage: hu meter service list|find|type|call <name> [args]");
            render::exit(1);
            return;
        };
        match subcmd.as_str() {
            "list" => {
                let services = graph::list_services();
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
            "find" => {
                let Some(needle) = args.get(1) else {
                    render::println("Usage: hu meter service find <substr>");
                    render::exit(1);
                    return;
                };
                let services: Vec<_> = graph::list_services()
                    .into_iter()
                    .filter(|s| s.name.contains(needle.as_str()))
                    .collect();
                for s in &services {
                    render::println(&s.name);
                }
                render::exit(0);
            }
            "type" => {
                let Some(name) = args.get(1) else {
                    render::println("Usage: hu meter service type <name>");
                    render::exit(1);
                    return;
                };
                let Some(svc) = graph::list_services().into_iter().find(|s| &s.name == name)
                else {
                    render::eprintln(&format!("service not found: {name}"));
                    render::exit(1);
                    return;
                };
                render::println(&svc.type_name);
                render::exit(0);
            }
            "call" => {
                let Some(name) = args.get(1) else {
                    render::println(
                        "Usage: hu meter service call <name> [--yaml <yaml> --msg-type <type> | --payload <hex>] [--timeout <s>]",
                    );
                    render::exit(1);
                    return;
                };
                let timeout_ms: u32 = flag_value(args, "--timeout")
                    .and_then(|v| v.parse().ok())
                    .map(|s: u32| s.saturating_mul(1000))
                    .unwrap_or(5000);
                let msg_type = flag_value(args, "--msg-type").unwrap_or_else(|| {
                    graph::list_services()
                        .into_iter()
                        .find(|s| &s.name == name)
                        .map(|s| s.type_name)
                        .unwrap_or_default()
                });

                let client = match ros::connect_service(name, &msg_type) {
                    Ok(c) => c,
                    Err(e) => {
                        render::eprintln(&format!("ERROR: connect to {name}: {e}"));
                        render::exit(1);
                        return;
                    }
                };

                if let Some(hex_payload) = flag_value(args, "--payload") {
                    let payload = match parse_hex_bytes(&hex_payload) {
                        Ok(b) => b,
                        Err(e) => {
                            render::eprintln(&format!("ERROR: bad --payload: {e}"));
                            render::exit(1);
                            return;
                        }
                    };
                    match client.call_raw(&payload, timeout_ms) {
                        Ok(resp) => {
                            render::println(&format!(
                                "response ({} bytes): {}",
                                resp.len(),
                                bytes_to_hex(&resp)
                            ));
                            render::exit(0);
                        }
                        Err(e) => {
                            render::eprintln(&format!("ERROR: call failed: {e}"));
                            render::exit(1);
                        }
                    }
                    return;
                }

                let yaml = flag_value(args, "--yaml").unwrap_or_else(|| "{}".to_string());
                match client.call(&yaml, timeout_ms) {
                    Ok(resp) => {
                        render::println(&resp);
                        render::exit(0);
                    }
                    Err(e) => {
                        render::eprintln(&format!("ERROR: call failed: {e}"));
                        render::exit(1);
                    }
                }
            }
            other => {
                render::eprintln(&format!("unknown service subcommand: {other}"));
                render::println("Usage: hu meter service list|find|type|call <name> [args]");
                render::exit(1);
            }
        }
    }

    fn cmd_param(&self, args: &[String]) {
        // hu meter param list <node>
        // hu meter param get  <node> <name>
        // hu meter param set  <node> <name> <value>
        let subcmd = match args.first() {
            Some(s) => s.as_str(),
            None => {
                render::println(
                    "Usage: hu meter param list|get|set|dump|describe|load <node> [<param>] [<value>]",
                );
                render::exit(1);
                return;
            }
        };
        let node = match args.get(1) {
            Some(n) => n.clone(),
            None => {
                render::println("ERROR: node name required");
                render::exit(1);
                return;
            }
        };
        match subcmd {
            "list" => {
                let filter = flag_value(args, "--filter");
                let names = match list_param_names(&node) {
                    Ok(n) => n,
                    Err(e) => {
                        render::eprintln(&format!("ERROR: {e}"));
                        render::exit(1);
                        return;
                    }
                };
                let names: Vec<String> = match &filter {
                    Some(f) => names.into_iter().filter(|n| n.contains(f.as_str())).collect(),
                    None => names,
                };
                if self.json {
                    render::println(&format!(
                        "[{}]",
                        names
                            .iter()
                            .map(|n| format!("\"{n}\""))
                            .collect::<Vec<_>>()
                            .join(",")
                    ));
                } else {
                    for n in &names {
                        render::println(n);
                    }
                }
                render::exit(0);
            }
            "get" => {
                // Supports `param get <node> <name>` and `param get <node> <name1> <name2> ...`
                // — every positional arg after <node> (excluding flags) is a param name.
                let param_names: Vec<String> = args[2..]
                    .iter()
                    .filter(|a| !a.starts_with("--"))
                    .cloned()
                    .collect();
                if param_names.is_empty() {
                    render::println("ERROR: parameter name required");
                    render::exit(1);
                    return;
                }
                match get_param_values(&node, &param_names) {
                    Ok(values) => {
                        if self.json {
                            render::println(&format!(
                                "{{{}}}",
                                param_names
                                    .iter()
                                    .zip(values.iter())
                                    .map(|(n, v)| format!("\"{n}\":{v}"))
                                    .collect::<Vec<_>>()
                                    .join(",")
                            ));
                        } else {
                            for (n, v) in param_names.iter().zip(values.iter()) {
                                render::println(&format!("{n}: {v}"));
                            }
                        }
                        render::exit(0);
                    }
                    Err(e) => {
                        render::eprintln(&format!("ERROR: {e}"));
                        render::exit(1);
                    }
                }
            }
            "dump" => {
                let names = match list_param_names(&node) {
                    Ok(n) => n,
                    Err(e) => {
                        render::eprintln(&format!("ERROR: {e}"));
                        render::exit(1);
                        return;
                    }
                };
                let values = if names.is_empty() {
                    Vec::new()
                } else {
                    match get_param_values(&node, &names) {
                        Ok(v) => v,
                        Err(e) => {
                            render::eprintln(&format!("ERROR: {e}"));
                            render::exit(1);
                            return;
                        }
                    }
                };
                render::println(&format!("{node}:"));
                render::println("  ros__parameters:");
                for (n, v) in names.iter().zip(values.iter()) {
                    render::println(&format!("    {n}: {v}"));
                }
                render::exit(0);
            }
            "describe" => {
                let Some(param_name) = args.get(2) else {
                    render::println("ERROR: parameter name required");
                    render::exit(1);
                    return;
                };
                match get_param_values(&node, std::slice::from_ref(param_name)) {
                    Ok(values) => {
                        let value = values.first().cloned().unwrap_or_else(|| "null".to_string());
                        if self.json {
                            render::println(&format!(
                                "{{\"name\":\"{param_name}\",\"value\":{value}}}"
                            ));
                        } else {
                            render::println(&format!("Name: {param_name}"));
                            render::println(&format!("Value: {value}"));
                        }
                        render::exit(0);
                    }
                    Err(e) => {
                        render::eprintln(&format!("ERROR: {e}"));
                        render::exit(1);
                    }
                }
            }
            "set" => {
                let param_name = match args.get(2) {
                    Some(n) => n.clone(),
                    None => {
                        render::println("ERROR: parameter name required");
                        render::exit(1);
                        return;
                    }
                };
                let value_str = match args.get(3) {
                    Some(v) => v.clone(),
                    None => {
                        render::println("ERROR: value required");
                        render::exit(1);
                        return;
                    }
                };
                // Auto-detect type: bool → 1, integer → 2, float → 3, string → 4
                let (type_id, value_json) = infer_param_value(&value_str);
                let svc = format!("{node}/set_parameters");
                let client = match ros::connect_service(&svc, "rcl_interfaces/srv/SetParameters") {
                    Ok(c) => c,
                    Err(e) => {
                        render::eprintln(&format!("ERROR: connect to {svc}: {e}"));
                        render::exit(1);
                        return;
                    }
                };
                let req = format!(
                    r#"{{"parameters":[{{"name":"{param_name}","value":{{"type":{type_id},{value_json}}}}}]}}"#
                );
                match client.call(&req, 5000) {
                    Ok(resp) => {
                        render::println(&resp);
                        render::exit(0);
                    }
                    Err(e) => {
                        render::eprintln(&format!("ERROR: {e}"));
                        render::exit(1);
                    }
                }
            }
            "load" => {
                // The host CLI (main.rs) reads and parses the YAML file --
                // WASM plugins have no filesystem host interface -- and
                // hands us the already-flattened parameters as a JSON array
                // of [name, value] pairs in args[2] instead of a path.
                let Some(params_json) = args.get(2) else {
                    render::println("ERROR: missing pre-parsed param data");
                    render::exit(1);
                    return;
                };
                let entries: Vec<(String, serde_json::Value)> =
                    match serde_json::from_str(params_json) {
                        Ok(v) => v,
                        Err(e) => {
                            render::eprintln(&format!("ERROR: bad param data: {e}"));
                            render::exit(1);
                            return;
                        }
                    };
                let svc = format!("{node}/set_parameters");
                let client = match ros::connect_service(&svc, "rcl_interfaces/srv/SetParameters")
                {
                    Ok(c) => c,
                    Err(e) => {
                        render::eprintln(&format!("ERROR: connect to {svc}: {e}"));
                        render::exit(1);
                        return;
                    }
                };
                let params_field: Vec<String> = entries
                    .iter()
                    .map(|(name, value)| {
                        let (type_id, value_json) = infer_param_value_json(value);
                        format!(r#"{{"name":"{name}","value":{{"type":{type_id},{value_json}}}}}"#)
                    })
                    .collect();
                let req = format!(r#"{{"parameters":[{}]}}"#, params_field.join(","));
                match client.call(&req, 5000) {
                    Ok(resp) => {
                        render::println(&resp);
                        render::exit(0);
                    }
                    Err(e) => {
                        render::eprintln(&format!("ERROR: {e}"));
                        render::exit(1);
                    }
                }
            }
            other => {
                render::eprintln(&format!("unknown param subcommand: {other}"));
                render::println(
                    "Usage: hu meter param list|get|set|dump|describe|load <node> [<param>] [<value>]",
                );
                render::exit(1);
            }
        }
    }

    fn cmd_action(&mut self, args: &[String]) {
        // hu meter action list|info|send-goal <name> [args]
        // hu meter action echo <name> --msg-type <action_type> [--count <n>]
        let subcmd = match args.first() {
            Some(s) => s.as_str(),
            None => {
                render::println("Usage: hu meter action list|info|send-goal|echo <name> [args]");
                render::println("  Example: hu meter action send-goal /fibonacci --payload <hex>");
                render::println("  Example: hu meter action echo /fibonacci --msg-type example_interfaces/action/Fibonacci");
                render::exit(1);
                return;
            }
        };
        match subcmd {
            "list" => {
                let actions = list_actions();
                if self.json {
                    render::println(&format!(
                        "[{}]",
                        actions
                            .iter()
                            .map(|(name, type_name)| format!(
                                "{{\"name\":\"{name}\",\"type\":\"{type_name}\"}}"
                            ))
                            .collect::<Vec<_>>()
                            .join(",")
                    ));
                } else {
                    for (name, type_name) in &actions {
                        render::println(&format!("{name}\t[{type_name}]"));
                    }
                }
                render::exit(0);
            }
            "info" => {
                let Some(name) = args.get(1) else {
                    render::println("Usage: hu meter action info <name>");
                    render::exit(1);
                    return;
                };
                let send_goal_svc = format!("{name}/_action/send_goal");
                let Some(svc) = graph::list_services()
                    .into_iter()
                    .find(|s| &s.name == &send_goal_svc)
                else {
                    render::eprintln(&format!("action not found: {name}"));
                    render::exit(1);
                    return;
                };
                let action_type = svc.type_name.strip_suffix("_SendGoal").unwrap_or(&svc.type_name);
                if self.json {
                    render::println(&format!(
                        "{{\"name\":\"{name}\",\"type\":\"{action_type}\",\"servers\":{}}}",
                        svc.servers
                    ));
                } else {
                    render::println(&format!("Action: {name}"));
                    render::println(&format!("Type: {action_type}"));
                    render::println(&format!("Servers: {}", svc.servers));
                }
                render::exit(0);
            }
            "send-goal" => {
                let Some(name) = args.get(1) else {
                    render::println(
                        "Usage: hu meter action send-goal <name> --payload <hex> [--timeout <s>]",
                    );
                    render::exit(1);
                    return;
                };
                let Some(hex_payload) = flag_value(args, "--payload") else {
                    render::println("ERROR: --payload <hex-bytes> required");
                    render::exit(1);
                    return;
                };
                let payload = match parse_hex_bytes(&hex_payload) {
                    Ok(b) => b,
                    Err(e) => {
                        render::eprintln(&format!("ERROR: bad --payload: {e}"));
                        render::exit(1);
                        return;
                    }
                };
                let timeout_ms: u32 = flag_value(args, "--timeout")
                    .and_then(|v| v.parse().ok())
                    .map(|s: u32| s.saturating_mul(1000))
                    .unwrap_or(30000);
                let send_goal_svc = format!("{name}/_action/send_goal");
                let send_goal_type = graph::list_services()
                    .into_iter()
                    .find(|s| s.name == send_goal_svc)
                    .map(|s| s.type_name)
                    .unwrap_or_default();
                let client = match ros::connect_service(&send_goal_svc, &send_goal_type) {
                    Ok(c) => c,
                    Err(e) => {
                        render::eprintln(&format!("ERROR: connect to {send_goal_svc}: {e}"));
                        render::exit(1);
                        return;
                    }
                };
                match client.call_raw(&payload, timeout_ms) {
                    Ok(resp) => {
                        render::println(&format!(
                            "response ({} bytes): {}",
                            resp.len(),
                            bytes_to_hex(&resp)
                        ));
                        render::exit(0);
                    }
                    Err(e) => {
                        render::eprintln(&format!("ERROR: send-goal failed: {e}"));
                        render::exit(1);
                    }
                }
            }
            "echo" => {
                // Subscribe to <action_name>/_action/feedback
                let action_name = match args.get(1) {
                    Some(n) => n.clone(),
                    None => {
                        render::println(
                            "Usage: hu meter action echo <name> --msg-type <type> [--count <n>]",
                        );
                        render::exit(1);
                        return;
                    }
                };
                // --msg-type is part of the documented `action echo` surface;
                // require it even though feedback is decoded via the discovered
                // schema (ros::subscribe), not from this type name.
                if flag_value(args, "--msg-type").is_none() {
                    render::println("ERROR: --msg-type required");
                    render::exit(1);
                    return;
                }
                let count = flag_value(args, "--count")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0usize);
                // Explicit --timeout always applies. Absent that, fall back to a
                // safety bound so `action echo` can never hang forever waiting
                // for feedback that never arrives -- same fix as cmd_echo's
                // DEFAULT_ECHO_TIMEOUT_TICKS, which this mode never had.
                const DEFAULT_ACTION_ECHO_TIMEOUT_TICKS: u32 = 30;
                self.duration_ticks = flag_value(args, "--timeout")
                    .and_then(|v| v.parse::<f64>().ok())
                    .map(|s| s.ceil().max(1.0) as u32)
                    .unwrap_or(DEFAULT_ACTION_ECHO_TIMEOUT_TICKS);
                let feedback_topic = format!("{action_name}/_action/feedback");
                let sub = match ros::subscribe(&feedback_topic) {
                    Ok(s) => s,
                    Err(e) => {
                        render::eprintln(&format!("Failed to subscribe to {feedback_topic}: {e}"));
                        render::exit(1);
                        return;
                    }
                };
                self.mode = Mode::ActionEcho {
                    sub: Some(sub),
                    count,
                    printed: 0,
                };
            }
            other => {
                render::eprintln(&format!("unknown action subcommand: {other}"));
                render::println(
                    "Usage: hu meter action list|info|send-goal|echo <name> [args]",
                );
                render::exit(1);
            }
        }
    }

    fn on_tick(&mut self) {
        self.ticks += 1;
        let done = self.duration_ticks > 0 && self.ticks >= self.duration_ticks;

        match &mut self.mode {
            Mode::Hz { topic, sub } => {
                let window_ms = 1000u32;
                match ros::measure_hz(topic, window_ms) {
                    Ok(m) => {
                        if self.json {
                            render::println(&format!(
                                "{{\"topic\":\"{}\",\"rate_hz\":{:.3},\"samples\":{}}}",
                                m.topic, m.rate_hz, m.sample_count
                            ));
                        } else {
                            render::println(&format!(
                                "{}: {:.3} Hz  ({} samples)",
                                m.topic, m.rate_hz, m.sample_count
                            ));
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
                    Ok(m) => {
                        if self.json {
                            render::println(&format!(
                                "{{\"topic\":\"{}\",\"rate_kbps\":{:.3},\"samples\":{}}}",
                                m.topic, m.rate_kbps, m.sample_count
                            ));
                        } else {
                            render::println(&format!(
                                "{}: {:.3} KB/s  ({} samples)",
                                m.topic, m.rate_kbps, m.sample_count
                            ));
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
                field,
            } => {
                let Some(s) = sub.as_ref() else {
                    return;
                };
                while let Some(json_msg) = s.try_recv() {
                    let t = topic.clone();
                    *printed += 1;
                    if let Some(ref fp) = field {
                        let extracted = extract_field(&json_msg, fp);
                        render::println(&format!("[{t}] {extracted}"));
                    } else {
                        render::println(&format!("[{t}] {json_msg}"));
                    }
                    if *count > 0 && *printed >= *count {
                        render::exit(0);
                        self.mode = Mode::Done;
                        return;
                    }
                }
                if done {
                    render::exit(0);
                    self.mode = Mode::Done;
                }
            }
            Mode::EchoRaw {
                topic,
                sub,
                count,
                printed,
            } => {
                let Some(s) = sub.as_ref() else {
                    return;
                };
                while let Some(bytes) = s.try_recv() {
                    *printed += 1;
                    render::println(&format!("[{topic}] {}", bytes_to_hex(&bytes)));
                    if *count > 0 && *printed >= *count {
                        render::exit(0);
                        self.mode = Mode::Done;
                        return;
                    }
                }
                if done {
                    render::exit(0);
                    self.mode = Mode::Done;
                }
            }
            Mode::Delay { topic, sub } => {
                let Some(s) = sub.as_ref() else {
                    return;
                };
                while let Some(json_msg) = s.try_recv() {
                    let delay_note = extract_delay_note(&json_msg);
                    let t = topic.clone();
                    render::println(&format!("[{t}] {delay_note}"));
                }
                if done {
                    render::exit(0);
                    self.mode = Mode::Done;
                }
            }
            Mode::Pub {
                pub_,
                cdr,
                topic,
                interval_ticks,
                times_remaining,
                ticks_since_last,
            } => {
                *ticks_since_last += 1;
                let ready = if *interval_ticks == 0 {
                    true // no rate limit: publish each tick
                } else {
                    *ticks_since_last >= *interval_ticks
                };
                if ready && *times_remaining > 0 {
                    *ticks_since_last = 0;
                    if let Err(e) = pub_.publish(cdr) {
                        render::println(&format!("publish error: {e}"));
                        render::exit(1);
                        self.mode = Mode::Done;
                        return;
                    }
                    let t = topic.clone();
                    render::println(&format!("Published to {t}"));
                    *times_remaining -= 1;
                    if *times_remaining == 0 {
                        render::exit(0);
                        self.mode = Mode::Done;
                    }
                }
            }
            Mode::ActionEcho {
                sub,
                count,
                printed,
            } => {
                let Some(s) = sub.as_ref() else {
                    return;
                };
                while let Some(json_msg) = s.try_recv() {
                    *printed += 1;
                    render::println(&json_msg);
                    if *count > 0 && *printed >= *count {
                        render::exit(0);
                        self.mode = Mode::Done;
                        return;
                    }
                }
                if done {
                    render::exit(0);
                    self.mode = Mode::Done;
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

// Best-effort note for `hu meter delay`. A full implementation would parse the
// decoded-JSON message and compute (receive-time − header.stamp); for now it
// surfaces the raw message so the receive path is observable.
fn extract_delay_note(json: &str) -> String {
    format!("(raw) {json}")
}

fn parse_topic_duration(args: &[String]) -> (Option<String>, u32) {
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
    (topic, duration_ticks)
}

fn is_hidden(name: &str) -> bool {
    name.split('/').any(|seg| seg.starts_with('_'))
}

/// Infer a rcl_interfaces ParameterValue type_id and JSON field for `param set`.
/// Returns (type_id, field_json) where field_json is e.g. `"bool_value":true`.
fn infer_param_value(s: &str) -> (u8, String) {
    if s == "true" || s == "True" {
        return (1, r#""bool_value":true"#.to_string());
    }
    if s == "false" || s == "False" {
        return (1, r#""bool_value":false"#.to_string());
    }
    if let Ok(i) = s.parse::<i64>() {
        return (2, format!(r#""integer_value":{i}"#));
    }
    if let Ok(f) = s.parse::<f64>() {
        return (3, format!(r#""double_value":{f}"#));
    }
    // Use serde_json to serialize the string so control characters (newlines,
    // tabs, etc.) are escaped correctly and the request stays valid JSON.
    let quoted = serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string());
    (4, format!(r#""string_value":{quoted}"#))
}

/// Serialize a string as a JSON string literal (with surrounding quotes),
/// escaping quotes, backslashes, and control characters so the emitted
/// `--json` output stays valid JSON regardless of topic/type contents.
fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

/// Same as `infer_param_value` but from an already-typed JSON value (as
/// produced by the host's YAML-file parser for `param load`), so a numeric
/// YAML scalar like `55` isn't round-tripped through string parsing.
fn infer_param_value_json(v: &serde_json::Value) -> (u8, String) {
    match v {
        serde_json::Value::Bool(b) => (1, format!(r#""bool_value":{b}"#)),
        serde_json::Value::Number(n) if n.is_i64() || n.is_u64() => {
            (2, format!(r#""integer_value":{n}"#))
        }
        serde_json::Value::Number(n) => (3, format!(r#""double_value":{n}"#)),
        serde_json::Value::String(s) => {
            // An explicit JSON string stays a string — do NOT re-infer its
            // content (a quoted "true" or "55" must remain a string_value).
            let quoted = serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string());
            (4, format!(r#""string_value":{quoted}"#))
        }
        other => infer_param_value(&other.to_string()),
    }
}

/// Calls `<node>/list_parameters` and returns the parameter names.
fn list_param_names(node: &str) -> Result<Vec<String>, String> {
    let svc = format!("{node}/list_parameters");
    let client = ros::connect_service(&svc, "rcl_interfaces/srv/ListParameters")
        .map_err(|e| format!("connect to {svc}: {e}"))?;
    let resp = client
        .call(r#"{"prefixes":[],"depth":0}"#, 5000)
        .map_err(|e| e.to_string())?;
    let value: serde_json::Value =
        serde_json::from_str(&resp).map_err(|e| format!("bad list_parameters response: {e}"))?;
    let names = value["result"]["names"]
        .as_array()
        .ok_or("list_parameters response missing result.names")?;
    Ok(names
        .iter()
        .filter_map(|n| n.as_str().map(str::to_string))
        .collect())
}

/// Calls `<node>/get_parameters` for the given names and returns each value
/// re-expressed as a JSON literal (e.g. `"42"`, `"\"hello\""`), in request order.
fn get_param_values(node: &str, names: &[String]) -> Result<Vec<String>, String> {
    let svc = format!("{node}/get_parameters");
    let client = ros::connect_service(&svc, "rcl_interfaces/srv/GetParameters")
        .map_err(|e| format!("connect to {svc}: {e}"))?;
    let names_json = names
        .iter()
        .map(|n| format!("\"{n}\""))
        .collect::<Vec<_>>()
        .join(",");
    let req = format!(r#"{{"names":[{names_json}]}}"#);
    let resp = client.call(&req, 5000).map_err(|e| e.to_string())?;
    let value: serde_json::Value =
        serde_json::from_str(&resp).map_err(|e| format!("bad get_parameters response: {e}"))?;
    let values = value["values"]
        .as_array()
        .ok_or("get_parameters response missing values")?;
    Ok(values.iter().map(param_value_literal).collect())
}

/// Extracts the ROS2 `rcl_interfaces/ParameterValue` union's active field as a
/// JSON literal, based on its `type` discriminant (1=bool, 2=integer,
/// 3=double, 4=string, 5=byte_array, 6=bool_array, 7=integer_array,
/// 8=double_array, 9=string_array). Any other discriminant falls back to the
/// raw JSON value.
fn param_value_literal(v: &serde_json::Value) -> String {
    let ty = v["type"].as_u64().unwrap_or(0);
    let field = match ty {
        1 => &v["bool_value"],
        2 => &v["integer_value"],
        3 => &v["double_value"],
        4 => &v["string_value"],
        5 => &v["byte_array_value"],
        6 => &v["bool_array_value"],
        7 => &v["integer_array_value"],
        8 => &v["double_array_value"],
        9 => &v["string_array_value"],
        // Unknown/extended discriminant: fall back to the raw JSON value so
        // newer ParameterValue variants aren't silently hidden as "null".
        _ => return v.to_string(),
    };
    if field.is_null() {
        "null".to_string()
    } else {
        field.to_string()
    }
}

/// Parses a whitespace-separated hex byte string (e.g. `"00 01 02"`) into bytes.
fn parse_hex_bytes(s: &str) -> Result<Vec<u8>, String> {
    s.split_whitespace()
        .map(|tok| u8::from_str_radix(tok, 16).map_err(|e| format!("invalid hex byte '{tok}': {e}")))
        .collect()
}

/// Derives the list of ROS2 actions from the service graph: an action server
/// always registers a `<action>/_action/send_goal` service of type
/// `<ActionType>_SendGoal` — no dedicated "action" entity exists in the graph.
fn list_actions() -> Vec<(String, String)> {
    graph::list_services()
        .into_iter()
        .filter_map(|s| {
            let name = s.name.strip_suffix("/_action/send_goal")?.to_string();
            let type_name = s
                .type_name
                .strip_suffix("_SendGoal")
                .unwrap_or(&s.type_name)
                .to_string();
            Some((name, type_name))
        })
        .collect()
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Extract a dot-separated field path from a JSON string.
/// E.g. "header.stamp.sec" on `{"header":{"stamp":{"sec":42,...},...},...}` → "42".
/// Descends into nested objects segment by segment using serde_json; a leaf
/// string is returned unquoted, any other leaf (number/bool/object/array) as
/// its raw JSON. Falls back to "(field not found)" when the path does not
/// resolve, or "(invalid JSON)" when the input does not parse.
fn extract_field(json: &str, path: &str) -> String {
    let value: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(e) => return format!("(invalid JSON: {e})"),
    };
    let mut current = &value;
    for segment in path.split('.') {
        match current.get(segment) {
            Some(next) => current = next,
            None => return format!("(field '{segment}' not found in path '{path}')"),
        }
    }
    match current {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

// ─── Plugin entry points ──────────────────────────────────────────────────────
//
// wit-bindgen resource handles (ros::Subscription, etc.) aren't Sync, but WASM
// components are single-threaded, so wrapping state in AssertSync is safe.
struct AssertSync<T>(T);
// SAFETY: WASM components run on a single thread; no concurrent access is possible.
unsafe impl<T> Sync for AssertSync<T> {}

use std::cell::{OnceCell, RefCell};

static STATE: AssertSync<OnceCell<RefCell<HuMeter>>> = AssertSync(OnceCell::new());

fn state() -> std::cell::RefMut<'static, HuMeter> {
    STATE
        .0
        .get_or_init(|| RefCell::new(HuMeter::new()))
        .borrow_mut()
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
            subscribed_events: vec![EventKind::Startup, EventKind::Tick],
            required_permissions: vec![
                Permission::SubscribeTopic,
                Permission::PublishTopic,
                Permission::CallService,
                Permission::MeasureMetrics,
                Permission::OpenSession,
                Permission::AccessRawCdr,
            ],
        }
    }

    fn on_event(event: CliEvent) {
        match event {
            CliEvent::Startup(args) => state().startup(args),
            CliEvent::Tick => state().on_tick(),
            CliEvent::Interrupt => render::exit(130),
        }
    }
}

export!(Plugin);
