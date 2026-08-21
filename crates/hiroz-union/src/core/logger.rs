use tracing_subscriber::{EnvFilter, fmt};

/// Default filter directives when `RUST_LOG` is unset.
///
/// `hu` must be listed explicitly: the binary target is named `hu`
/// (`[[bin]] name = "hu"`), so every `tracing::` call in this crate emits under
/// the `hu` target. A filter naming only `hiroz` and `zenoh` drops all of them,
/// which is why the host's schema-discovery and decode warnings were emitted and
/// then discarded before reaching a terminal.
pub(crate) fn default_filter(debug: bool) -> &'static str {
    if debug {
        "hu=debug,hiroz=debug,zenoh=debug"
    } else {
        "hu=info,hiroz=info,zenoh=warn"
    }
}

pub fn init_logger(json_mode: bool, debug: bool) {
    // Build filter from RUST_LOG environment variable or default
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter(debug)));

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

#[cfg(test)]
mod tests {
    use super::default_filter;
    use tracing_subscriber::EnvFilter;

    // The whole of the `hu` target fix is one string literal, and it is exactly
    // the kind of line a later edit drops without noticing.
    #[test]
    fn defaults_name_the_hu_target_and_parse() {
        for debug in [false, true] {
            let directives = default_filter(debug);
            assert!(
                directives.contains("hu="),
                "default filter for debug={debug} does not name the `hu` target: {directives}"
            );
            EnvFilter::try_new(directives)
                .unwrap_or_else(|e| panic!("default filter {directives:?} does not parse: {e}"));
        }
    }
}
