mod app;
mod backend;
mod catalog;
mod config;
mod credentials;
mod diff;
mod session;
mod tools;
mod ui;
mod update;

use std::{
    future::Future,
    io::{self, Stdout},
    pin::Pin,
};

use anyhow::{Context, Result, bail};
use clap::Parser;
use crossterm::{
    cursor::Show,
    event::{DisableBracketedPaste, EnableBracketedPaste, Event, EventStream},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures_util::StreamExt;
use ratatui::{Terminal, backend::CrosstermBackend};
#[cfg(unix)]
use tokio::signal::unix::{SignalKind, signal};
use tokio::{sync::mpsc, task::JoinHandle, time::Duration};

use crate::{
    app::{Action, App, ToolEvent},
    backend::{Backend, StreamEvent},
    catalog::FetchResult,
    config::{Cli, Command, Config, load_session},
};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("yopes: {error:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    if matches!(cli.command, Some(Command::Update)) {
        return update::run().await;
    }
    let config = Config::from_cli(cli)?;
    let session_path = config.session_path.clone();
    let config_path = config.config_path.clone();
    let session = load_session(session_path.as_deref())?;
    let backend = Backend::new(&config)?;
    let custom_providers = config.custom_providers.clone();
    let tavily_api_key = config.tavily_api_key.clone();
    let mut app = App::new(
        session,
        backend,
        session_path,
        config_path,
        custom_providers,
        tavily_api_key,
    );
    let mut terminal = TerminalGuard::new()?;
    let (sender, mut receiver) = mpsc::unbounded_channel::<StreamEvent>();
    let (tool_sender, mut tool_receiver) = mpsc::unbounded_channel::<ToolEvent>();
    let (catalog_sender, mut catalog_receiver) = mpsc::unbounded_channel::<FetchResult>();
    let (paste_sender, mut paste_receiver) =
        mpsc::unbounded_channel::<Result<(String, String), String>>();
    let mut terminal_events = EventStream::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(100));
    // Ctrl+C is a key in raw mode, but a real interrupt must also work when the
    // key event is swallowed, or while a blocking tool is running.
    let interrupt = ctrl_c_interrupt();
    tokio::pin!(interrupt);
    let mut term_signal: Pin<Box<dyn Future<Output = ()> + Send>> = {
        #[cfg(unix)]
        {
            let mut term = signal(SignalKind::terminate())
                .context("installing the termination signal handler")?;
            Box::pin(async move {
                let _ = term.recv().await;
            })
        }
        #[cfg(not(unix))]
        {
            Box::pin(std::future::pending::<()>())
        }
    };
    let mut active_task: Option<(u64, JoinHandle<()>)> = None;

    loop {
        if app.should_quit {
            break;
        }
        terminal
            .terminal
            .draw(|frame| ui::draw(frame, &mut app))
            .context("drawing terminal UI")?;

        tokio::select! {
            event = terminal_events.next() => {
                match event {
                    Some(Ok(Event::Key(key))) => {
                        let action = app.handle_key(key);
                        dispatch(
                            action,
                            &mut app,
                            &sender,
                            &tool_sender,
                            &catalog_sender,
                            &paste_sender,
                            &mut active_task,
                        );
                    }
                    Some(Ok(Event::Paste(value))) => app.handle_paste(&value),
                    Some(Ok(Event::Mouse(mouse))) => app.handle_mouse(mouse),
                    Some(Ok(Event::Resize(_, _))) => {}
                    Some(Ok(_)) => {}
                    Some(Err(error)) => return Err(error).context("reading terminal event"),
                    None => return Err(anyhow::anyhow!("terminal event stream closed")),
                }
            }
            event = receiver.recv() => {
                let Some(event) = event else { break; };
                let request_id = match &event {
                    StreamEvent::Delta { request_id, .. }
                    | StreamEvent::Reasoning { request_id, .. }
                    | StreamEvent::ToolProgress { request_id, .. }
                    | StreamEvent::Usage { request_id, .. }
                    | StreamEvent::ToolCalls { request_id, .. }
                    | StreamEvent::Done { request_id }
                    | StreamEvent::Error { request_id, .. } => *request_id,
                };
                let finished = matches!(event, StreamEvent::Done { .. } | StreamEvent::Error { .. });
                let action = app.on_stream_event(event);
                if finished && active_task.as_ref().is_some_and(|(id, _)| *id == request_id) {
                    active_task = None;
                }
                dispatch(
                    action,
                    &mut app,
                    &sender,
                    &tool_sender,
                    &catalog_sender,
                    &paste_sender,
                    &mut active_task,
                );
            }
            result = tool_receiver.recv() => {
                let Some(result) = result else { break; };
                let action = app.on_tool_result(result);
                dispatch(
                    action,
                    &mut app,
                    &sender,
                    &tool_sender,
                    &catalog_sender,
                    &paste_sender,
                    &mut active_task,
                );
            }
            result = catalog_receiver.recv() => {
                if let Some(result) = result {
                    app.on_catalog(result);
                }
            }
            result = paste_receiver.recv() => {
                let Some(result) = result else { break; };
                app.on_paste_image(result);
            }
            _ = &mut interrupt => break,
            _ = term_signal.as_mut() => break,
            _ = ticker.tick(),
            if app.is_streaming()
                || app.connect_open()
                || app.tools_busy()
                || app.session.messages.is_empty() =>
            {
                app.tick()
            }
        }
    }

    if let Some((_, task)) = active_task.take() {
        task.abort();
    }
    drop(terminal);
    app.persist()
}

