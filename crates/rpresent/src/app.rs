use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;

use rpresent_annotation::{
    AnnotationCommand, AnnotationEventSink, AnnotationTool, Preset, Rgb,
    channel as annotation_channel, parse_rgb,
};
use rpresent_config::{ConfigService, default_config_path, write_default_config};
use rpresent_spotlight::{SpotlightCommand, SpotlightRuntimeMessage};
use rpresent_zoom::{ZoomCommand, channel as zoom_channel, run_overlay};

use crate::command::AppCommand;
use crate::coordinator::{Coordinator, CoordinatorOutputs};
use crate::input::AppEvent;
use crate::ipc;
use crate::native_input::channel as native_channel;

#[derive(Debug, Parser, PartialEq, Eq)]
#[command(
    version,
    about = "Presentation shell and command router",
    long_about = "Presentation shell and command router.\n\nRun with no subcommand to start the daemon.",
    after_long_help = "Examples:\n  rpresent\n  rpresent --config /tmp/rpresent.toml spot toggle\n  rpresent annotation tool line\n  rpresent send 'annotation=color=#00FF00'\n  rpresent config init"
)]
pub struct Cli {
    /// Config file to use instead of the default path
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub subcommand: Option<CliSubcommand>,
}

#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum CliSubcommand {
    /// Write the default config file
    Config {
        #[command(subcommand)]
        command: ConfigSubcommand,
    },
    /// Escape hatch for legacy command strings not covered by the structured subcommands
    Send(SendCli),
    /// Send a spotlight command
    Spot(SpotArgs),
    /// Send a zoom command
    Zoom(ZoomArgs),
    /// Send an annotation command
    Annotation {
        #[command(subcommand)]
        command: AnnotationCli,
    },
}

#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum ConfigSubcommand {
    /// Write the default config file without overwriting an existing one
    Init(ConfigInitCli),
}

#[derive(Debug, Args, PartialEq, Eq)]
pub struct ConfigInitCli {
    /// Destination path; defaults to the built-in config path
    #[arg(value_name = "PATH")]
    pub path: Option<PathBuf>,
}

#[derive(Debug, Args, PartialEq, Eq)]
pub struct SendCli {
    /// Command string routed through the legacy parser, for example `spot=toggle`
    #[arg(value_name = "COMMAND")]
    pub command: String,
}

