use std::{collections::BTreeMap, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt;
use reqwest::{Client, Response};
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::{
    config::{AuthKind, Config, ProviderKind, SavedModel},
    session::{Message, Role, ToolCall},
    skills,
    tools,
};

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

    pub fn set_model(&mut self, model: String) {
        self.model = model;
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
        let response = send_with_retry("OpenAI-compatible API", || {
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
        let response = check_response(response, "OpenAI-compatible API").await?;
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
        let response = send_with_retry("Ollama", || {
            self.client
                .post(format!("{}/api/chat", self.base_url))
                .json(&request)
        })
        .await
        .context("connecting to Ollama; is `ollama serve` running?")?;
        let response = check_response(response, "Ollama").await?;
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

/// How images ride along in request messages: OpenAI-compatible multimodal
/// text parts, Ollama's `images` base64 array, or not at all.
#[derive(Clone, Copy, Default, Debug, Eq, PartialEq)]
enum ImageStyle {
    #[default]
    None,
    Parts,
    Ollama,
}

fn request_messages(
    messages: &[Message],
    tooling: bool,
    image_style: ImageStyle,
    custom_prompt: Option<&str>,
    mode_note: Option<&'static str>,
) -> Result<Vec<serde_json::Value>> {
    let mut request = Vec::with_capacity(messages.len() + 1);
    let mut system = tools::system_prompt(tooling);
    // The user's prompt may duplicate what a built-in line already says: send
    // it anyway, since a provider may also enforce its own system prompt.
    if let Some(custom) = custom_prompt.filter(|custom| !custom.trim().is_empty()) {
        system.push_str("\n\n");
        system.push_str(custom.trim());
    }
    if let Some(note) = mode_note {
        system.push_str("\n\n");
        system.push_str(note);
    }
    // Skills auto-load through read_file, so only tool-capable sessions get
    // the listing; slash invocation injects the content by itself.
    if tooling {
        if let Some(listing) =
            skills::section(&std::env::current_dir().unwrap_or_default())
        {
            system.push_str("\n\n");
            system.push_str(&listing);
        }
    }
    request.push(serde_json::json!({
        "role": "system",
        "content": system,
    }));
    for message in messages {
        if message.role == Role::Tool && !tooling {
            request.push(serde_json::json!({
                "role": "user",
                "content": format!("Tool result:\n{}", message.content),
            }));
            continue;
        }
        // Attached screenshots travel so a vision-capable model sees them:
        // multimodal parts for OpenAI-compatible providers, Ollama's own
        // `images` array there.
        let openai_parts = image_style == ImageStyle::Parts;
        if openai_parts && message.role == Role::User && !message.images.is_empty() {
            let mut parts: Vec<serde_json::Value> = Vec::with_capacity(1 + message.images.len());
            if !message.content.is_empty() {
                parts.push(serde_json::json!({ "type": "text", "text": message.content }));
            }
            for image in &message.images {
                parts.push(serde_json::json!({
                    "type": "image_url",
                    "image_url": {
                        "url": format!("data:image/png;base64,{}", image.data)
                    }
                }));
            }
            request.push(serde_json::json!({ "role": "user", "content": parts }));
            continue;
        }
        if image_style == ImageStyle::Ollama
            && message.role == Role::User
            && !message.images.is_empty()
        {
            let mut value = serde_json::to_value(message)?;
            if let Some(object) = value.as_object_mut() {
                let attached = object
                    .remove("images")
                    .and_then(|images| images.as_array().cloned())
                    .unwrap_or_default();
                let images = attached
                    .iter()
                    .filter_map(|image| image.get("data").and_then(serde_json::Value::as_str))
                    .map(|data| serde_json::Value::String(data.to_owned()))
                    .collect::<Vec<_>>();
                object.insert("images".to_owned(), serde_json::Value::from(images));
            }
            request.push(value);
            continue;
        }
        if message.role == Role::Assistant
            && message.content.is_empty()
            && message.tool_calls.is_none()
        {
            continue;
        }
        let mut value = serde_json::to_value(message)?;
        if !tooling {
            if let Some(object) = value.as_object_mut() {
                object.remove("tool_calls");
                object.remove("tool_call_id");
            }
            if message.role == Role::Assistant && message.content.is_empty() {
                continue;
            }
        }
        // Parts and Ollama branches above already carry the images; anything
        // left in the plain path must not leak the raw attachment field.
        if let Some(object) = value.as_object_mut() {
            object.remove("images");
        }
        request.push(value);
    }
    Ok(request)
}

/// How long to wait between connection retries, doubling up to the cap.
const RETRY_MIN_DELAY: Duration = Duration::from_secs(1);
const RETRY_MAX_DELAY: Duration = Duration::from_secs(30);

/// Send a request, retrying forever while the provider is unreachable.
///
/// DNS failures and refused connections are usually a dropped network or a
/// sleeping server, so giving up wastes a turn the user can do nothing about.
/// HTTP error statuses are *not* retried; those are answered by `check_response`.
async fn send_with_retry<F>(provider: &str, build: F) -> Result<Response>
where
    F: Fn() -> reqwest::RequestBuilder + Send,
{
    let mut delay = RETRY_MIN_DELAY;
    let mut attempt: u32 = 0;
    loop {
        attempt = attempt.saturating_add(1);
        match build().send().await {
            Ok(response) => return Ok(response),
            Err(error) => {
                let transient = error.is_connect() || error.is_timeout() || error.is_request();
                if !transient {
                    return Err(error).with_context(|| format!("connecting to {provider}"));
                }
                // Long outages should not look like a hang.
                if attempt > 3 {
                    eprintln!(
                        "{provider} unreachable ({attempt} tries); retrying in {}s · ctrl+c to stop",
                        delay.as_secs()
                    );
                }
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(RETRY_MAX_DELAY);
            }
        }
    }
}

async fn check_response(response: Response, provider: &str) -> Result<Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    let detail = response_error_detail(&body);
    bail!("{provider} returned {}: {detail}", status.as_u16());
}

fn response_error_detail(body: &str) -> String {
    let value = serde_json::from_str::<serde_json::Value>(body).ok();
    let detail = value
        .as_ref()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .or_else(|| value.get("error"))
                .or_else(|| value.get("message"))
        })
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| body.to_owned());
    let detail = detail.trim();
    if detail.is_empty() {
        return "empty error response".to_owned();
    }
    detail.chars().take(500).collect()
}

