help:
    @echo "Available commands:"
    @echo "  run-spotlight       - Start the spotlight application and toggle it on"
    @echo "  build-qml-host      - Build the rpresent-qml-host binary"
    @echo "  install-qml-host    - Build and install rpresent-qml-host"

build-qml-host:
    cargo build -p rpresent-qml-host

install-qml-host:
    cargo install --path crates/rpresent-qml-host

run-daemon:
    RUST_LOG=debug cargo run --package rpresent -- --config ./config.toml &

run-spotlight: run-daemon
    RUST_LOG=debug cargo run --package rpresent -- spot=toggle &