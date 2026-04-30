qml_host_dir := "tools/rpresent-qml-host"
qml_build_dir := qml_host_dir / "build"

help:
    @echo "Available commands:"
    @echo "  run-spotlight       - Start the spotlight application and toggle it on"
    @echo "  build-qml-host      - Configure and build the rpresent-qml-host binary"
    @echo "  install-qml-host    - Build and install rpresent-qml-host to ~/.local/bin"
    @echo "  install-qml-deps    - Install Qt6 QML runtime packages (requires sudo)"

install-qml-deps:
    sudo apt install qt6-base-dev qt6-declarative-dev

build-qml-host:
    cmake -S {{qml_host_dir}} -B {{qml_build_dir}} -DCMAKE_BUILD_TYPE=Release
    cmake --build {{qml_build_dir}} -j`nproc`

install-qml-host: build-qml-host
    install -Dm755 {{qml_build_dir}}/rpresent-qml-host ~/.local/bin/rpresent-qml-host
    @echo "Installed to ~/.local/bin/rpresent-qml-host"

run-daemon:
    RUST_LOG=trace cargo run --package rpresent -- --config ./config.toml &

run-spotlight: run-daemon
    RUST_LOG=trace cargo run --package rpresent -- spot=toggle
