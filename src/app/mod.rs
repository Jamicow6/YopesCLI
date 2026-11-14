use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    path::PathBuf,
    time::{Instant, SystemTime},
};

use anyhow::Result;
use base64::Engine as _;
use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

use crate::{
    backend::{Backend, StreamEvent},
    catalog,
    config::{self, AuthKind, Connection, ProviderKind, SavedModel},
    diff::FileDiff,
    logos::Logos,
    session::{self, ImageAttachment, Message, Role, Session, TaskState, ToolCall},
    skills, tools,
};

mod action;
mod commands;
mod connect;
mod format;
mod images;
mod input;
mod keys;
mod mode;
mod palette;
mod pickers;
mod stream;
mod turn;

pub(crate) use action::{Action, ChangeReport, Notice, NoticeKind, RetryNotice, ToolEvent};
pub(crate) use commands::{COMMANDS, CommandSpec, EFFORT_CHOICES, matching_commands};
pub(crate) use input::Input;
pub(crate) use pickers::{ListPicker, ModelChoice, ModelRow};

use format::estimated_output_tokens;

pub(crate) use connect::state::{
    AddProviderField, ConnectStage, ConnectState, ListRow, list_row_of,
};
pub use format::{format_bytes, format_cost, format_tokens};
pub use mode::{Mode, ToolsMode};

/// Fraction of the model context window at which compaction triggers.
const COMPACTION_THRESHOLD_FRAC: u64 = 3; // fire at ~1/3 of window
/// Fraction of the model context window kept intact after compaction.
const COMPACTION_KEEP_FRAC: u64 = 8; // keep ~1/8 of window
/// Default context window (128K) when no specific model context size is known.
pub(crate) const DEFAULT_CONTEXT_WINDOW: u64 = 128_000;

struct ActiveRequest {
    id: u64,
    started: Instant,
    prompt_tokens: u64,
    usage: Option<(u64, u64)>,
    tool_progress: Option<(String, String, u64)>,
    /// Set while this request summarizes older turns instead of replying.
    compacting: Option<Compaction>,
}

/// A compaction in flight: which history prefix it summarizes and the text
/// streamed back so far.
struct Compaction {
    split: usize,
    text: String,
}

pub struct ToolApproval {
    pub call: ToolCall,
    pub note: Option<String>,
    /// Change previews of a write or edit, drawn in the dialog itself.
    pub diffs: Vec<FileDiff>,
    pub scroll: usize,
    pub page_height: u16,
    pub popup_area: std::cell::Cell<ratatui::layout::Rect>,
}

pub struct RunningTool {
    pub call: ToolCall,
    pub started: Instant,
    /// File contents from before the call, kept for the outcome diff.
    pub before: Option<String>,
    /// Whether the write or edit target existed before the call ran.
    pub existed: bool,
    /// Every file the call may change and whether each existed before.
    pub targets: Vec<(String, bool)>,
}

/// Session snapshots written off the UI thread.
///
/// Encoding and writing a large history can take seconds, which used to block
/// the event loop on every send and looked like a freeze. Snapshots are queued
/// here instead; only the newest queued one is written, because everything
/// older is already stale.
struct SessionSaver {
    jobs: std::sync::mpsc::Sender<SaveJob>,
    errors: std::sync::mpsc::Receiver<String>,
}

struct SaveJob {
    path: PathBuf,
    session: Session,
    done: Option<std::sync::mpsc::Sender<()>>,
}

impl SessionSaver {
    fn new() -> Self {
        let (jobs, queue) = std::sync::mpsc::channel::<SaveJob>();
        let (error_tx, errors) = std::sync::mpsc::channel::<String>();
        std::thread::spawn(move || {
            while let Ok(mut job) = queue.recv() {
                let mut superseded = Vec::new();
                while let Ok(newer) = queue.try_recv() {
                    if let Some(done) = job.done.take() {
                        superseded.push(done);
                    }
                    job = newer;
                }
                if let Err(error) = job.session.save(&job.path) {
                    let _ = error_tx.send(format!("{error:#}"));
                }
                for done in superseded {
                    let _ = done.send(());
                }
                if let Some(done) = job.done {
                    let _ = done.send(());
                }
            }
        });
        Self { jobs, errors }
    }

    fn save(&self, path: PathBuf, session: Session) {
        let _ = self.jobs.send(SaveJob {
            path,
            session,
            done: None,
        });
    }

    /// Save and wait for the write, for the final snapshot on exit.
    fn save_and_wait(&self, path: PathBuf, session: Session) -> bool {
        let (done_tx, done) = std::sync::mpsc::channel();
        let job = SaveJob {
            path,
            session,
            done: Some(done_tx),
        };
        if self.jobs.send(job).is_err() {
            return false;
        }
        done.recv_timeout(std::time::Duration::from_secs(10))
            .is_ok()
    }