#[derive(Debug, Args, PartialEq, Eq)]
pub struct SpotArgs {
    /// Spotlight action to apply
    #[arg(value_enum, value_name = "ACTION")]
    pub action: SpotAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SpotAction {
    /// Turn the spotlight on
    On,
    /// Turn the spotlight off
    Off,
    /// Flip the spotlight between on and off
    Toggle,
}

impl From<SpotArgs> for SpotlightCommand {
    fn from(args: SpotArgs) -> Self {
        match args.action {
            SpotAction::On => Self::On,
            SpotAction::Off => Self::Off,
            SpotAction::Toggle => Self::Toggle,
        }
    }
}

#[derive(Debug, Args, PartialEq, Eq)]
pub struct ZoomArgs {
    /// Zoom action to apply
    #[arg(value_enum, value_name = "ACTION")]
    pub action: ZoomAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ZoomAction {
    /// Turn zoom on
    On,
    /// Turn zoom off
    Off,
    /// Flip zoom between on and off
    Toggle,
    /// Return the view to its unzoomed state
    Reset,
}

impl From<ZoomArgs> for ZoomCommand {
    fn from(args: ZoomArgs) -> Self {
        match args.action {
            ZoomAction::On => Self::On,
            ZoomAction::Off => Self::Off,
            ZoomAction::Toggle => Self::Toggle,
            ZoomAction::Reset => Self::Reset,
        }
    }
}

#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum AnnotationCli {
    /// Turn the annotation overlay on
    On,
    /// Turn the annotation overlay off
    Off,
    /// Toggle the annotation overlay
    Toggle,
    /// Clear every annotation
    Clear,
    /// Undo the last annotation change
    Undo,
    /// Redo the last undone annotation change
    Redo,
    /// Delete the current selection
    Delete,
    /// Select the drawing tool
    Tool(ToolArgs),
    /// Set the drawing color
    Color(ColorArgs),
    /// Set the stroke width
    Width(SizeArgs),
    /// Set the text size
    TextSize(SizeArgs),
}

#[derive(Debug, Args, PartialEq, Eq)]
pub struct ToolArgs {
    /// Tool name: select, pen, line, arrow, rect, ellipse, eraser, or text
    #[arg(value_name = "TOOL", value_parser = parse_tool)]
    pub tool: AnnotationTool,
}

#[derive(Debug, Args, PartialEq, Eq)]
pub struct ColorArgs {
    /// Color as #RRGGBB, or next / prev to cycle the palette
    #[arg(value_name = "COLOR", value_parser = parse_color)]
    pub color: Preset<Rgb>,
}

#[derive(Debug, Args, PartialEq, Eq)]
pub struct SizeArgs {
    /// Positive integer, or next / prev to cycle the presets
    #[arg(value_name = "VALUE", value_parser = parse_size)]
    pub value: Preset<u32>,
}

impl From<AnnotationCli> for AnnotationCommand {
    fn from(cli: AnnotationCli) -> Self {
        match cli {
            AnnotationCli::On => Self::On,
            AnnotationCli::Off => Self::Off,
            AnnotationCli::Toggle => Self::Toggle,
            AnnotationCli::Clear => Self::Clear,
            AnnotationCli::Undo => Self::Undo,
            AnnotationCli::Redo => Self::Redo,
            AnnotationCli::Delete => Self::Delete,
            AnnotationCli::Tool(args) => Self::Tool(args.tool),
            AnnotationCli::Color(args) => Self::Color(args.color),
            AnnotationCli::Width(args) => Self::Width(args.value),
            AnnotationCli::TextSize(args) => Self::TextSize(args.value),
        }
    }
}

fn parse_tool(value: &str) -> Result<AnnotationTool, String> {
    match value {
        "select" => Ok(AnnotationTool::Select),
        "pen" => Ok(AnnotationTool::Pen),
        "line" => Ok(AnnotationTool::Line),
        "arrow" => Ok(AnnotationTool::Arrow),
        "rect" => Ok(AnnotationTool::Rect),
        "ellipse" => Ok(AnnotationTool::Ellipse),
        "eraser" => Ok(AnnotationTool::Eraser),
        "text" => Ok(AnnotationTool::Text),
        other => Err(format!(
            "invalid tool {other}: expected select, pen, line, arrow, rect, ellipse, eraser, or text"
        )),
    }
}

fn parse_color(value: &str) -> Result<Preset<Rgb>, String> {
    match value {
        "next" => Ok(Preset::Next),
        "prev" => Ok(Preset::Prev),
        other => parse_rgb(other)
            .map(Preset::Value)
            .map_err(|error| error.to_string()),
    }
}

fn parse_size(value: &str) -> Result<Preset<u32>, String> {
    match value {
        "next" => Ok(Preset::Next),
        "prev" => Ok(Preset::Prev),
        other => other
            .parse::<u32>()
            .map(Preset::Value)
            .map_err(|_| "expected a positive integer, next, or prev".to_owned()),
    }
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

pub fn run_from_env() -> Result<(), AppError> {
    let mut cli = match Cli::try_parse_from(std::env::args_os()) {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            error.exit();
        }
        Err(error) => return Err(error.into()),
    };

    tracing::debug!(subcommand = ?cli.subcommand, "parsed entrypoint");

    let Some(subcommand) = cli.subcommand.take() else {
        return run(cli);
    };

    match subcommand {
        CliSubcommand::Config {
            command: ConfigSubcommand::Init(config_init),
        } => {
            let written_path = run_config_init(config_init)?;
            println!("{}", written_path.display());
            Ok(())
        }
        CliSubcommand::Send(send) => {
            let command = send.command.parse().map_err(AppError::Command)?;
            send_command(cli.config, command)
        }
        CliSubcommand::Spot(args) => send_command(cli.config, AppCommand::Spotlight(args.into())),
        CliSubcommand::Zoom(args) => send_command(cli.config, AppCommand::Zoom(args.into())),
        CliSubcommand::Annotation { command } => {
            send_command(cli.config, AppCommand::Annotation(command.into()))
        }
    }
}

#[tracing::instrument(level = "debug", skip(cli))]
pub fn run(cli: Cli) -> Result<(), AppError> {
    tracing::debug!(config_override = cli.config.is_some(), "starting app run");

    let config_service = load_config_service(cli.config)?;
    tracing::debug!("entering daemon mode");
    run_daemon(config_service)
}

fn load_config_service(config: Option<PathBuf>) -> Result<Arc<ConfigService>, AppError> {
    let config_path = match config {
        Some(path) => path,
        None => default_config_path()?,
    };
    tracing::debug!(config_path = %config_path.display(), "loading configuration");

    Ok(Arc::new(ConfigService::load_or_default(config_path)?))
}

#[tracing::instrument(level = "debug", skip(command))]
fn send_command(config: Option<PathBuf>, command: AppCommand) -> Result<(), AppError> {
    let config_service = load_config_service(config)?;
    let socket_path = ipc::socket_path_for(&config_service.app().socket_name)?;
    tracing::debug!(socket_path = %socket_path.display(), command = %command, "sending command to daemon");

    ipc::send_command(&socket_path, command)?;
    Ok(())
}

#[tracing::instrument(level = "debug", skip(cli))]
pub fn run_config_init(cli: ConfigInitCli) -> Result<PathBuf, AppError> {
    tracing::debug!(path_override = cli.path.is_some(), "writing default config");

    let config_path = match cli.path {
        Some(path) => path,
        None => default_config_path()?,
    };
    tracing::debug!(config_path = %config_path.display(), "resolved config init path");

    Ok(write_default_config(config_path)?)
}

#[tracing::instrument(level = "debug", skip(config_service))]
fn run_daemon(config_service: Arc<ConfigService>) -> Result<(), AppError> {
    tracing::debug!(
        socket_name = %config_service.app().socket_name,
        "starting daemon runtime"
    );

    let (ipc_tx, ipc_rx) = mpsc::channel::<AppCommand>();
    let (event_tx, event_rx) = mpsc::channel::<AppEvent>();
    let (spotlight_tx, spotlight_rx) = mpsc::channel::<SpotlightRuntimeMessage>();
    config_service.zoom().validate()?;
    config_service.annotation().validate()?;
    let (annotation_handle_channel, annotation_inbox) = annotation_channel().map_err(|error| {
        AppError::Annotation(rpresent_annotation::OverlayError::Runtime(format!(
            "failed to create annotation transport: {error}"
        )))
    })?;
    let (zoom_handle, zoom_inbox) = zoom_channel().map_err(|error| {
        AppError::Zoom(rpresent_zoom::OverlayError::Runtime(format!(
            "failed to create zoom transport: {error}"
        )))
    })?;
    let socket_path = ipc::socket_path_for(&config_service.app().socket_name)?;

    #[cfg(not(target_os = "linux"))]
    return Err(AppError::Zoom(
        rpresent_zoom::OverlayError::UnsupportedPlatform,
    ));
    #[cfg(target_os = "linux")]
    let (native_handle, native_inbox) = native_channel().map_err(|error| {
        AppError::Zoom(rpresent_zoom::OverlayError::Runtime(format!(
            "failed to create native input transport: {error}"
        )))
    })?;

    // Forward IPC commands into the unified event channel.
    let event_tx_for_ipc = event_tx.clone();
    std::thread::spawn(move || {
        for cmd in ipc_rx {
            let _ = event_tx_for_ipc.send(AppEvent::Command(cmd));
        }
    });
    let event_tx_for_ipc_error = event_tx.clone();
    std::thread::spawn(move || {
        if let Err(error) = ipc::serve(&socket_path, ipc_tx) {
            tracing::error!(error = %error, "ipc server failed");
            let _ = event_tx_for_ipc_error.send(AppEvent::IpcFailed(error.to_string()));
        }
    });

    let zoom_events: rpresent_zoom::ZoomEventSink = {
        let events = event_tx.clone();
        Arc::new(move |event| {
            let _ = events.send(AppEvent::ZoomRuntime(event));
        })
    };
    let spotlight_events: rpresent_spotlight::SpotlightEventSink = {
        let events = event_tx.clone();
        Arc::new(move |event| {
            let _ = events.send(AppEvent::SpotlightRuntime(event));
        })
    };

    #[cfg(target_os = "linux")]
    {
        let events: crate::native_input::NativeEventSink = {
            let event_tx = event_tx.clone();
            Arc::new(move |event| {
                let _ = event_tx.send(AppEvent::Native(event));
            })
        };
        crate::x11_input::spawn(Arc::clone(&config_service), native_inbox, events);
    }
    let spotlight_service = Arc::clone(&config_service);
    let spotlight_handle = std::thread::spawn(move || {
        rpresent_spotlight::run_overlay(spotlight_service, spotlight_rx, spotlight_events)
    });
    let annotation_events: AnnotationEventSink = {
        let events = event_tx.clone();
        Arc::new(move |event| {
            let _ = events.send(AppEvent::AnnotationRuntime(event));
        })
    };
    let annotation_service = Arc::clone(&config_service);
    let annotation_handle = std::thread::spawn(move || {
        rpresent_annotation::run_annotation_overlay(
            annotation_service,
            annotation_inbox,
            annotation_events,
        )
    });
    let zoom_service = Arc::clone(&config_service);
    let zoom_runtime =
        std::thread::spawn(move || run_overlay(zoom_service, zoom_inbox, zoom_events));

    let outputs = CoordinatorOutputs {
        spotlight: spotlight_tx,
        zoom: zoom_handle,
        native: native_handle,
        annotation: annotation_handle_channel,
    };
    let coordinator_result = Coordinator::run(Arc::clone(&config_service), event_rx, outputs);

    let _ = spotlight_handle.join();
    let _ = annotation_handle.join();
    let _ = zoom_runtime.join();

    coordinator_result.map_err(|error| AppError::Zoom(rpresent_zoom::OverlayError::Runtime(error)))
}