/// How long a stream may go silent before we stop waiting on it.
///
/// Long reasoning can pause for a while between chunks, so this is generous;
/// without it a dropped connection stalls the response with no way to finish.
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(180);

async fn consume_lines<F>(response: Response, mut on_line: F) -> Result<()>
where
    F: FnMut(&str) -> Result<bool>,
{
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    loop {
        let chunk = match tokio::time::timeout(STREAM_IDLE_TIMEOUT, stream.next()).await {
            Ok(Some(chunk)) => chunk?,
            Ok(None) => break,
            Err(_) => bail!(
                "the provider stopped sending for {}s; the connection may have dropped",
                STREAM_IDLE_TIMEOUT.as_secs()
            ),
        };
        buffer.extend_from_slice(&chunk);
        while let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
            let bytes = buffer.drain(..=newline).collect::<Vec<_>>();
            let line = std::str::from_utf8(&bytes[..newline])?.trim_end_matches('\r');
            if !on_line(line)? {
                return Ok(());
            }
        }
    }
    if !buffer.is_empty() {
        on_line(std::str::from_utf8(&buffer)?)?;
    }
    Ok(())
}

#[derive(Debug, Eq, PartialEq)]
enum ParsedEvent {
    Delta(String),
    Reasoning(String),
    ToolProgress {
        name: String,
        detail: String,
        bytes: u64,
    },
    Usage {
        prompt_tokens: u64,
        completion_tokens: u64,
    },
}

fn send_parsed(
    request_id: u64,
    events: Vec<ParsedEvent>,
    done: bool,
    sender: &mpsc::UnboundedSender<StreamEvent>,
) -> Result<bool> {
    for event in events {
        match event {
            ParsedEvent::Delta(text) => {
                sender.send(StreamEvent::Delta { request_id, text })?;
            }
            ParsedEvent::Reasoning(text) => {
                sender.send(StreamEvent::Reasoning { request_id, text })?;
            }
            ParsedEvent::ToolProgress {
                name,
                detail,
                bytes,
            } => {
                sender.send(StreamEvent::ToolProgress {
                    request_id,
                    name,
                    detail,
                    bytes,
                })?;
            }
            ParsedEvent::Usage {
                prompt_tokens,
                completion_tokens,
            } => {
                sender.send(StreamEvent::Usage {
                    request_id,
                    prompt_tokens,
                    completion_tokens,
                })?;
            }
        }
    }
    Ok(!done)
}

