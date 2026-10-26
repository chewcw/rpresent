use std::fs;
use std::path::PathBuf;

use clap::Parser;
use tempfile::tempdir;

use rpresent::app::{Cli, CliSubcommand, ConfigSubcommand, run_config_init};
use rpresent_config::load_from_str;

#[test]
fn config_init_writes_default_config_file() {
    rpresent::logging::init();

    let tempdir = tempdir().unwrap();
    let config_path = tempdir.path().join("rpresent.toml");

    let cli =
        Cli::try_parse_from(["rpresent", "config", "init", config_path.to_str().unwrap()]).unwrap();

    let Some(CliSubcommand::Config {
        command: ConfigSubcommand::Init(config_init),
    }) = cli.subcommand
    else {
        panic!("expected config init subcommand");
    };

    let written_path = run_config_init(config_init).unwrap();
    assert_eq!(written_path, PathBuf::from(&config_path));

    let contents = fs::read_to_string(&written_path).unwrap();
    assert!(contents.contains("[app]"));
    assert!(contents.contains("[zoom]"));
    assert!(contents.contains("socket_name = \"rpresent.sock\""));

    let config = load_from_str(&contents).unwrap();
    let default = rpresent_config::Config::default();
    assert_eq!(config.spotlight.radius_px, default.spotlight.radius_px);
    assert!(
        (config.spotlight.shade_opacity - default.spotlight.shade_opacity).abs() < f32::EPSILON
    );
    assert!(!contents.contains("dim_alpha"));
    assert!(!contents.contains("spot_size_pct"));
}
