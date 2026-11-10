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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn new(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn tool_result(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: None,
            tool_call_id: Some(call_id.into()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    version: u32,
    pub id: u64,
    pub title: String,
    pub messages: Vec<Message>,
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
        loop {
            let Some(position) = self.messages.iter().rposition(|message| {
                message.role == Role::Assistant
                    && message
                        .tool_calls
                        .as_ref()
                        .is_some_and(|calls| !calls.is_empty())
            }) else {
                break;
            };
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
}
