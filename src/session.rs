use std::{
    collections::BTreeSet,
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const SESSION_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionCall,
}

impl ToolCall {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            kind: "function".to_owned(),
            function: FunctionCall {
                name: name.into(),
                arguments: arguments.into(),
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reasoning: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Billing and timing for a finished assistant reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Images the user pasted with this message, stored as base64 PNG data.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageAttachment>,
    /// A provider failure shown in place of the reply. Never sent upstream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One attached screenshot: name for display, PNG bytes base64-encoded so the
/// session file stays plain text.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ImageAttachment {
    pub name: String,
    /// base64 of the PNG file.
    pub data: String,
}

/// What one reply cost and how long it took.
///
/// Stored as whole numbers so `Message` can keep deriving `Eq`; the elapsed
/// time is milliseconds.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub elapsed_ms: u64,
}

impl Usage {
    /// Average output tokens per second, `None` when nothing was timed.
    pub fn tokens_per_second(&self) -> Option<f64> {
        if self.elapsed_ms == 0 || self.completion_tokens == 0 {
            return None;
        }
        Some(self.completion_tokens as f64 / (self.elapsed_ms as f64 / 1000.0))
    }
}

impl Message {
    pub fn new(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            reasoning: String::new(),
            tool_calls: None,
            tool_call_id: None,
            usage: None,
            images: Vec::new(),
            error: None,
        }
    }

    /// A failed request, kept in the transcript where the reply would have
    /// been so the provider message lands under the model label.
    pub fn provider_error(message: impl Into<String>) -> Self {
        let mut msg = Self::new(Role::Assistant, "");
        msg.error = Some(message.into());
        msg
    }

    pub fn tool_result(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            reasoning: String::new(),
            tool_calls: None,
            tool_call_id: Some(call_id.into()),
            usage: None,
            images: Vec::new(),
            error: None,
        }
    }
}

