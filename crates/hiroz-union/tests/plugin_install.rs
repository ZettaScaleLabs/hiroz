//! `hu plugin install` over the network: URL and registry sources.
//!
//! These drive the **real binary** via `CARGO_BIN_EXE_hu` rather than calling
//! the functions directly. That is not a stylistic choice: `hiroz-union` is a
//! binary-only crate with no lib target, so an integration test cannot `use`
//! it. Driving the CLI also covers argument parsing and exit status, which is
//! what a user and a script actually depend on.
//!
//! Every test here is a **refusal**. Those are the paths that had never once
//! executed — including the WIT world-mismatch check, which is the kind of
//! guard that looks correct forever and is never proven to fire.
//!
//! The success paths are **not** here, because they need a genuine WASM
//! component and this crate cannot build one: the plugins are a separate,
//! `exclude`d, `wasm32-wasip2` workspace. They live at the end of
//! `scripts/ci/hu-tests.sh`, which has already built real plugins by that
//! point — install by URL with a `.sha256` sidecar, install by registry name
//! through a served index, dispatch, and uninstall.
//!
//! An earlier version of this comment claimed that script and the
//! docs-reproduction suite already covered them. Neither did: the script
//! touched only `plugin validate` and `plugin list`, and every
//! `hu plugin install` line in the docs is `skip`. The claim is why nobody
//! noticed for so long — a comment asserting coverage is as good at hiding a
//! gap as a doc asserting behaviour.
//!
//! The server is a few lines of `std::net` on a loopback ephemeral port: no
//! new dependency, and no network access, so these stay runnable offline.

use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::Command,
    sync::Arc,
};

/// A canned response: HTTP status and body.
type Route = (u16, Vec<u8>);

/// Serve a fixed route table on loopback until the test drops the handle.
/// Returns the base URL.
fn serve(routes: HashMap<String, Route>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    let routes = Arc::new(routes);

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let routes = Arc::clone(&routes);
            std::thread::spawn(move || handle(stream, &routes));
        }
    });

    format!("http://127.0.0.1:{port}")
}

fn handle(mut stream: TcpStream, routes: &HashMap<String, Route>) {
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    // "GET /path HTTP/1.1"
    let path = request_line.split_whitespace().nth(1).unwrap_or("/");

    let (status, body) = routes
        .get(path)
        .cloned()
        .unwrap_or((404, b"not found".to_vec()));

    let head = format!(
        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&body);
    let _ = stream.flush();
}

struct Outcome {
    ok: bool,
    output: String,
    home: PathBuf,
}

