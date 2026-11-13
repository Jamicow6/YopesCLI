//! HTTP client for OpenAI-compatible and Ollama endpoints.
//!
//! `mod.rs` holds `Backend`, the public entry point, and `StreamEvent`, the
//! vocabulary it reports back with. `messages` builds the request bodies,
//! `retry` sends them with backoff, and `sse` parses the streamed replies.

mod messages;
mod retry;
mod sse;

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use reqwest::Client;
use tokio::sync::mpsc;

use crate::{
    config::{AuthKind, Config, ProviderKind, SavedModel},
    session::{Message, Role, ToolCall},
    tools,
};

use messages::{ImageStyle, request_messages};
use retry::send_with_retry;
use sse::{OpenAiState, consume_lines, parse_ollama_line, parse_openai_line, send_parsed};

#[derive(Debug)]
pub enum StreamEvent {
    Delta {
        request_id: u64,
        text: String,
    },
    Reasoning {
        request_id: u64,
        text: String,
    },
    ToolProgress {
        request_id: u64,
        name: String,
        detail: String,
        bytes: u64,
    },
    Usage {
        request_id: u64,
        prompt_tokens: u64,
        completion_tokens: u64,
    },
    ToolCalls {
        request_id: u64,
        calls: Vec<ToolCall>,
    },
    /// A request failed and is being tried again.
    Retrying {
        request_id: u64,
        attempt: u32,
        delay_secs: u64,
        reason: String,
    },
    Done {
        request_id: u64,
    },
    Error {
        request_id: u64,
        message: String,
    },
}

#[derive(Clone)]
pub struct Backend {
    provider: ProviderKind,
    provider_label: String,
    model: String,
    base_url: String,
    api_key: Option<String>,
    auth: AuthKind,
    models: Vec<SavedModel>,
    /// Custom system prompt text, appended to the built-in one.
    custom_prompt: Option<String>,
    /// Reasoning effort hint sent with OpenAI-compatible requests.
    reasoning_effort: Option<String>,
    client: Client,
}

