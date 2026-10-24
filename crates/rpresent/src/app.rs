use clap::Parser;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;

use rpresent_annotation::AnnotationState;
use rpresent_config::{ConfigService, default_config_path, write_default_config};
use rpresent_spotlight::SpotlightRuntimeMessage;
use rpresent_zoom::{channel as zoom_channel, run_overlay};

use crate::command::AppCommand;
use crate::coordinator::{Coordinator, CoordinatorOutputs};
use crate::input::AppEvent;
use crate::ipc;
use crate::native_input::channel as native_channel;

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

    let config_service = Arc::new(ConfigService::load_or_default(config_path)?);

    match mode {
        AppMode::Send(command) => {
            let socket_path = ipc::socket_path_for(&config_service.app().socket_name)?;
            tracing::debug!(socket_path = %socket_path.display(), "sending command to daemon");
            ipc::send_command(&socket_path, command)?;
            Ok(())
        }
        AppMode::Run => {
            tracing::debug!("entering daemon mode");
            run_daemon(config_service)
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

#[tracing::instrument(level = "debug", skip(config_service))]
fn run_daemon(config_service: Arc<ConfigService>) -> Result<(), AppError> {
    tracing::debug!(
        socket_name = %config_service.app().socket_name,
        "starting daemon runtime"
    );

    let (ipc_tx, ipc_rx) = mpsc::channel::<AppCommand>();
    let (event_tx, event_rx) = mpsc::channel::<AppEvent>();
    let (spotlight_tx, spotlight_rx) = mpsc::channel::<SpotlightRuntimeMessage>();
    let (annotation_tx, annotation_rx) = mpsc::channel::<AnnotationState>();
    config_service.zoom().validate()?;
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
    let annotation_service = Arc::clone(&config_service);
    let annotation_handle = std::thread::spawn(move || {
        rpresent_annotation::run_overlay(annotation_service, annotation_rx)
    });
    let zoom_service = Arc::clone(&config_service);
    let zoom_runtime =
        std::thread::spawn(move || run_overlay(zoom_service, zoom_inbox, zoom_events));

    let outputs = CoordinatorOutputs {
        spotlight: spotlight_tx,
        zoom: zoom_handle,
        native: native_handle,
        annotation: annotation_tx,
    };
    let coordinator_result = Coordinator::run(Arc::clone(&config_service), event_rx, outputs);

    let _ = spotlight_handle.join();
    let _ = annotation_handle.join();
    let _ = zoom_runtime.join();

    coordinator_result.map_err(|error| AppError::Zoom(rpresent_zoom::OverlayError::Runtime(error)))
}
