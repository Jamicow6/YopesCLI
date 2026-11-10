mod app;
mod backend;
mod catalog;
mod config;
mod credentials;
mod session;
mod ui;
mod update;

use std::io::{self, Stdout};

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::{
    cursor::Show,
    event::{DisableBracketedPaste, EnableBracketedPaste, Event, EventStream},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures_util::StreamExt;
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::{sync::mpsc, task::JoinHandle, time::Duration};

use crate::{
    app::{Action, App},
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
    let mut app = App::new(session, backend, session_path, config_path);
    let mut terminal = TerminalGuard::new()?;
    let (sender, mut receiver) = mpsc::unbounded_channel::<StreamEvent>();
    let (catalog_sender, mut catalog_receiver) = mpsc::unbounded_channel::<FetchResult>();
    let mut terminal_events = EventStream::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(100));
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
                        match app.handle_key(key) {
                            Action::None => {}
                            Action::Start { request_id, messages } => {
                                let backend = app.backend.clone();
                                let sender = sender.clone();
                                let task = tokio::spawn(async move {
                                    backend.stream(request_id, messages, sender).await;
                                });
                                active_task = Some((request_id, task));
                            }
                            Action::Cancel(request_id) => {
                                cancel_task(&mut active_task, request_id);
                            }
                            Action::Quit => {}
                            Action::LoadCatalog => {
                                let sender = catalog_sender.clone();
                                tokio::spawn(async move {
                                    let result = catalog::fetch()
                                        .await
                                        .map_err(|error| format!("{error:#}"));
                                    let _ = sender.send(result);
                                });
                            }
                        }
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
                    | StreamEvent::Done { request_id }
                    | StreamEvent::Error { request_id, .. } => *request_id,
                };
                let finished = matches!(event, StreamEvent::Done { .. } | StreamEvent::Error { .. });
                app.on_stream_event(event);
                if finished && active_task.as_ref().is_some_and(|(id, _)| *id == request_id) {
                    active_task = None;
                }
            }
            result = catalog_receiver.recv() => {
                if let Some(result) = result {
                    app.on_catalog(result);
                }
            }
            _ = ticker.tick(), if app.is_streaming() || app.connect_open() => app.tick(),
        }
    }

    if let Some((_, task)) = active_task.take() {
        task.abort();
    }
    drop(terminal);
    app.persist()
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
