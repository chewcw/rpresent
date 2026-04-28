use std::fs;
use std::path::PathBuf;

use clap::Parser;
use tempfile::tempdir;

use rpresent::app::{ConfigInitCli, run_config_init};

#[test]
fn config_init_writes_default_config_file() {
    let tempdir = tempdir().unwrap();
    let config_path = tempdir.path().join("rpresent.toml");

    let cli =
        ConfigInitCli::try_parse_from(["rpresent", "--config", config_path.to_str().unwrap()])
            .unwrap();

    let written_path = run_config_init(cli).unwrap();
    assert_eq!(written_path, PathBuf::from(&config_path));

    let contents = fs::read_to_string(&written_path).unwrap();
    assert!(contents.contains("[app]"));
    assert!(contents.contains("[zoom]"));
    assert!(contents.contains("socket_name = \"rpresent.sock\""));
}
