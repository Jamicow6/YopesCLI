use std::{
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
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

impl Message {
    pub fn new(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
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
        while session
            .messages
            .last()
            .is_some_and(|message| message.role == Role::Assistant && message.content.is_empty())
        {
            session.messages.pop();
        }
        Ok(session)
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
    use super::{Message, Role, Session};

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
}
