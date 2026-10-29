use std::path::{Path, PathBuf};

use clap::Parser;

use rpresent::app::{Cli, CliSubcommand, ConfigInitCli, ConfigSubcommand};
use rpresent::command::{
    AnnotationCommand, AnnotationTool, AppCommand, SpotlightCommand, ZoomCommand,
};

#[test]
fn parses_daemon_mode_without_subcommand() {
    rpresent::logging::init();

    let cli = Cli::try_parse_from(["rpresent"]).unwrap();

    assert!(cli.subcommand.is_none());
}

#[test]
fn parses_config_override() {
    rpresent::logging::init();

    let cli = Cli::try_parse_from(["rpresent", "--config", "/tmp/rpresent.toml"]).unwrap();

    assert_eq!(cli.config.as_deref(), Some(Path::new("/tmp/rpresent.toml")));
}

#[test]
fn parses_config_init_with_default_path() {
    rpresent::logging::init();

    let cli = Cli::try_parse_from(["rpresent", "config", "init"]).unwrap();

    let Some(CliSubcommand::Config {
        command: ConfigSubcommand::Init(config_init),
    }) = cli.subcommand
    else {
        panic!("expected config init subcommand");
    };

    assert_eq!(config_init, ConfigInitCli { path: None });
}

#[test]
fn parses_config_init_with_explicit_path() {
    rpresent::logging::init();

    let cli = Cli::try_parse_from(["rpresent", "config", "init", "/tmp/rpresent.toml"]).unwrap();

    let Some(CliSubcommand::Config {
        command: ConfigSubcommand::Init(config_init),
    }) = cli.subcommand
    else {
        panic!("expected config init subcommand");
    };

    assert_eq!(
        config_init,
        ConfigInitCli {
            path: Some(PathBuf::from("/tmp/rpresent.toml")),
        }
    );
}

#[test]
fn parses_spot_subcommand() {
    rpresent::logging::init();

    let cli = Cli::try_parse_from(["rpresent", "spot", "toggle"]).unwrap();

    let Some(CliSubcommand::Spot(args)) = cli.subcommand else {
        panic!("expected spot subcommand");
    };

    assert_eq!(
        AppCommand::Spotlight(args.into()),
        AppCommand::Spotlight(SpotlightCommand::Toggle)
    );
}

#[test]
fn parses_zoom_subcommand() {
    rpresent::logging::init();

    let cli = Cli::try_parse_from(["rpresent", "zoom", "reset"]).unwrap();

    let Some(CliSubcommand::Zoom(args)) = cli.subcommand else {
        panic!("expected zoom subcommand");
    };

    assert_eq!(
        AppCommand::Zoom(args.into()),
        AppCommand::Zoom(ZoomCommand::Reset)
    );
}

#[test]
fn parses_annotation_action_subcommand() {
    rpresent::logging::init();

    let cli = Cli::try_parse_from(["rpresent", "annotation", "undo"]).unwrap();

    let Some(CliSubcommand::Annotation {
        command: annotation,
    }) = cli.subcommand
    else {
        panic!("expected annotation subcommand");
    };

    assert_eq!(
        AppCommand::Annotation(annotation.into()),
        AppCommand::Annotation(AnnotationCommand::Undo)
    );
}

#[test]
fn parses_annotation_tool_subcommand() {
    rpresent::logging::init();

    let cli = Cli::try_parse_from(["rpresent", "annotation", "tool", "line"]).unwrap();

    let Some(CliSubcommand::Annotation {
        command: annotation,
    }) = cli.subcommand
    else {
        panic!("expected annotation subcommand");
    };

    assert_eq!(
        AppCommand::Annotation(annotation.into()),
        AppCommand::Annotation(AnnotationCommand::Tool(AnnotationTool::Line))
    );
}

#[test]
fn parses_ellipse_annotation_tool_subcommand() {
    rpresent::logging::init();

    let cli = Cli::try_parse_from(["rpresent", "annotation", "tool", "ellipse"]).unwrap();

    let Some(CliSubcommand::Annotation {
        command: annotation,
    }) = cli.subcommand
    else {
        panic!("expected annotation subcommand");
    };

    assert_eq!(
        AppCommand::Annotation(annotation.into()),
        AppCommand::Annotation(AnnotationCommand::Tool(AnnotationTool::Ellipse))
    );
}

#[test]
fn parses_annotation_color_subcommand() {
    rpresent::logging::init();

    let cli = Cli::try_parse_from(["rpresent", "annotation", "color", "#00FF00"]).unwrap();

    let Some(CliSubcommand::Annotation {
        command: annotation,
    }) = cli.subcommand
    else {
        panic!("expected annotation subcommand");
    };

    assert_eq!(
        AppCommand::Annotation(annotation.into()),
        AppCommand::Annotation(AnnotationCommand::Color(
            rpresent_annotation::Preset::Value([0x00, 0xFF, 0x00]),
        ))
    );
}

#[test]
fn parses_annotation_size_subcommand() {
    rpresent::logging::init();

    let cli = Cli::try_parse_from(["rpresent", "annotation", "text-size", "next"]).unwrap();

    let Some(CliSubcommand::Annotation {
        command: annotation,
    }) = cli.subcommand
    else {
        panic!("expected annotation subcommand");
    };

    assert_eq!(
        AppCommand::Annotation(annotation.into()),
        AppCommand::Annotation(AnnotationCommand::TextSize(
            rpresent_annotation::Preset::Next,
        ))
    );
}

#[test]
fn parses_send_legacy_command_string() {
    rpresent::logging::init();

    let cli = Cli::try_parse_from(["rpresent", "send", "spot=toggle"]).unwrap();

    let Some(CliSubcommand::Send(send)) = cli.subcommand else {
        panic!("expected send subcommand");
    };

    assert_eq!(send.command, "spot=toggle");
}

#[test]
fn rejects_unknown_annotation_color_value() {
    rpresent::logging::init();

    let result = Cli::try_parse_from(["rpresent", "annotation", "color", "not-a-color"]);

    assert!(result.is_err());
    let error = result.unwrap_err().to_string().to_lowercase();
    assert!(
        error.contains("color"),
        "error should mention the failing value: {error}"
    );
}

#[test]
fn rejects_unknown_annotation_tool_value() {
    rpresent::logging::init();

    let result = Cli::try_parse_from(["rpresent", "annotation", "tool", "not-a-tool"]);

    assert!(result.is_err());
    let error = result.unwrap_err().to_string().to_lowercase();
    assert!(
        error.contains("tool"),
        "error should mention the failing value: {error}"
    );
    assert!(
        error.contains("ellipse"),
        "error should list the valid tools: {error}"
    );
}

#[test]
fn rejects_unknown_spot_action() {
    rpresent::logging::init();

    let result = Cli::try_parse_from(["rpresent", "spot", "dim"]);

    assert!(result.is_err());
    let error = result.unwrap_err().to_string().to_lowercase();
    assert!(
        error.contains("dim"),
        "error should mention the failing value: {error}"
    );
}
