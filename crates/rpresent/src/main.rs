fn main() {
    if let Err(error) = rpresent::run_from_env() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