#[derive(Default)]
struct OpenAiState {
    calls: BTreeMap<usize, ToolCallBuilder>,
}

impl OpenAiState {
    fn record(&mut self, calls: Vec<OpenAiToolCall>) -> Vec<ParsedEvent> {
        for call in calls {
            let builder = self.calls.entry(call.index).or_default();
            if let Some(id) = call.id.filter(|id| !id.is_empty()) {
                if builder.id.is_empty() {
                    builder.id = id;
                }
            }
            let Some(function) = call.function else {
                continue;
            };
            if let Some(name) = function.name.filter(|name| !name.is_empty()) {
                if builder.name.is_empty() {
                    builder.name = name;
                } else if !builder.name.ends_with(&name) {
                    builder.name.push_str(&name);
                }
            }
            if let Some(arguments) = function.arguments {
                builder.arguments.push_str(&arguments);
            }
        }
        self.progress()
    }

    fn progress(&mut self) -> Vec<ParsedEvent> {
        self.calls
            .values_mut()
            .filter_map(|builder| {
                let bytes = builder.arguments.len() as u64;
                let fresh = !builder.name.is_empty() && !builder.progressed;
                let grew = bytes - builder.progress_bytes >= PROGRESS_STEP;
                if !fresh && !grew {
                    return None;
                }
                builder.progressed = true;
                builder.progress_bytes = bytes;
                let detail = progress_detail(&builder.name, &builder.arguments);
                Some(ParsedEvent::ToolProgress {
                    name: builder.name.clone(),
                    detail,
                    bytes,
                })
            })
            .collect()
    }

    fn finish(self, request_id: u64) -> Vec<ToolCall> {
        self.calls
            .into_iter()
            .filter_map(|(index, builder)| {
                let name = builder.name.trim();
                if name.is_empty() {
                    return None;
                }
                let id = if builder.id.is_empty() {
                    format!("call_{request_id}_{index}")
                } else {
                    builder.id
                };
                let arguments = if builder.arguments.trim().is_empty() {
                    "{}".to_owned()
                } else {
                    builder.arguments
                };
                Some(ToolCall::new(id, name, arguments))
            })
            .collect()
    }
}

#[derive(Default)]
struct ToolCallBuilder {
    id: String,
    name: String,
    arguments: String,
    progressed: bool,
    progress_bytes: u64,
}

const PROGRESS_STEP: u64 = 1024;

/// Best-effort read of one string field from partially streamed tool arguments.
fn progress_detail(name: &str, arguments: &str) -> String {
    let key = match name {
        "list_dir" | "read_file" | "write_file" | "edit_file" => "path",
        "run_command" => "command",
        _ => return String::new(),
    };
    let marker = format!("\"{key}\"");
    let Some(start) = arguments.find(&marker) else {
        return String::new();
    };
    let rest = arguments[start + marker.len()..].trim_start();
    let Some(rest) = rest.strip_prefix(':').map(str::trim_start) else {
        return String::new();
    };
    let Some(rest) = rest.strip_prefix('"') else {
        return String::new();
    };
    let mut value = String::new();
    let mut escaped = false;
    for character in rest.chars() {
        if escaped {
            value.push(character);
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '"' => break,
            _ => value.push(character),
        }
    }
    value
}

#[derive(Deserialize)]
struct OpenAiChunk {
    #[serde(default)]
    choices: Vec<OpenAiChoice>,
    error: Option<OpenAiError>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Deserialize)]
struct OpenAiUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    delta: OpenAiDelta,
}

#[derive(Deserialize)]
struct OpenAiDelta {
    content: Option<String>,
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OpenAiToolCall>>,
}

#[derive(Deserialize)]
struct OpenAiToolCall {
    #[serde(default)]
    index: usize,
    id: Option<String>,
    function: Option<OpenAiFunctionDelta>,
}

#[derive(Deserialize)]
struct OpenAiFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiError {
    message: String,
}

