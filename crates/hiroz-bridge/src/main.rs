fn main() {
    let argv: Vec<String> = std::env::args().collect();
    std::process::exit(hiroz_bridge::run_argv(&argv));
}
