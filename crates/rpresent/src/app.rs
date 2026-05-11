use clap::Parser;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::mpsc;

use rpresent_annotation::{AnnotationAppearance, AnnotationController, AnnotationState};
use rpresent_config::{
    Config, default_config_path, load_or_default_from_path, write_default_config,
};
use rpresent_spotlight::{SpotlightController, SpotlightRuntimeMessage};
use rpresent_zoom::{ZoomAppearance, ZoomController, ZoomRuntimeMessage};

use crate::command::AppCommand;
use crate::input::AppEvent;
use crate::ipc;

#[derive(Debug, Parser, Clone, PartialEq, Eq)]
#[command(
    version,
    about = "Presentation shell and command router",
    long_about = None,
    after_long_help = "Examples:\n  rpresent spot=toggle\n  rpresent annotation=tool=line\n  rpresent zoom=reset\n  rpresent config init --config ~/.config/rpresent/config.toml"
)]
pub struct Cli {
    #[arg(long)]
    pub config: Option<PathBuf>,

    #[arg(value_name = "COMMAND")]
    pub command: Option<String>,
}

#[derive(Debug, Parser, Clone, PartialEq, Eq)]
#[command(version, about = "Write the default config file", long_about = None)]
pub struct ConfigInitCli {
    #[arg(long)]
    pub config: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppMode {
    Run,
    Send(AppCommand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEntry {
    Run(Cli),
    InitConfig(ConfigInitCli),
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Clap(#[from] clap::Error),
    #[error(transparent)]
    Config(#[from] rpresent_config::ConfigError),
    #[error(transparent)]
    Command(#[from] crate::command::AppCommandParseError),
    #[error(transparent)]
    Ipc(#[from] crate::ipc::IpcError),
    #[error(transparent)]
    Spotlight(#[from] rpresent_spotlight::OverlayError),
    #[error(transparent)]
    Annotation(#[from] rpresent_annotation::OverlayError),
    #[error(transparent)]
    Zoom(#[from] rpresent_zoom::OverlayError),
}

#[tracing::instrument(level = "debug", skip(args))]
pub fn parse_entrypoint(args: Vec<OsString>) -> Result<AppEntry, AppError> {
    tracing::debug!(arg_count = args.len(), "parsing entrypoint");

    if args.get(1).and_then(|arg| arg.to_str()) == Some("config")
        && args.get(2).and_then(|arg| arg.to_str()) == Some("init")
    {
        tracing::debug!("resolved config init entrypoint");

        let filtered_args = std::iter::once(args[0].clone())
            .chain(args.into_iter().skip(3))
            .collect::<Vec<_>>();
        let cli = ConfigInitCli::try_parse_from(filtered_args)?;
        return Ok(AppEntry::InitConfig(cli));
    }

    tracing::debug!("resolved run entrypoint");

    Ok(AppEntry::Run(Cli::try_parse_from(args)?))
}

#[tracing::instrument(level = "debug", skip(cli))]
pub fn resolve_mode(cli: &Cli) -> Result<AppMode, AppError> {
    tracing::debug!(command = ?cli.command, "resolving app mode");

    match cli.command.as_deref() {
        Some(command) => {
            tracing::debug!(command = %command, "resolved send mode");
            Ok(AppMode::Send(command.parse()?))
        }
        None => {
            tracing::debug!("resolved run mode");
            Ok(AppMode::Run)
        }
    }
}

pub fn run_from_env() -> Result<(), AppError> {
    match parse_entrypoint(std::env::args_os().collect()) {
        Ok(AppEntry::Run(cli)) => run(cli),
        Ok(AppEntry::InitConfig(cli)) => {
            let written_path = run_config_init(cli)?;
            println!("{}", written_path.display());
            Ok(())
        }
        Err(AppError::Clap(error))
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            error.exit();
        }
        Err(error) => Err(error),
    }
}

#[tracing::instrument(level = "debug", skip(cli))]
pub fn run(cli: Cli) -> Result<(), AppError> {
    tracing::debug!(config_override = cli.config.is_some(), "starting app run");

    let mode = resolve_mode(&cli)?;
    tracing::debug!(mode = ?mode, "resolved app mode");

    let config_path = match cli.config {
        Some(path) => path,
        None => default_config_path()?,
    };
    tracing::debug!(config_path = %config_path.display(), "loading configuration");

    let config = load_or_default_from_path(config_path)?;

    match mode {
        AppMode::Send(command) => {
            let socket_path = ipc::socket_path_for(&config.app.socket_name)?;
            tracing::debug!(socket_path = %socket_path.display(), "sending command to daemon");
            ipc::send_command(&socket_path, command)?;
            Ok(())
        }
        AppMode::Run => {
            tracing::debug!("entering daemon mode");
            run_daemon(config)
        }
    }
}

#[tracing::instrument(level = "debug", skip(cli))]
pub fn run_config_init(cli: ConfigInitCli) -> Result<PathBuf, AppError> {
    tracing::debug!(
        config_override = cli.config.is_some(),
        "writing default config"
    );

    let config_path = match cli.config {
        Some(path) => path,
        None => default_config_path()?,
    };
    tracing::debug!(config_path = %config_path.display(), "resolved config init path");

    Ok(write_default_config(config_path)?)
}

#[tracing::instrument(level = "debug", skip(config))]
fn run_daemon(config: Config) -> Result<(), AppError> {
    tracing::debug!(socket_name = %config.app.socket_name, "starting daemon runtime");

    let (ipc_tx, ipc_rx) = mpsc::channel::<AppCommand>();
    let (event_tx, event_rx) = mpsc::channel::<AppEvent>();
    let (spotlight_tx, spotlight_rx) = mpsc::channel::<SpotlightRuntimeMessage>();
    let (annotation_tx, annotation_rx) = mpsc::channel::<AnnotationState>();
    let (zoom_tx, zoom_rx) = crossbeam_channel::unbounded::<ZoomRuntimeMessage>();
    let socket_path = ipc::socket_path_for(&config.app.socket_name)?;

    let spotlight_config = config.spotlight.clone();
    let annotation_config = config.annotation.clone();
    let zoom_config = config.zoom.clone();
    let annotation_appearance = AnnotationAppearance::from(annotation_config.clone());
    let zoom_appearance = ZoomAppearance::from(zoom_config.clone());

    // Forward IPC commands into unified event channel
    let event_tx_for_ipc = event_tx.clone();
    std::thread::spawn(move || {
        for cmd in ipc_rx {
            let _ = event_tx_for_ipc.send(AppEvent::Command(cmd));
        }
    });
    std::thread::spawn(move || {
        if let Err(error) = ipc::serve(&socket_path, ipc_tx) {
            tracing::error!(error = %error, "ipc server failed");
            std::process::exit(1);
        }
    });

    // X11 input capture (Linux only)
    #[cfg(target_os = "linux")]
    crate::x11_input::spawn(config.shortcuts.clone(), event_tx);
    #[cfg(not(target_os = "linux"))]
    let _ = event_tx;

    std::thread::spawn(move || {
        let mut spotlight_controller = SpotlightController::new(spotlight_config.enabled_on_start);
        let mut annotation_controller = AnnotationController::new(
            annotation_config.enabled_on_start,
            annotation_config.default_tool,
            annotation_config.history_depth,
        );
        let mut zoom_controller = ZoomController::from_config(zoom_config);

        let _ = spotlight_tx.send(SpotlightRuntimeMessage::Visibility(
            spotlight_controller.state(),
        ));
        let _ = annotation_tx.send(annotation_controller.state());
        for message in zoom_controller.initial_messages() {
            let _ = zoom_tx.send(message);
        }

        for event in event_rx {
            match event {
                AppEvent::Command(AppCommand::Spotlight(command)) => {
                    if spotlight_controller.apply(command) {
                        let _ = spotlight_tx.send(SpotlightRuntimeMessage::Visibility(
                            spotlight_controller.state(),
                        ));
                    }
                }
                AppEvent::Command(AppCommand::Annotation(command)) => {
                    if annotation_controller.apply(command) {
                        let _ = annotation_tx.send(annotation_controller.state());
                    }
                }
                AppEvent::Command(AppCommand::Zoom(command)) => {
                    if let Some(message) = zoom_controller.apply(command) {
                        let _ = zoom_tx.send(message);
                    }
                }
                AppEvent::Input(crate::input::InputAction::SpotlightWheel { delta }) => {
                    let _ = spotlight_tx.send(SpotlightRuntimeMessage::ResizeBy(delta * 12.0));
                }
                AppEvent::Input(crate::input::InputAction::ZoomWheel { delta, cursor_x, cursor_y }) => {
                    if let Some(message) = zoom_controller.zoom_by_scroll(delta, (cursor_x as f32, cursor_y as f32)) {
                        let _ = zoom_tx.send(message);
                    }
                }
                AppEvent::Input(crate::input::InputAction::ZoomDragStart { cursor_x, cursor_y }) => {
                    if let Some(message) = zoom_controller.drag_start((cursor_x as f32, cursor_y as f32)) {
                        let _ = zoom_tx.send(message);
                    }
                }
                AppEvent::Input(crate::input::InputAction::ZoomDragMove { cursor_x, cursor_y }) => {
                    if let Some(message) = zoom_controller.drag_move((cursor_x as f32, cursor_y as f32)) {
                        let _ = zoom_tx.send(message);
                    }
                }
                AppEvent::Input(crate::input::InputAction::ZoomDragEnd) => {
                    if let Some(message) = zoom_controller.drag_end() {
                        let _ = zoom_tx.send(message);
                    }
                }
            }
        }
    });

    let spotlight_handle =
        std::thread::spawn(move || rpresent_spotlight::run_overlay(spotlight_config, spotlight_rx));
    let annotation_handle = std::thread::spawn(move || {
        rpresent_annotation::run_overlay(annotation_appearance, annotation_rx)
    });
    let zoom_handle =
        std::thread::spawn(move || rpresent_zoom::run_overlay(zoom_appearance, zoom_rx));

    spotlight_handle.join().unwrap()?;
    annotation_handle.join().unwrap()?;
    zoom_handle.join().unwrap()?;
    Ok(())
}
