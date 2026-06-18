pub fn print_manifest() {
    let manifest = serde_json::json!({
        "name": "meter",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Rate, bandwidth, echo, service, and param commands for ROS 2",
        "commands": ["hz", "bw", "delay", "echo", "list", "info", "service", "param"],
    });
    println!("{}", serde_json::to_string_pretty(&manifest).unwrap());
}
