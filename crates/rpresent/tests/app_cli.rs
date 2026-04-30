use std::path::{Path, PathBuf};

use clap::Parser;

use rpresent::app::{AppEntry, AppMode, Cli, ConfigInitCli, parse_entrypoint, resolve_mode};
use rpresent::command::{AnnotationCommand, AppCommand, SpotlightCommand, ZoomCommand};

#[test]
fn parses_config_override_and_spot_command() {
    rpresent::logging::init();

    let cli =
        Cli::try_parse_from(["rpresent", "--config", "/tmp/rpresent.toml", "spot=toggle"]).unwrap();

    assert_eq!(cli.config.as_deref(), Some(Path::new("/tmp/rpresent.toml")));
    assert_eq!(cli.command.as_deref(), Some("spot=toggle"));
    assert_eq!(
        resolve_mode(&cli).unwrap(),
        AppMode::Send(AppCommand::Spotlight(SpotlightCommand::Toggle))
    );
}

#[test]
fn parses_annotation_command_and_resolve_mode() {
    rpresent::logging::init();

    let cli = Cli::try_parse_from(["rpresent", "annotation=tool=line"]).unwrap();

    assert_eq!(
        resolve_mode(&cli).unwrap(),
        AppMode::Send(AppCommand::Annotation(AnnotationCommand::Tool(
            rpresent_config::AnnotationTool::Line,
        )))
    );
}

#[test]
fn parses_zoom_command_and_resolve_mode() {
    rpresent::logging::init();

    let cli = Cli::try_parse_from(["rpresent", "zoom=reset"]).unwrap();

    assert_eq!(
        resolve_mode(&cli).unwrap(),
        AppMode::Send(AppCommand::Zoom(ZoomCommand::Reset))
    );
}

#[test]
fn no_command_enters_run_mode() {
    rpresent::logging::init();

    let cli = Cli::try_parse_from(["rpresent"]).unwrap();

    assert_eq!(resolve_mode(&cli).unwrap(), AppMode::Run);
}

#[test]
fn parses_config_init_entrypoint() {
    rpresent::logging::init();

    let entry = parse_entrypoint(vec![
        "rpresent".into(),
        "config".into(),
        "init".into(),
        "--config".into(),
        "/tmp/rpresent.toml".into(),
    ])
    .unwrap();

    assert_eq!(
        entry,
        AppEntry::InitConfig(ConfigInitCli {
            config: Some(PathBuf::from("/tmp/rpresent.toml")),
        })
    );
}

#[test]
fn zoom_command_sends_state_to_overlay_channel() {
    use crossbeam_channel::unbounded;
    use rpresent_zoom::{ZoomCommand, ZoomController};

    let (tx, rx) = unbounded();
    let mut controller = ZoomController::from_config(rpresent_zoom::ZoomConfig::default());

    controller.apply(ZoomCommand::On);
    tx.send(controller.state()).unwrap();

    let state = rx.recv().unwrap();
    assert!(state.enabled);
}
