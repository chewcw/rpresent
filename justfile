help:
    @echo "Available commands:"
    @echo "  run-spotlight - Start the spotlight application and toggle it on"

run-daemon:
    RUST_LOG=debug cargo run --package rpresent -- --config ./config.toml &

run-spotlight: run-daemon
    RUST_LOG=debug cargo run --package rpresent -- spot=toggle &

run-zoom: run-daemon
    @echo "zoom mode starts from config"
    RUST_LOG=debug cargo run --package rpresent -- zoom=on &
    sleep 10
    pkill -SIGUSR1 rpresent