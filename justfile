help:
    @echo "Available commands:"
    @echo "  run-spotlight - Start the spotlight application and toggle it on"

run-daemon:
    RUST_LOG=trace cargo run --package rpresent -- --config ./config.toml &

run-spotlight: run-daemon
    RUST_LOG=trace cargo run --package rpresent -- spot=toggle