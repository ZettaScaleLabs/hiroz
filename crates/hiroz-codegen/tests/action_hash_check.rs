use std::path::PathBuf;

fn assets_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/jazzy")
}

#[test]
fn test_fibonacci_get_result_hash() {
    use hiroz_codegen::{
        discovery::{discover_actions, discover_messages},
        resolver::Resolver,
    };

    let assets = assets_dir();
    let packages = [
        "builtin_interfaces",
        "unique_identifier_msgs",
        "action_msgs",
        "service_msgs",
        "action_tutorials_interfaces",
    ];
    let mut all_messages = Vec::new();
    for pkg in &packages {
        let pkg_path = assets.join(pkg);
        let msgs = discover_messages(&pkg_path, pkg).unwrap_or_default();
        all_messages.extend(msgs);
    }

    let mut resolver = Resolver::new(false);
    resolver
        .resolve_messages(all_messages)
        .expect("resolve messages");

    let pkg_path = assets.join("action_tutorials_interfaces");
    let actions =
        discover_actions(&pkg_path, "action_tutorials_interfaces").expect("discover actions");
    let resolved = resolver.resolve_actions(actions).expect("resolve actions");
    let fib = resolved
        .iter()
        .find(|a| a.parsed.name == "Fibonacci")
        .expect("Fibonacci action");

    println!("get_result_hash: {}", fib.get_result_hash.to_rihs_string());
    println!("status_hash:     {}", fib.status_hash.to_rihs_string());
    println!("send_goal_hash:  {}", fib.send_goal_hash.to_rihs_string());

    // Expected hash from rmw_zenoh_cpp interop test verification
    assert_eq!(
        fib.get_result_hash.to_rihs_string(),
        "RIHS01_8b47e383f1e31f6d8df6417ab54957e7d5ea24dad315646ad711ac3fdea81d58",
        "get_result hash mismatch"
    );
    assert_eq!(
        fib.status_hash.to_rihs_string(),
        "RIHS01_6c1684b00f177d37438febe6e709fc4e2b0d4248dca4854946f9ed8b30cda83e",
        "status hash mismatch"
    );
}
