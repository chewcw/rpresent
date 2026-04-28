use std::sync::Once;

use tracing_log::LogTracer;
use tracing_subscriber::{fmt, EnvFilter};

static INIT: Once = Once::new();

pub fn init() {
    INIT.call_once(|| {
        let filter =
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

        let _ = LogTracer::init();
        let _ = fmt().with_env_filter(filter).compact().try_init();
    });
}

#[cfg(test)]
mod tests {
    use super::init;

    #[test]
    fn init_is_safe_to_call_more_than_once() {
        init();
        init();
    }
}