    /// The last write failure since the previous call, if any.
    fn take_error(&self) -> Option<String> {
        let mut last = None;
        while let Ok(error) = self.errors.try_recv() {
            last = Some(error);
        }
        last
    }
}

pub struct App {
    pub session: Session,
    pub backend: Backend,
    /// Session snapshots are written here, never on the UI thread.
    session_saver: SessionSaver,
    pub input: Input,
    pub scroll: usize,
    pub input_scroll: usize,
    pub transcript_height: u16,
    pub follow_tail: bool,
    pub show_help: bool,
    /// How far the help popup is scrolled down.
    pub help_scroll: usize,
    pub should_quit: bool,
    pub notice: Option<Notice>,
    /// A request being retried, shown in the footer while it waits.
    pub retry: Option<RetryNotice>,
    /// Whether the terminal has focus. A background terminal draws nothing.
    pub focused: bool,
    pub spinner: usize,
    /// Submitted turn body: the composer prompt flashes for a few ticks.
    pub send_pulse: Option<usize>,
    pub tools_mode: ToolsMode,
    /// What the current turn is for: applying changes, planning, or chat.
    pub mode: Mode,
    /// Whether history is summarized automatically when the context fills.
    pub auto_compact: bool,
    /// Images pasted with Ctrl+V, sent with the next submitted message.
    pub attachments: Vec<ImageAttachment>,
    /// Counter giving each pasted image a unique display name.
    next_image: usize,
    /// Pre-rendered half-block rows of pasted images, keyed `name:width`.
    image_rows: HashMap<String, Vec<Line<'static>>>,
    /// Terminal width the cached image rows were rendered for.
    image_width: u16,
    pub approval: Option<ToolApproval>,
    /// Popup picker opened by `/models` with no argument.
    pub model_picker: Option<ListPicker>,
    /// The models offered by every connected provider, rebuilt when the
    /// picker opens.
    pub model_choices: Vec<ModelChoice>,
    /// Whether models are actively being fetched in the background.
    pub fetching_models: bool,
    /// Popup picker opened by `/sessions`.
    pub session_picker: Option<ListPicker>,
    pub running_tool: Option<RunningTool>,
    /// Diffs for write and edit calls, shown under their results.
    pub changes: HashMap<String, ChangeReport>,
    /// Tool results the user expanded to see the full output.
    pub expanded: HashSet<String>,
    /// Screen row of each collapsible tool output, for click detection.
    pub output_rows: Vec<(u16, String)>,
    /// Whether the Progress checklist is folded to its title row.
    pub progress_collapsed: bool,
    /// Screen row of the Progress title, for click-to-fold.
    pub progress_row: Option<u16>,
    /// Composer target area for mouse click and drag selection.
    pub composer_area: Option<ratatui::layout::Rect>,
    /// Unified or side-by-side diff view, kept across approvals and results.
    pub diff_sides: bool,
    active: Option<ActiveRequest>,
    next_request_id: u64,
    pending_tools: VecDeque<ToolCall>,
    approved_tools: HashSet<String>,
    session_path: Option<PathBuf>,
    /// Directory holding every saved session file, when saving.
    sessions_dir: Option<PathBuf>,
    config_path: PathBuf,
    custom_providers: Vec<config::CustomProvider>,
    /// The provider logos, drawn from the models.dev artwork.
    pub logos: Logos,
    /// The logos that still have to be fetched.
    pending_logos: Vec<String>,
    /// Cache of rendered markdown lines for completed messages: (width, msg_len, lines).
    pub(crate) markdown_cache: HashMap<usize, (u16, usize, Vec<ratatui::text::Line<'static>>)>,
    /// Tavily key for the web_search tool.
    tavily_api_key: Option<String>,
    palette_index: usize,
    palette_hidden: bool,
    connect: Option<ConnectState>,
    /// Prompt tokens billed across this session.
    spent_prompt_tokens: u64,
    /// Completion tokens billed across this session.
    spent_completion_tokens: u64,
    pub(crate) prompt_started: Option<Instant>,
}

impl App {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        session: Session,
        backend: Backend,
        session_path: Option<PathBuf>,
        sessions_dir: Option<PathBuf>,
        config_path: PathBuf,
        custom_providers: Vec<config::CustomProvider>,
        tavily_api_key: Option<String>,
        auto_compact: bool,
    ) -> Self {
        Self {
            session,
            backend,
            session_saver: SessionSaver::new(),
            input: Input::default(),
            scroll: 0,
            input_scroll: 0,
            transcript_height: 10,
            follow_tail: true,
            show_help: false,
            help_scroll: 0,
            should_quit: false,
            notice: None,
            retry: None,
            focused: true,
            spinner: 0,
            send_pulse: None,
            tools_mode: ToolsMode::default(),
            mode: Mode::default(),
            auto_compact,
            attachments: Vec::new(),
            next_image: 0,
            image_rows: HashMap::new(),
            image_width: 0,
            approval: None,
            model_picker: None,
            model_choices: Vec::new(),
            fetching_models: false,
            session_picker: None,
            running_tool: None,
            changes: HashMap::new(),
            expanded: HashSet::new(),
            output_rows: Vec::new(),
            progress_collapsed: false,
            progress_row: None,
            composer_area: None,
            diff_sides: true,
            active: None,
            next_request_id: 0,
            pending_tools: VecDeque::new(),
            approved_tools: HashSet::new(),
            session_path,
            sessions_dir,
            config_path: config_path.clone(),
            custom_providers,
            logos: Logos::new(crate::logos::cache_dir(&config_path)),
            pending_logos: Vec::new(),
            markdown_cache: HashMap::new(),
            tavily_api_key,
            palette_index: 0,
            palette_hidden: false,
            connect: None,
            spent_prompt_tokens: 0,
            spent_completion_tokens: 0,
            prompt_started: None,
        }
    }