impl Backend {
    pub fn new(config: &Config) -> Result<Self> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .user_agent(concat!("yopes/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("creating HTTP client")?;
        Ok(Self {
            provider: config.provider,
            provider_label: config.provider_label.clone(),
            model: config.model.clone(),
            base_url: config.base_url.trim_end_matches('/').to_owned(),
            api_key: config.api_key.clone(),
            auth: config.auth,
            models: config.models.clone(),
            custom_prompt: config.system_prompt.clone(),
            reasoning_effort: config.reasoning_effort.clone(),
            client,
        })
    }

    pub const fn is_configured(&self) -> bool {
        !matches!(self.provider, ProviderKind::Unconfigured)
    }

    pub const fn provider_kind(&self) -> ProviderKind {
        self.provider
    }

    pub fn provider_label(&self) -> &str {
        &self.provider_label
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    pub const fn auth(&self) -> AuthKind {
        self.auth
    }

    pub fn saved_models(&self) -> &[SavedModel] {
        &self.models
    }

    /// Dollars per million tokens for the active model, when the catalog that
    /// saved this connection listed a price.
    pub fn model_price(&self) -> Option<(f64, f64)> {
        self.models
            .iter()
            .find(|saved| {
                saved.id.eq_ignore_ascii_case(&self.model)
                    || (!saved.name.is_empty() && saved.name.eq_ignore_ascii_case(&self.model))
            })
            .and_then(|saved| saved.input_cost.zip(saved.output_cost))
    }
    pub fn set_model(&mut self, model: String) {
        self.model = model;
    }

    pub fn update_model_context_window(&mut self, model_id: &str, context_window: Option<u64>) {
        for model in &mut self.models {
            if model.id.eq_ignore_ascii_case(model_id) {
                model.context_window = context_window;
            }
        }
    }

    pub fn system_prompt(&self) -> Option<&str> {
        self.custom_prompt.as_deref()
    }

    pub fn set_system_prompt(&mut self, value: Option<String>) {
        self.custom_prompt = value;
    }

    pub fn reasoning_effort(&self) -> Option<&str> {
        self.reasoning_effort.as_deref()
    }

    pub fn set_reasoning_effort(&mut self, value: Option<String>) {
        self.reasoning_effort = value;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn configure(
        &mut self,
        provider: ProviderKind,
        provider_label: String,
        model: String,
        base_url: String,
        api_key: Option<String>,
        auth: AuthKind,
        models: Vec<SavedModel>,
    ) {
        self.provider = provider;
        self.provider_label = provider_label;
        self.model = model;
        self.base_url = base_url.trim_end_matches('/').to_owned();
        self.api_key = api_key;
        self.auth = auth;
        self.models = models;
    }

    /// One helper request for the `delegate` tool: empty conversation, no
    /// tools, and the answer text as the result. Uses `mode_note` as the
    /// subagent brief appended to the system prompt.
    pub async fn complete_once(self, brief: &'static str, task: &str) -> Result<String> {
        // The request id never reaches the app: this channel is task-local.
        let request_id = u64::MAX;
        let (events, mut receiver) = mpsc::unbounded_channel();
        let messages = vec![Message::new(Role::User, task)];
        self.stream(request_id, messages, false, Some(brief), events)
            .await;
        let mut text = String::new();
        while let Some(event) = receiver.recv().await {
            match event {
                StreamEvent::Delta { text: delta, .. } => text.push_str(&delta),
                StreamEvent::Error { message, .. } => bail!("{message}"),
                StreamEvent::Done { .. } => break,
                _ => {}
            }
        }
        if text.is_empty() {
            bail!("the subagent returned no content");
        }
        Ok(text.trim().to_owned())
    }

    pub async fn stream(
        self,
        request_id: u64,
        messages: Vec<Message>,
        tools_enabled: bool,
        mode_note: Option<&'static str>,
        events: mpsc::UnboundedSender<StreamEvent>,
    ) {
        let tooling =
            tools_enabled && matches!(self.provider, ProviderKind::OpenAi | ProviderKind::Custom);
        let result = match self.provider {
            ProviderKind::Unconfigured => Err(anyhow!(
                "no provider connected; run /connect to get started"
            )),
            ProviderKind::OpenAi | ProviderKind::Custom => {
                self.stream_openai(request_id, &messages, tooling, mode_note, &events)
                    .await
            }
            ProviderKind::Ollama => {
                self.stream_ollama(request_id, &messages, mode_note, &events)
                    .await
            }
        };
        match result {
            Ok(()) => {
                let _ = events.send(StreamEvent::Done { request_id });
            }
            Err(error) => {
                let _ = events.send(StreamEvent::Error {
                    request_id,
                    message: format!("{error:#}"),
                });
            }
        }
    }

    async fn stream_openai(
        &self,
        request_id: u64,
        messages: &[Message],
        tooling: bool,
        mode_note: Option<&'static str>,
        events: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<()> {
        let mut request = serde_json::json!({
            "model": self.model,
            "messages": request_messages(
                messages,
                tooling,
                if matches!(
                    self.provider,
                    ProviderKind::OpenAi | ProviderKind::Custom
                ) {
                    ImageStyle::Parts
                } else {
                    ImageStyle::None
                },
                self.custom_prompt.as_deref(),
                mode_note,
            )?,
            "stream": true,
            "stream_options": { "include_usage": true },
        });
        if tooling {
            request["tools"] = tools::definitions();
        }
        if let Some(effort) = self
            .reasoning_effort
            .as_deref()
            .filter(|effort| !effort.trim().is_empty())
        {
            request["reasoning_effort"] = serde_json::Value::String(effort.to_owned());
        }
        let response = send_with_retry("OpenAI-compatible API", request_id, events, || {
            let mut builder = self
                .client
                .post(completion_url(&self.base_url))
                .json(&request);
            if let Some(api_key) = &self.api_key {
                builder = match self.auth {
                    AuthKind::Bearer => builder.bearer_auth(api_key),
                    AuthKind::XApiKey => builder.header("x-api-key", api_key.as_str()),
                    AuthKind::None => builder,
                };
            }
            builder
        })
        .await?;
        let mut state = OpenAiState::default();
        consume_lines(response, |line| {
            let (parsed, done) = parse_openai_line(line, &mut state)?;
            send_parsed(request_id, parsed, done, events)
        })
        .await?;
        let calls = state.finish(request_id);
        if !calls.is_empty() {
            events.send(StreamEvent::ToolCalls { request_id, calls })?;
        }
        Ok(())
    }

    async fn stream_ollama(
        &self,
        request_id: u64,
        messages: &[Message],
        mode_note: Option<&'static str>,
        events: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<()> {
        let request = serde_json::json!({
            "model": self.model,
            "messages": request_messages(
                messages,
                false,
                ImageStyle::Ollama,
                self.custom_prompt.as_deref(),
                mode_note,
            )?,
            "stream": true,
        });
        let response = send_with_retry("Ollama", request_id, events, || {
            self.client
                .post(format!("{}/api/chat", self.base_url))
                .json(&request)
        })
        .await?;
        consume_lines(response, |line| {
            let (parsed, done) = parse_ollama_line(line)?;
            send_parsed(request_id, parsed, done, events)
        })
        .await
    }
}

fn completion_url(base_url: &str) -> String {
    let base_url = base_url.trim_end_matches('/');
    if base_url.ends_with("/chat/completions") {
        base_url.to_owned()
    } else {
        format!("{base_url}/chat/completions")
    }
}

#[cfg(test)]
mod tests {
    use super::completion_url;

    #[test]
    fn completion_url_accepts_a_base_or_full_endpoint() {
        assert_eq!(
            completion_url("https://api.example.com/v1"),
            "https://api.example.com/v1/chat/completions"
        );
        assert_eq!(
            completion_url("https://api.example.com/v1/chat/completions/"),
            "https://api.example.com/v1/chat/completions"
        );
    }
}