fn parse_openai_line(line: &str, state: &mut OpenAiState) -> Result<(Vec<ParsedEvent>, bool)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with(':') {
        return Ok((Vec::new(), false));
    }
    let Some(data) = line.strip_prefix("data:") else {
        return Ok((Vec::new(), false));
    };
    let data = data.trim();
    if data == "[DONE]" {
        return Ok((Vec::new(), true));
    }
    let chunk: OpenAiChunk = serde_json::from_str(data).context("invalid OpenAI stream chunk")?;
    if let Some(error) = chunk.error {
        bail!("OpenAI stream error: {}", error.message);
    }
    let mut events = Vec::new();
    if let Some(usage) = chunk.usage {
        if usage.prompt_tokens > 0 || usage.completion_tokens > 0 {
            events.push(ParsedEvent::Usage {
                prompt_tokens: usage.prompt_tokens,
                completion_tokens: usage.completion_tokens,
            });
        }
    }
    for choice in chunk.choices {
        if let Some(text) = choice.delta.content.filter(|text| !text.is_empty()) {
            events.push(ParsedEvent::Delta(text));
        }
        if let Some(text) = choice
            .delta
            .reasoning_content
            .filter(|text| !text.is_empty())
        {
            events.push(ParsedEvent::Reasoning(text));
        }
        if let Some(calls) = choice.delta.tool_calls {
            events.extend(state.record(calls));
        }
    }
    Ok((events, false))
}

#[derive(Deserialize)]
struct OllamaChunk {
    message: Option<OllamaMessage>,
    done: Option<bool>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct OllamaMessage {
    content: String,
}

fn parse_ollama_line(line: &str) -> Result<(Vec<ParsedEvent>, bool)> {
    if line.trim().is_empty() {
        return Ok((Vec::new(), false));
    }
    let chunk: OllamaChunk = serde_json::from_str(line).context("invalid Ollama stream chunk")?;
    if let Some(error) = chunk.error {
        bail!("Ollama stream error: {error}");
    }
    if chunk.done.unwrap_or(false) {
        return Ok((Vec::new(), true));
    }
    let mut events = Vec::new();
    if let Some(text) = chunk
        .message
        .map(|message| message.content)
        .filter(|text| !text.is_empty())
    {
        events.push(ParsedEvent::Delta(text));
    }
    Ok((events, false))
}

#[cfg(test)]
mod tests {
    use super::{
        ImageStyle, OpenAiState, ParsedEvent, completion_url, parse_ollama_line,
        parse_openai_line, progress_detail, request_messages, response_error_detail,
    };
    use crate::session::{Message, Role, ToolCall};

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

    #[test]
    fn parses_openai_deltas_and_done() {
        let mut state = OpenAiState::default();
        assert_eq!(
            parse_openai_line(": keep-alive", &mut state).unwrap(),
            (Vec::new(), false)
        );
        assert_eq!(
            parse_openai_line("event: message", &mut state).unwrap(),
            (Vec::new(), false)
        );
        let (events, done) = parse_openai_line(
            r#"data: {"choices":[{"delta":{"content":"Hello"}}]}"#,
            &mut state,
        )
        .unwrap();
        assert_eq!(done, false);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ParsedEvent::Delta(text) => assert_eq!(text, "Hello"),
            other => panic!("expected delta, got {other:?}"),
        }
        assert_eq!(
            parse_openai_line("data: [DONE]", &mut state).unwrap(),
            (Vec::new(), true)
        );
    }