impl Outcome {
    /// Nothing may be left in the plugin directory after a refusal — a
    /// partially-written plugin is worse than none, because discovery would
    /// pick it up.
    fn installed_plugins(&self) -> Vec<String> {
        let dir = self.home.join(".local/share/hu/plugins");
        std::fs::read_dir(dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Run `hu plugin install <args>` with an isolated HOME.
fn install(args: &[&str]) -> Outcome {
    let home = std::env::temp_dir().join(format!(
        "hu-install-test-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_hu"))
        .arg("plugin")
        .arg("install")
        .args(args)
        .env("HOME", &home)
        // Discovery must not reach a build tree during these tests.
        .env_remove("HU_PLUGIN_PATH")
        .env_remove("HU_PLUGIN_REGISTRY")
        .output()
        .expect("run hu");

    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    Outcome {
        ok: out.status.success(),
        output,
        home,
    }
}

fn index_json(world: &str, file: &str, sha: &str) -> Vec<u8> {
    format!(
        r#"{{"schema":1,"hu_version":"0.1.0","wit_world":"{world}",
            "plugins":[{{"name":"meter","file":"{file}","version":"0.1.0",
            "sha256":"{sha}","world":"hu-cli-plugin","description":"d"}}]}}"#
    )
    .into_bytes()
}

#[test]
fn url_install_refuses_a_404_instead_of_installing_the_error_page() {
    let base = serve(HashMap::new()); // every path 404s
    let r = install(&[&format!("{base}/hu_meter.wasm")]);

    assert!(!r.ok, "a 404 must fail, output was:\n{}", r.output);
    assert!(
        r.output.contains("download failed") || r.output.contains("error"),
        "should say the download failed, got:\n{}",
        r.output
    );
    assert!(
        r.installed_plugins().is_empty(),
        "nothing may be installed after a 404, found {:?}",
        r.installed_plugins()
    );
}

#[test]
fn url_install_refuses_a_checksum_mismatch() {
    let mut routes = HashMap::new();
    routes.insert("/hu_meter.wasm".to_string(), (200, b"payload".to_vec()));
    // Sidecar advertises a hash the payload does not have.
    routes.insert("/hu_meter.wasm.sha256".to_string(), (200, vec![b'0'; 64]));
    let base = serve(routes);

    let r = install(&[&format!("{base}/hu_meter.wasm")]);
    assert!(!r.ok, "checksum mismatch must fail:\n{}", r.output);
    assert!(
        r.output.contains("checksum mismatch"),
        "should name the mismatch, got:\n{}",
        r.output
    );
    assert!(r.installed_plugins().is_empty());
}

#[test]
fn url_install_refuses_bytes_that_are_not_a_component() {
    let mut routes = HashMap::new();
    // No sidecar, so the checksum step is skipped and validation is what has
    // to catch this.
    routes.insert(
        "/hu_meter.wasm".to_string(),
        (200, b"definitely not a wasm component".to_vec()),
    );
    let base = serve(routes);

    let r = install(&[&format!("{base}/hu_meter.wasm")]);
    assert!(!r.ok, "a non-component must fail:\n{}", r.output);
    assert!(
        r.output.contains("not a loadable WASM component"),
        "should say it is not loadable, got:\n{}",
        r.output
    );
    assert!(
        r.installed_plugins().is_empty(),
        "the partial file must be cleaned up, found {:?}",
        r.installed_plugins()
    );
}

#[test]
fn registry_install_refuses_a_wit_world_this_hu_does_not_host() {
    let mut routes = HashMap::new();
    routes.insert(
        "/index.json".to_string(),
        (
            200,
            index_json("hu:plugin@9.9.9", "hu_meter-0.1.0.wasm", &"0".repeat(64)),
        ),
    );
    let base = serve(routes);

    let r = install(&["meter", "--registry", &format!("{base}/index.json")]);
    assert!(!r.ok, "a world mismatch must fail:\n{}", r.output);
    assert!(
        r.output.contains("9.9.9") && r.output.contains("hu:plugin@"),
        "the message must name both worlds so the user can act, got:\n{}",
        r.output
    );
    assert!(r.installed_plugins().is_empty());
}

#[test]
fn registry_install_refuses_an_unknown_name_and_lists_what_exists() {
    let mut routes = HashMap::new();
    routes.insert(
        "/index.json".to_string(),
        (
            200,
            index_json("hu:plugin@0.1.0", "hu_meter-0.1.0.wasm", &"0".repeat(64)),
        ),
    );
    let base = serve(routes);

    let r = install(&["nosuchplugin", "--registry", &format!("{base}/index.json")]);
    assert!(!r.ok, "an unknown name must fail:\n{}", r.output);
    assert!(
        r.output.contains("meter"),
        "should list what IS available, got:\n{}",
        r.output
    );
}

#[test]
fn registry_install_without_a_registry_says_how_to_configure_one() {
    // Not a URL and not an existing file, with no registry configured: the
    // message has to name the way out, or the user is stuck.
    let r = install(&["meter"]);
    assert!(!r.ok);
    assert!(
        r.output.contains("HU_PLUGIN_REGISTRY") && r.output.contains("--registry"),
        "should name both ways to configure a registry, got:\n{}",
        r.output
    );
}

// --------------------------------------------------------------- uninstall
//
// `hu plugin uninstall` had never run either. Its interesting behaviour is
// the second branch: a plugin visible on `$HU_PLUGIN_PATH` is discoverable
// but not ours to delete, and saying "not installed" there would be actively
// misleading — the user can see it in `hu plugin list`.

/// Run `hu plugin uninstall <name>`, optionally with a plugin dir on
/// `$HU_PLUGIN_PATH`.
fn uninstall(name: &str, plugin_path: Option<&std::path::Path>) -> Outcome {
    let home = std::env::temp_dir().join(format!(
        "hu-uninstall-test-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hu"));
    cmd.arg("plugin")
        .arg("uninstall")
        .arg(name)
        .env("HOME", &home);
    match plugin_path {
        Some(p) => cmd.env("HU_PLUGIN_PATH", p),
        None => cmd.env_remove("HU_PLUGIN_PATH"),
    };

    let out = cmd.output().expect("run hu");
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    Outcome {
        ok: out.status.success(),
        output,
        home,
    }
}

#[test]
fn uninstall_refuses_a_plugin_that_is_not_installed() {
    let r = uninstall("nosuchplugin", None);
    assert!(!r.ok, "removing nothing must fail:\n{}", r.output);
    assert!(
        r.output
            .contains("no installed plugin named 'nosuchplugin'"),
        "should name what it looked for, got:\n{}",
        r.output
    );
}

#[test]
fn uninstall_explains_when_the_plugin_lives_on_hu_plugin_path() {
    // A plugin here is discoverable — `hu plugin list` shows it — but it is
    // not in the directory installs manage. "Not installed" would contradict
    // what the user can see, so the message has to distinguish the two.
    let dir = std::env::temp_dir().join(format!("hu-extpath-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("hu_meter.wasm"), b"not a real component").unwrap();

    let r = uninstall("meter", Some(&dir));
    assert!(!r.ok, "must not claim success:\n{}", r.output);
    assert!(
        r.output.contains("HU_PLUGIN_PATH"),
        "should point at the real location, not say 'not installed', got:\n{}",
        r.output
    );

    let _ = std::fs::remove_dir_all(&dir);
}
