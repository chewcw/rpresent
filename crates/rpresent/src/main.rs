fn main() {
    rpresent::logging::init();
    if let Err(error) = rpresent::run_from_env() {
        tracing::error!(error = %error, "rpresent exited with error");
        std::process::exit(1);
    }
}