fn dispatch(
    action: Action,
    app: &mut App,
    events: &mpsc::UnboundedSender<StreamEvent>,
    tool_events: &mpsc::UnboundedSender<ToolEvent>,
    catalog_events: &mpsc::UnboundedSender<FetchResult>,
    paste_sender: &mpsc::UnboundedSender<Result<(String, String), String>>,
    active_task: &mut Option<(u64, JoinHandle<()>)>,
) {
    match action {
        Action::None | Action::Quit => {}
        Action::Start {
            request_id,
            messages,
            tools_enabled,
            mode_note,
        } => {
            if let Some((_, previous)) = active_task.take() {
                previous.abort();
            }
            let backend = app.backend.clone();
            let sender = events.clone();
            let task = tokio::spawn(async move {
                backend
                    .stream(request_id, messages, tools_enabled, mode_note, sender)
                    .await;
            });
            *active_task = Some((request_id, task));
        }
        Action::Cancel(request_id) => cancel_task(active_task, request_id),
        Action::PasteImage => {
            let sender = paste_sender.clone();
            tokio::spawn(async move {
                // Anyhow's chain needs rendering before crossing the channel.
                let read = match tokio::task::spawn_blocking(read_clipboard_image).await {
                    Ok(Ok(result)) => Ok(result),
                    Ok(Err(error)) => Err(format!("{error:#}")),
                    Err(join) => Err(format!("paste task failed: {join}")),
                };
                let _ = sender.send(read);
            });
        }
        Action::RunTool { call } => {
            let sender = tool_events.clone();
            let web_key = app.tavily_key().map(str::to_owned);
            tokio::spawn(async move {
                let task_call = call.clone();
                let joined = tokio::task::spawn_blocking(move || {
                    tools::execute(&task_call, web_key.as_deref())
                })
                .await;
                let result = match joined {
                    Ok(Ok(output)) => Ok(output),
                    Ok(Err(error)) => Err(format!("{error:#}")),
                    Err(error) => Err(format!("tool task failed: {error}")),
                };
                let _ = sender.send(ToolEvent { call, result });
            });
        }
        Action::LoadCatalog => {
            let sender = catalog_events.clone();
            tokio::spawn(async move {
                let result = catalog::fetch().await.map_err(|error| format!("{error:#}"));
                let _ = sender.send(result);
            });
        }
        Action::FetchFasfas(key) => {
            let sender = catalog_events.clone();
            tokio::spawn(async move {
                let result = catalog::fetch_fasfas(&key)
                    .await
                    .map_err(|error| format!("{error:#}"));
                let _ = sender.send(result);
            });
        }
    }
}

/// Resolves when the user interrupts. Works on every platform, including when
/// the key event itself is swallowed.
fn ctrl_c_interrupt() -> Pin<Box<dyn Future<Output = ()> + Send>> {
    Box::pin(async {
        let _ = tokio::signal::ctrl_c().await;
    })
}

/// Read the clipboard as a PNG via PowerShell, returning its base64 text.
///
/// PowerShell encodes the image itself so binary data never crosses stdout,
/// which Windows PowerShell would otherwise re-encode.
#[cfg(windows)]
fn read_clipboard_image() -> Result<(String, String)> {
    use std::process::Command as ProcessCommand;

    const SCRIPT: &str = "Add-Type -AssemblyName System.Windows.Forms; \
        Add-Type -AssemblyName System.Drawing; \
        $img = [System.Windows.Forms.Clipboard]::GetImage(); \
        if ($img -eq $null) { exit 2 } \
        $ms = New-Object System.IO.MemoryStream; \
        $img.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png); \
        [Convert]::ToBase64String($ms.ToArray())";
    let output = ProcessCommand::new("powershell")
        .args(["-NoProfile", "-STA", "-Command", SCRIPT])
        .output()
        .context("running powershell to read the clipboard")?;
    if output.status.code() == Some(2) {
        bail!("the clipboard does not hold an image");
    }
    let encoded = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() || encoded.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("could not read the clipboard image: {}", stderr.trim());
    }
    Ok(("paste".to_owned(), encoded))
}

#[cfg(not(windows))]
fn read_clipboard_image() -> Result<(String, String)> {
    bail!("image paste is currently only supported on windows")
}

fn cancel_task(active_task: &mut Option<(u64, JoinHandle<()>)>, request_id: u64) {
    if active_task
        .as_ref()
        .is_some_and(|(id, _)| *id == request_id)
    {
        if let Some((_, task)) = active_task.take() {
            task.abort();
        }
    }
}

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    fn new() -> Result<Self> {
        enable_raw_mode().context("enabling terminal raw mode")?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, EnableBracketedPaste, Show) {
            let _ = disable_raw_mode();
            return Err(error).context("initializing terminal");
        }
        let backend = CrosstermBackend::new(stdout);
        match Terminal::new(backend) {
            Ok(terminal) => Ok(Self { terminal }),
            Err(error) => {
                let _ = disable_raw_mode();
                let _ = execute!(
                    io::stdout(),
                    DisableBracketedPaste,
                    LeaveAlternateScreen,
                    Show
                );
                Err(error).context("creating terminal")
            }
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            DisableBracketedPaste,
            LeaveAlternateScreen,
            Show
        );
        let _ = self.terminal.show_cursor();
    }
}