/// One checklist task the model maintains, kept in the session itself so it
/// survives restarts without a workspace file.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TaskState {
    pub text: String,
    #[serde(default)]
    pub done: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    version: u32,
    pub id: u64,
    pub title: String,
    pub messages: Vec<Message>,
    /// Summary of compacted earlier turns, kept so long sessions stay small
    /// without forgetting them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// The todo checklist from todo_write, native instead of `todo.md`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<TaskState>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Self {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis() as u64);
        Self {
            version: SESSION_VERSION,
            id,
            title: "New session".to_owned(),
            messages: Vec::new(),
            summary: None,
            tasks: Vec::new(),
        }
    }

    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::new());
        }
        let source = fs::read_to_string(path)?;
        let mut session: Self = serde_json::from_str(&source)?;
        if session.version != SESSION_VERSION {
            bail!(
                "unsupported session version {}; expected {}",
                session.version,
                SESSION_VERSION
            );
        }
        session.repair_messages();
        Ok(session)
    }

    fn repair_messages(&mut self) {
        while let Some(position) = self.messages.iter().rposition(|message| {
            message.role == Role::Assistant
                && message
                    .tool_calls
                    .as_ref()
                    .is_some_and(|calls| !calls.is_empty())
        }) {
            let expected = self.messages[position]
                .tool_calls
                .as_ref()
                .map(|calls| {
                    calls
                        .iter()
                        .map(|call| call.id.clone())
                        .collect::<BTreeSet<_>>()
                })
                .unwrap_or_default();
            let mut answered = BTreeSet::new();
            for message in &self.messages[position + 1..] {
                if message.role != Role::Tool {
                    break;
                }
                if let Some(id) = &message.tool_call_id {
                    if expected.contains(id) {
                        answered.insert(id.clone());
                    }
                }
            }
            if answered.len() == expected.len() {
                break;
            }
            self.messages.truncate(position);
        }
        while self.messages.last().is_some_and(|message| {
            message.role == Role::Assistant
                && message.content.is_empty()
                && message.tool_calls.is_none()
        }) {
            self.messages.pop();
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(self)?;
        let temporary = path.with_extension("tmp");

        // Write the complete snapshot before replacing the previous session.
        fs::write(&temporary, json).with_context(|| format!("writing {}", temporary.display()))?;
        #[cfg(not(windows))]
        fs::rename(&temporary, path).with_context(|| format!("replacing {}", path.display()))?;
        #[cfg(windows)]
        {
            if path.exists() {
                fs::remove_file(path)
                    .with_context(|| format!("removing old {}", path.display()))?;
            }
            fs::rename(&temporary, path)
                .with_context(|| format!("replacing {}", path.display()))?;
        }
        Ok(())
    }

    pub fn update_title(&mut self, prompt: &str) {
        if self.title != "New session" {
            return;
        }
        let normalized = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut title = normalized.chars().take(42).collect::<String>();
        if normalized.chars().count() > 42 {
            title.push('…');
        }
        if !title.is_empty() {
            self.title = title;
        }
    }

    /// Estimated prompt tokens for `messages`, at four characters per token.
    ///
    /// Matches how the provider request is built: reasoning and tool arguments
    /// are both sent, so both count.
    pub fn estimate_tokens(messages: &[Message]) -> u64 {
        messages
            .iter()
            .map(|message| {
                (message.content.chars().count()
                    + message.reasoning.chars().count()
                    + message.tool_calls.as_ref().map_or(0, |calls| {
                        calls
                            .iter()
                            .map(|call| {
                                call.function.name.chars().count()
                                    + call.function.arguments.chars().count()
                            })
                            .sum::<usize>()
                    })) as u64
                    / 4
            })
            .sum()
    }

    /// Where the transcript can be cut for compaction: the start of the
    /// earliest user turn whose messages all fit in `keep_tokens`.
    ///
    /// Cutting at a user turn keeps every tool call and its result on the same
    /// side of the split, so both halves stay valid request history. `None`
    /// when nothing can be dropped, such as one turn over the whole budget.
    pub fn compaction_split(&self, keep_tokens: u64) -> Option<usize> {
        let mut used = 0u64;
        let mut split = None;
        for (index, message) in self.messages.iter().enumerate().rev() {
            if message.role == Role::User && used <= keep_tokens {
                split = Some(index);
            }
            used += Self::estimate_tokens(std::slice::from_ref(message));
            if used > keep_tokens {
                break;
            }
        }
        split
    }

    /// Replace everything before `split` with one user message holding the
    /// summary, so the model still sees what happened earlier.
    pub fn compact(&mut self, split: usize, summary: String) {
        if split == 0 || split > self.messages.len() || summary.trim().is_empty() {
            return;
        }
        self.messages.drain(..split);
        self.summary = Some(summary.clone());
        self.messages.insert(
            0,
            Message::new(
                Role::User,
                format!("Earlier conversation, summarized:\n\n{summary}"),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{Message, Role, Session, ToolCall};

    #[test]
    fn title_is_normalized_and_bounded() {
        let mut session = Session::new();
        session.update_title("  hello\n   world  ");
        assert_eq!(session.title, "hello world");

        let mut session = Session::new();
        session.update_title(&"é".repeat(60));
        assert_eq!(session.title.chars().count(), 43);
        assert!(session.title.ends_with('…'));
    }

    #[test]
    fn messages_use_openai_compatible_role_names() {
        let message = Message::new(Role::User, "hello");
        assert_eq!(
            serde_json::to_string(&message).unwrap(),
            r#"{"role":"user","content":"hello"}"#
        );
    }

    #[test]
    fn tool_messages_use_openai_compatible_shapes() {
        let mut assistant = Message::new(Role::Assistant, "");
        assistant.tool_calls = Some(vec![ToolCall::new(
            "call_1",
            "write_file",
            r#"{"path":"test.html"}"#,
        )]);
        assert_eq!(
            serde_json::to_string(&assistant).unwrap(),
            r#"{"role":"assistant","content":"","tool_calls":[{"id":"call_1","type":"function","function":{"name":"write_file","arguments":"{\"path\":\"test.html\"}"}}]}"#
        );

        let result = Message::tool_result("call_1", "Wrote 12 bytes.");
        assert_eq!(
            serde_json::to_string(&result).unwrap(),
            r#"{"role":"tool","content":"Wrote 12 bytes.","tool_call_id":"call_1"}"#
        );
    }

    #[test]
    fn reasoning_round_trips_but_empty_is_skipped() {
        let message = Message::new(Role::User, "hello");
        assert_eq!(
            serde_json::to_string(&message).unwrap(),
            r#"{"role":"user","content":"hello"}"#
        );
        let mut thinking = Message::new(Role::Assistant, "answer");
        thinking.reasoning = "let me think".to_owned();
        assert_eq!(
            serde_json::to_string(&thinking).unwrap(),
            r#"{"role":"assistant","content":"answer","reasoning":"let me think"}"#
        );
    }

    #[test]
    fn incomplete_tool_calls_are_removed_with_their_results() {
        let mut session = Session::new();
        session
            .messages
            .push(Message::new(Role::User, "write a file"));
        let mut assistant = Message::new(Role::Assistant, "");
        assistant.tool_calls = Some(vec![
            ToolCall::new("call_1", "write_file", "{}"),
            ToolCall::new("call_2", "read_file", "{}"),
        ]);
        session.messages.push(assistant);
        session.messages.push(Message::tool_result("call_1", "ok"));

        session.repair_messages();

        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.messages[0].content, "write a file");
    }

    #[test]
    fn compaction_cuts_at_a_user_turn_and_keeps_the_summary() {
        let mut session = Session::new();
        session.messages.push(Message::new(Role::User, "first"));
        let mut assistant = Message::new(Role::Assistant, "");
        assistant.tool_calls = Some(vec![ToolCall::new("call_1", "read_file", "{}")]);
        session.messages.push(assistant);
        session.messages.push(Message::tool_result("call_1", "ok"));
        session.messages.push(Message::new(Role::User, "second"));
        session
            .messages
            .push(Message::new(Role::Assistant, "answer"));

        // A budget the whole history fits never splits.
        assert_eq!(session.compaction_split(u64::MAX), Some(0));
        // A tiny budget still lands on a user message, never mid tool pair.
        let split = session.compaction_split(1).unwrap();
        assert_eq!(session.messages[split].role, Role::User);

        session.compact(split, "the user asked about files".to_owned());
        assert_eq!(session.messages.len(), 3);
        assert!(
            session.messages[0]
                .content
                .contains("the user asked about files")
        );
        assert_eq!(session.messages[1].content, "second");
        assert_eq!(session.messages[2].content, "answer");
        assert_eq!(
            session.summary.as_deref(),
            Some("the user asked about files")
        );

        // Round-trips through the session file as an optional field.
        let json = serde_json::to_string(&session).unwrap();
        let restored: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.summary.as_deref(), session.summary.as_deref());
    }
}
