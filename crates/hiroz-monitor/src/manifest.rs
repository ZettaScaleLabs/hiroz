pub fn print_manifest() {
    let manifest = serde_json::json!({
        "name": "monitor",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Graph watch, log viewer, and log-level commands for ROS 2",
        "commands": ["watch", "graph", "log", "log-level"],
    });
    println!("{}", serde_json::to_string_pretty(&manifest).unwrap());
}
