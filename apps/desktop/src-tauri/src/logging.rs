use std::fmt;

/// A value that is safe to put in a structured event without revealing the
/// value it represents. Use this for credential references and secret-bearing
/// material even when the event is emitted at TRACE.
#[derive(Clone, Copy)]
pub(crate) struct Redacted;

impl fmt::Debug for Redacted {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted>")
    }
}

impl fmt::Display for Redacted {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted>")
    }
}

pub(crate) const REDACTED: Redacted = Redacted;

/// Install the process-local logger. Logs go to stderr and are intentionally
/// quiet by default; no file logger, clipboard logger, or diagnostic export is
/// installed here.
pub(crate) fn init() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("mobarust=warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_target(true)
        .compact()
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacted_fields_never_render_their_source_value() {
        assert_eq!(format!("{:?}", REDACTED), "<redacted>");
        assert_eq!(format!("{REDACTED}"), "<redacted>");
    }
}
