use tracing_subscriber::{EnvFilter, fmt};

pub fn init_logger(json_mode: bool, debug: bool) {
    // Build filter from RUST_LOG environment variable or default
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        if debug {
            EnvFilter::new("hiroz=debug,zenoh=debug")
        } else {
            EnvFilter::new("hiroz=info,zenoh=warn")
        }
    });

    if json_mode {
        // Structured JSON logs to stderr (for real-time visibility)
        fmt()
            .json()
            .with_target(true)
            .with_current_span(false)
            .with_writer(std::io::stderr)
            .with_ansi(false)
            .with_env_filter(filter)
            .init();
    } else {
        // Human-readable logs to stderr
        fmt()
            .compact()
            .with_writer(std::io::stderr)
            .with_ansi(true)
            .with_env_filter(filter)
            .init();
    }
}
