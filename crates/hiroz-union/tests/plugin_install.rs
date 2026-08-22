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
//! The success paths are here too. They were absent for a reason that was real
//! but did not apply. `hiroz-union` cannot build a plugin, because the plugins
//! are a separate, `exclude`d, `wasm32-wasip2` workspace. These tests do not
//! need one. `validate_plugin_static` compiles the file as a component and does
//! not instantiate it, so the 8-byte empty component below is enough to drive
//! install, list and uninstall end to end. It cannot dispatch, and no test here
//! claims it can.
//!
//! An earlier version of this comment said the success paths lived at the end
//! of `scripts/ci/hu-tests.sh`. They did not: that script contains no
//! `hu plugin install` line at all, and grepping for one is how the claim was
//! caught. A comment asserting coverage hides a gap exactly as well as a doc
//! asserting behaviour, and this file has now made that mistake twice.
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

/// A valid, empty WASM component: the 4-byte magic, then the component-model
/// version (0x0d) and layer (0x01). `Component::from_file` compiles it, which
/// is all `validate_plugin_static` asks of a plugin, so it exercises every step
/// of installation without needing the `wasm32-wasip2` toolchain. It exports
/// nothing, so nothing can dispatch it. That is a separate concern, and no test
/// here claims otherwise.
const EMPTY_COMPONENT: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00];

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// This test's private HOME. Stable within a test, distinct between tests, so a
/// test can install and then list or uninstall against the same state.
fn test_home() -> PathBuf {
    std::env::temp_dir().join(format!(
        "hu-install-test-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ))
}

fn managed_dir(home: &std::path::Path) -> PathBuf {
    home.join(".local/share/hu/plugins")
}

/// Run `hu plugin <args>` against an existing HOME, without wiping it.
fn hu(home: &std::path::Path, args: &[&str]) -> Outcome {
    let out = Command::new(env!("CARGO_BIN_EXE_hu"))
        .arg("plugin")
        .args(args)
        .env("HOME", home)
        .env_remove("HU_PLUGIN_PATH")
        .env_remove("HU_PLUGIN_REGISTRY")
        .env_remove("HU_RELEASE_TOKEN")
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
        home: home.to_path_buf(),
    }
}

/// Run `hu plugin install <args>` with a fresh, isolated HOME.
fn install(args: &[&str]) -> Outcome {
    let home = test_home();
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
        .env_remove("HU_RELEASE_TOKEN")
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
    // Not a URL and not an existing file: the message has to name the way out,
    // or the user is stuck.
    //
    // `--registry` points at a loopback port that serves 404 rather than being
    // omitted. Omitting it falls back to the real GitHub index, so this test
    // reached the network -- and would have started FAILING, not erroring, the
    // day a release exists for this CARGO_PKG_VERSION and the install succeeds.
    let base = serve(HashMap::new());
    let registry = format!("{base}/hu-plugins.json");
    let r = install(&["meter", "--registry", &registry]);
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

// --------------------------------------------------------------- success
//
// Every test above is a refusal. These are the paths a user actually takes,
// and until now not one of them had ever run. Review found the metadata
// defects this file now covers, because no test ever completed an install and
// then looked at the result.

#[test]
fn local_install_places_the_file_and_records_its_provenance() {
    let home = test_home();
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();

    let src = home.join("hu_meter.wasm");
    std::fs::write(&src, EMPTY_COMPONENT).unwrap();

    let r = hu(&home, &["install", src.to_str().unwrap()]);
    assert!(r.ok, "local install should succeed, got:\n{}", r.output);
    assert!(
        managed_dir(&home).join("hu_meter.wasm").exists(),
        "the plugin is not in the managed directory:\n{}",
        r.output
    );

    let list = hu(&home, &["list"]);
    assert!(list.ok, "{}", list.output);
    assert!(list.output.contains("meter"), "{}", list.output);
    // A local file states no version, so the listing must not invent one. It
    // used to print `local` here -- the source KIND in the version column.
    assert!(
        list.output.contains("local"),
        "the source column should say local:\n{}",
        list.output
    );
    assert!(
        !list.output.contains("VERSION local") && !list.output.contains(" local       local"),
        "the version column must not carry the source kind:\n{}",
        list.output
    );
}

#[test]
fn url_install_verifies_the_sidecar_and_installs() {
    let sha = sha256_hex(EMPTY_COMPONENT);
    let mut routes = HashMap::new();
    routes.insert(
        "/hu_meter.wasm".to_string(),
        (200u16, EMPTY_COMPONENT.to_vec()),
    );
    routes.insert(
        "/hu_meter.wasm.sha256".to_string(),
        (200u16, format!("{sha}  hu_meter.wasm\n").into_bytes()),
    );
    let base = serve(routes);

    let r = install(&[&format!("{base}/hu_meter.wasm")]);
    assert!(r.ok, "url install should succeed, got:\n{}", r.output);
    assert!(
        managed_dir(&r.home).join("hu_meter.wasm").exists(),
        "{}",
        r.output
    );

    let list = hu(&r.home, &["list", "--json"]);
    assert!(list.ok, "{}", list.output);
    // The recorded origin must be the URL, and the source kind must be `url`
    // (rendered as `download`), not a path sniffed at display time.
    assert!(
        list.output.contains("\"source\": \"url\""),
        "source kind should be recorded as url:\n{}",
        list.output
    );
    assert!(
        list.output.contains("\"version\": null"),
        "a bare URL states no version:\n{}",
        list.output
    );
}

#[test]
fn url_install_does_not_persist_a_credential_from_the_source_url() {
    let sha = sha256_hex(EMPTY_COMPONENT);
    let mut routes = HashMap::new();
    routes.insert(
        "/hu_meter.wasm".to_string(),
        (200u16, EMPTY_COMPONENT.to_vec()),
    );
    routes.insert(
        "/hu_meter.wasm.sha256".to_string(),
        (200u16, format!("{sha}  hu_meter.wasm\n").into_bytes()),
    );
    let base = serve(routes);

    // A signed asset URL carries its token in the query string, and
    // `hu plugin list --json` prints the recorded origin verbatim.
    let r = install(&[&format!("{base}/hu_meter.wasm?token=SUPERSECRET")]);
    assert!(r.ok, "install should succeed, got:\n{}", r.output);

    let list = hu(&r.home, &["list", "--json"]);
    assert!(
        !list.output.contains("SUPERSECRET"),
        "the query string must not reach installed.json:\n{}",
        list.output
    );
    let db =
        std::fs::read_to_string(managed_dir(&r.home).join("installed.json")).unwrap_or_default();
    assert!(!db.is_empty(), "no install record was written");
    assert!(
        !db.contains("SUPERSECRET"),
        "the query string must not be persisted:\n{db}"
    );
}

#[test]
fn registry_install_records_the_version_the_index_states() {
    let sha = sha256_hex(EMPTY_COMPONENT);
    let mut routes = HashMap::new();
    routes.insert(
        "/index.json".to_string(),
        (200u16, index_json("hu:plugin@0.1.0", "hu_meter.wasm", &sha)),
    );
    routes.insert(
        "/hu_meter.wasm".to_string(),
        (200u16, EMPTY_COMPONENT.to_vec()),
    );
    let base = serve(routes);

    let r = install(&["meter", "--registry", &format!("{base}/index.json")]);
    assert!(r.ok, "registry install should succeed, got:\n{}", r.output);

    let list = hu(&r.home, &["list", "--json"]);
    assert!(
        list.output.contains("\"version\": \"0.1.0\""),
        "the index states 0.1.0 and the record should carry it:\n{}",
        list.output
    );
    assert!(
        list.output.contains("\"source\": \"registry\""),
        "source kind should be registry:\n{}",
        list.output
    );
}

#[test]
fn uninstall_removes_both_the_file_and_the_record() {
    let home = test_home();
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let src = home.join("hu_meter.wasm");
    std::fs::write(&src, EMPTY_COMPONENT).unwrap();

    assert!(hu(&home, &["install", src.to_str().unwrap()]).ok);
    let installed = managed_dir(&home).join("hu_meter.wasm");
    assert!(installed.exists());

    let r = hu(&home, &["uninstall", "meter"]);
    assert!(r.ok, "uninstall should succeed, got:\n{}", r.output);
    assert!(!installed.exists(), "the file survived:\n{}", r.output);

    // The record must go with it. hu would otherwise attach a surviving entry
    // to the next plugin installed under the same name.
    let list = hu(&home, &["list", "--json"]);
    assert!(
        !list.output.contains("hu_meter.wasm"),
        "the install record survived the uninstall:\n{}",
        list.output
    );
}
