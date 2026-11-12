//! The state machine's vocabulary: notices shown to the user, the actions
//! `main` dispatches, and the reports a turn streams back.

use crate::config::AuthKind;
use crate::diff::FileDiff;
use crate::session::{Message, ToolCall};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NoticeKind {
    Info,
    Error,
}

pub struct Notice {
    pub kind: NoticeKind,
    pub message: String,
}

pub enum Action {
    None,
    Start {
        request_id: u64,
        messages: Vec<Message>,
        tools_enabled: bool,
        /// Mode instruction appended to the system prompt for this request.
        mode_note: Option<&'static str>,
    },
    Cancel(u64),
    RunTool {
        call: ToolCall,
    },
    /// Read the clipboard on Ctrl+V and attach an image when it holds one.
    PasteImage,
    /// Turn mouse capture off so the terminal's own text selection works,
    /// or back on to keep the app interactive.
    ToggleMouseCapture,
    Quit,
    LoadCatalog,
    /// Fetch models.dev again, bypassing the cached copy.
    RefreshCatalog,
    /// Fetch the Fasfas `/v1/models` list with the key entered in /connect.
    FetchFasfas(String),
    /// Fetch a custom provider's model list from its models endpoint.
    FetchCustomModels {
        url: String,
        token: Option<String>,
        auth: AuthKind,
    },
    /// Fetch a provider's logo and draw it.
    FetchLogo(String),
    /// Ask the provider for a short title after the first reply of a session.
    NameSession {
        task: String,
    },
}

pub struct ToolEvent {
    pub call: ToolCall,
    pub result: std::result::Result<String, String>,
}

/// A request that failed and is being tried again.
#[derive(Clone, Debug)]
pub struct RetryNotice {
    pub attempt: u32,
    pub delay_secs: u64,
    pub reason: String,
}

/// A diff shown under a tool result in the conversation.
pub struct ChangeReport {
    pub diffs: Vec<FileDiff>,
    pub side_by_side: bool,
}