    pub fn connect_state(&self) -> Option<&ConnectState> {
        self.connect.as_ref()
    }

    pub fn tavily_key(&self) -> Option<&str> {
        self.tavily_api_key.as_deref()
    }

    pub fn config_path(&self) -> &std::path::Path {
        &self.config_path
    }

    pub fn connect_open(&self) -> bool {
        self.connect.is_some()
    }

    pub fn is_streaming(&self) -> bool {
        self.active.is_some()
    }

    pub fn tools_busy(&self) -> bool {
        self.approval.is_some() || self.running_tool.is_some() || !self.pending_tools.is_empty()
    }

    pub fn is_busy(&self) -> bool {
        self.is_streaming() || self.tools_busy()
    }

    /// Path of the session currently open, for the `/sessions` popup marker.
    pub fn active_session_path(&self) -> Option<&PathBuf> {
        self.session_path.as_ref()
    }

    pub fn running_tool_elapsed(&self) -> Option<f64> {
        self.running_tool
            .as_ref()
            .map(|running| running.started.elapsed().as_secs_f64())
    }

    pub fn active_request_id(&self) -> Option<u64> {
        self.active.as_ref().map(|active| active.id)
    }

    pub fn elapsed_seconds(&self) -> Option<f64> {
        self.active
            .as_ref()
            .map(|active| active.started.elapsed().as_secs_f64())
    }

    /// True while the active request is summarizing older turns.
    pub fn compaction_active(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| active.compacting.is_some())
    }

    /// The summary streamed so far, shown live in the transcript.
    pub fn compaction_text(&self) -> Option<&str> {
        self.active
            .as_ref()?
            .compacting
            .as_ref()
            .map(|compaction| compaction.text.as_str())
    }

    pub fn stream_tool_progress(&self) -> Option<(String, String, u64)> {
        self.active
            .as_ref()
            .and_then(|active| active.tool_progress.clone())
    }

    /// (in, out) tokens for the active request: server-reported usage, else chars/4 estimates.
    pub fn stream_tokens(&self) -> Option<(u64, u64)> {
        let active = self.active.as_ref()?;
        match active.usage {
            Some((prompt, completion)) => Some((prompt.max(active.prompt_tokens), completion)),
            None => Some((active.prompt_tokens, estimated_output_tokens(&self.session))),
        }
    }

    /// Session spend so far, priced from the active model. A provider catalog
    /// price wins over the built-in list; an unknown model shows `—`.
    pub fn spent_cost(&self) -> String {
        let price = self
            .backend
            .model_price()
            .or_else(|| format::known_model_price(self.backend.model()));
        format_cost(
            self.spent_prompt_tokens,
            self.spent_completion_tokens,
            price,
        )
    }

    /// Estimated tokens the next request would send, the context window load.
    pub fn context_tokens(&self) -> u64 {
        Session::estimate_tokens(&self.session.messages)
    }

    /// Context-window size of the active model from its saved metadata, if known.
    pub fn model_context_window(&self) -> Option<u64> {
        let model_id = self.backend.model();
        // The backend's active model list carries the limit saved at connect
        // time, so this survives self.connect being cleared, and it is scoped
        // to the active provider: model ids are not globally unique.
        self.backend
            .saved_models()
            .iter()
            .find(|saved| {
                saved.id.eq_ignore_ascii_case(model_id)
                    || (!saved.name.is_empty() && saved.name.eq_ignore_ascii_case(model_id))
            })
            .and_then(|saved| saved.context_window)
    }

    pub fn tick(&mut self) {
        self.spinner = self.spinner.wrapping_add(1);
    }

    /// Record whether the terminal has focus.
    ///
    /// A background terminal shows nothing, so the ticker stops and the
    /// spinner freezes instead of burning power on frames nobody sees.
    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        if !focused {
            self.retry = None;
        }
    }
}