    #[test]
    fn accumulates_streamed_tool_calls() {
        let mut state = OpenAiState::default();
        parse_openai_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","function":{"name":"write_file","arguments":"{\"path\":"}}]}}]}"#,
            &mut state,
        )
        .unwrap();
        parse_openai_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"test.html\"}"}}]}}]}"#,
            &mut state,
        )
        .unwrap();
        parse_openai_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"id":"call_b","function":{"name":"read_file","arguments":"{\"path\":\"src/main.rs\"}"}}]}}]}"#,
            &mut state,
        )
        .unwrap();

        let calls = state.finish(7);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "call_a");
        assert_eq!(calls[0].function.name, "write_file");
        assert_eq!(calls[0].function.arguments, r#"{"path":"test.html"}"#);
        assert_eq!(calls[1].id, "call_b");
        assert_eq!(calls[1].function.name, "read_file");
    }

    #[test]
    fn request_messages_keep_tool_history_only_when_tooling_is_enabled() {
        let mut assistant = Message::new(Role::Assistant, "");
        assistant.tool_calls = Some(vec![ToolCall::new(
            "call_1",
            "read_file",
            r#"{"path":"README.md"}"#,
        )]);
        let messages = vec![
            Message::new(Role::User, "read it"),
            assistant,
            Message::tool_result("call_1", "contents"),
        ];

        let tooling = request_messages(&messages, true, ImageStyle::None, None, None).unwrap();
        assert_eq!(tooling[0]["role"], "system");
        assert_eq!(tooling[2]["tool_calls"][0]["function"]["name"], "read_file");
        assert_eq!(tooling[3]["tool_call_id"], "call_1");

        let plain = request_messages(&messages, false, ImageStyle::None, None, None).unwrap();
        assert!(
            plain
                .iter()
                .all(|message| message.get("tool_calls").is_none())
        );
        assert!(plain.iter().any(|message| {
            message["content"]
                .as_str()
                .is_some_and(|content| content.contains("Tool result"))
        }));
    }

    #[test]
    fn the_mode_instruction_lands_in_the_system_prompt() {
        let messages = vec![Message::new(Role::User, "hi")];
        let plain = request_messages(&messages, true, ImageStyle::None, None, None).unwrap();
        let plan = request_messages(
            &messages,
            true,
            ImageStyle::None,
            None,
            Some("PLAN MODE: propose only"),
        )
        .unwrap();
        assert!(
            !plain[0]["content"]
                .as_str()
                .is_some_and(|content| content.contains("PLAN MODE"))
        );
        assert!(
            plan[0]["content"]
                .as_str()
                .is_some_and(|content| content.contains("PLAN MODE: propose only"))
        );
    }

    #[test]
    fn the_custom_system_prompt_lands_between_base_and_mode() {
        let messages = vec![Message::new(Role::User, "hi")];
        let request = request_messages(
            &messages,
            true,
            ImageStyle::None,
            Some(" Always answer in rhyme. "),
            Some("PLAN MODE: propose only"),
        )
        .unwrap();
        let system = request[0]["content"].as_str().unwrap();
        assert!(system.contains("Working directory:"));
        assert!(system.contains("Always answer in rhyme."));
        // The custom text precedes the mode instruction, built-ins first.
        assert!(system.find("Always answer").unwrap() < system.find("PLAN MODE").unwrap());
        // A blank prompt adds nothing.
        assert_eq!(
            request_messages(
                &messages,
                true,
                ImageStyle::None,
                Some("  "),
                None
            )
            .unwrap(),
            request_messages(&messages, true, ImageStyle::None, None, None).unwrap()
        );
    }

    #[test]
    fn attached_images_travel_in_the_providers_own_format() {
        use crate::session::ImageAttachment;

        let mut message = Message::new(Role::User, "what changed?");
        message.images.push(ImageAttachment {
            name: "shot.png".to_owned(),
            data: "aWNvbg==".to_owned(),
        });
        let messages = vec![message];

        let parts = request_messages(&messages, true, ImageStyle::Parts, None, None).unwrap();
        let sent = parts
            .last()
            .unwrap()
            .get("content")
            .and_then(serde_json::Value::as_array)
            .unwrap();
        let urls = sent
            .iter()
            .filter_map(|part| part.get("image_url").and_then(serde_json::Value::as_object))
            .collect::<Vec<_>>();
        assert_eq!(urls.len(), 1);
        assert_eq!(
            urls[0]["url"],
            "data:image/png;base64,aWNvbg=="
        );
        assert!(parts.last().unwrap().get("images").is_none());

        // Ollama wants a flat base64 array on the message itself.
        let ollama = request_messages(&messages, true, ImageStyle::Ollama, None, None).unwrap();
        let last = ollama.last().unwrap();
        assert_eq!(last["content"], "what changed?");
        assert_eq!(last["images"][0], "aWNvbg==");
        // Plain requests keep the raw attachment field out.
        let plain =
            request_messages(&messages, false, ImageStyle::None, None, None).unwrap();
        assert!(plain.last().unwrap().get("images").is_none());
        assert_eq!(plain.last().unwrap()["role"], "user");
    }

    #[test]
    fn parses_ollama_deltas_and_done() {
        assert_eq!(
            parse_ollama_line(r#"{"message":{"content":"Hi"},"done":false}"#).unwrap(),
            (vec![ParsedEvent::Delta("Hi".to_owned())], false)
        );
        assert_eq!(
            parse_ollama_line(r#"{"done":true}"#).unwrap(),
            (Vec::new(), true)
        );
    }

    #[test]
    fn stream_errors_are_not_silently_ignored() {
        let mut state = OpenAiState::default();
        assert!(parse_openai_line(r#"data: {"error":{"message":"bad key"}}"#, &mut state).is_err());
        assert!(parse_ollama_line(r#"{"error":"model missing"}"#).is_err());
        assert_eq!(
            response_error_detail(r#"{"error":{"message":"nope"}}"#),
            "nope"
        );
    }

    #[test]
    fn usage_arrives_from_the_final_chunk() {
        let mut state = OpenAiState::default();
        let (events, done) = parse_openai_line(
            r#"data: {"choices":[],"usage":{"prompt_tokens":1234,"completion_tokens":57}}"#,
            &mut state,
        )
        .unwrap();
        assert_eq!(done, false);
        assert_eq!(
            events,
            vec![ParsedEvent::Usage {
                prompt_tokens: 1234,
                completion_tokens: 57
            }]
        );
        // Zero usage (intermediate chunks) stays silent.
        let (events, _) = parse_openai_line(
            r#"data: {"choices":[],"usage":{"prompt_tokens":0,"completion_tokens":0}}"#,
            &mut state,
        )
        .unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn reasoning_stays_apart_from_content() {
        let mut state = OpenAiState::default();
        let (events, _) = parse_openai_line(
            r#"data: {"choices":[{"delta":{"reasoning_content":"Let me think...","content":"So the"}}"],"usage":{"prompt_tokens":3,"completion_tokens":9}}"#,
            &mut state,
        )
        .unwrap();
        assert_eq!(
            events,
            vec![
                ParsedEvent::Usage {
                    prompt_tokens: 3,
                    completion_tokens: 9
                },
                ParsedEvent::Delta("So the".to_owned()),
                ParsedEvent::Reasoning("Let me think...".to_owned())
            ]
        );
    }

    #[test]
    fn tool_progress_tracks_arguments_as_they_stream() {
        let mut state = OpenAiState::default();
        let (events, _) = parse_openai_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","function":{"name":"write_file","arguments":"{\"path\":\"test.html\",\"content\":\"<h1>hi</h1>\""}}]}}]}"#,
            &mut state,
        )
        .unwrap();
        let mut progress = events
            .into_iter()
            .filter_map(|event| match event {
                ParsedEvent::ToolProgress {
                    name,
                    detail,
                    bytes,
                } => Some((name, detail, bytes)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(progress.len(), 1);
        let (name, detail, bytes) = progress.remove(0);
        assert_eq!(name, "write_file");
        assert_eq!(detail, "test.html");
        assert!(bytes > 0);

        // Small deltas do not re-emit; a big one does.
        for _ in 0..3 {
            let (events, _) = parse_openai_line(
                r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"a"}}]}}]}"#,
                &mut state,
            )
            .unwrap();
            assert!(
                events
                    .iter()
                    .all(|event| !matches!(event, ParsedEvent::ToolProgress { .. }))
            );
        }
        let big = format!(
            "{{\"path\":\"test.html\",\"content\":\"{}\"}}",
            "x".repeat(2048)
        );
        let big = big.replace('"', "\\\"");
        let (events, _) = parse_openai_line(
            &format!(
                r#"data: {{"choices":[{{"delta":{{"tool_calls":[{{"index":0,"function":{{"arguments":"{big}"}}}}]}}}}]}}""#
            ),
            &mut state,
        )
        .unwrap();
        assert!(
            events.iter().any(
                |event| matches!(event, ParsedEvent::ToolProgress { bytes, .. } if *bytes > 0)
            )
        );

        let calls = state.finish(7);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "write_file");
        assert!(
            calls[0]
                .function
                .arguments
                .contains("x".repeat(2048).as_str())
        );
    }

    #[test]
    fn progress_detail_reads_partial_json() {
        assert_eq!(
            progress_detail(
                "write_file",
                r##"{"path":"src/app.rs","content":"# [incomplete"##
            ),
            "src/app.rs"
        );
        assert_eq!(
            progress_detail("run_command", r#"{"command":"echo \"hi\" and more"}"#),
            "echo \"hi\" and more"
        );
        assert_eq!(
            progress_detail("write_file", r#"{"cont[incomplete without key"#),
            ""
        );
        assert_eq!(progress_detail("list_dir", "{}"), "");
    }
}
