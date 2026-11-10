use std::{collections::BTreeMap, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt;
use reqwest::{Client, Response};
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::{
    config::{AuthKind, Config, ProviderKind, SavedModel},
    session::{Message, Role, ToolCall},
    tools,
};

#[derive(Debug)]
pub enum StreamEvent {
    Delta {
        request_id: u64,
        text: String,
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

    pub const fn auth(&self) -> AuthKind {
        self.auth
    }

    pub fn saved_models(&self) -> &[SavedModel] {
        &self.models
    }

    pub fn set_model(&mut self, model: String) {
        self.model = model;
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

    pub async fn stream(
        self,
        request_id: u64,
        messages: Vec<Message>,
        tools_enabled: bool,
        events: mpsc::UnboundedSender<StreamEvent>,
    ) {
        let tooling =
            tools_enabled && matches!(self.provider, ProviderKind::OpenAi | ProviderKind::Custom);
        let result = match self.provider {
            ProviderKind::Unconfigured => Err(anyhow!(
                "no provider connected; run /connect to get started"
            )),
            ProviderKind::OpenAi | ProviderKind::Custom => {
                self.stream_openai(request_id, &messages, tooling, &events)
                    .await
            }
            ProviderKind::Ollama => self.stream_ollama(request_id, &messages, &events).await,
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
        events: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<()> {
        let mut request = serde_json::json!({
            "model": self.model,
            "messages": request_messages(messages, tooling)?,
            "stream": true,
        });
        if tooling {
            request["tools"] = tools::definitions();
        }
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
        let response = builder
            .send()
            .await
            .context("connecting to OpenAI-compatible API")?;
        let response = check_response(response, "OpenAI-compatible API").await?;
        let mut state = OpenAiState::default();
        consume_lines(response, |line| {
            let event = parse_openai_line(line, &mut state)?;
            send_parsed(request_id, event, events)
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
        events: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<()> {
        let request = serde_json::json!({
            "model": self.model,
            "messages": request_messages(messages, false)?,
            "stream": true,
        });
        let response = self
            .client
            .post(format!("{}/api/chat", self.base_url))
            .json(&request)
            .send()
            .await
            .context("connecting to Ollama; is `ollama serve` running?")?;
        let response = check_response(response, "Ollama").await?;
        consume_lines(response, |line| {
            let event = parse_ollama_line(line)?;
            send_parsed(request_id, event, events)
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

fn request_messages(messages: &[Message], tooling: bool) -> Result<Vec<serde_json::Value>> {
    let mut request = Vec::with_capacity(messages.len() + 1);
    request.push(serde_json::json!({
        "role": "system",
        "content": tools::system_prompt(tooling),
    }));
    for message in messages {
        if message.role == Role::Tool && !tooling {
            request.push(serde_json::json!({
                "role": "user",
                "content": format!("Tool result:\n{}", message.content),
            }));
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
        request.push(value);
    }
    Ok(request)
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

async fn consume_lines<F>(response: Response, mut on_line: F) -> Result<()>
where
    F: FnMut(&str) -> Result<bool>,
{
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    while let Some(chunk) = stream.next().await {
        buffer.extend_from_slice(&chunk?);
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
enum ParsedLine {
    Ignore,
    Delta(String),
    Done,
}

fn send_parsed(
    request_id: u64,
    parsed: ParsedLine,
    events: &mpsc::UnboundedSender<StreamEvent>,
) -> Result<bool> {
    match parsed {
        ParsedLine::Ignore => Ok(true),
        ParsedLine::Delta(text) if text.is_empty() => Ok(true),
        ParsedLine::Delta(text) => {
            events.send(StreamEvent::Delta { request_id, text })?;
            Ok(true)
        }
        ParsedLine::Done => Ok(false),
    }
}

#[derive(Default)]
struct OpenAiState {
    calls: BTreeMap<usize, ToolCallBuilder>,
}

impl OpenAiState {
    fn record(&mut self, calls: Vec<OpenAiToolCall>) {
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
}

#[derive(Deserialize)]
struct OpenAiChunk {
    #[serde(default)]
    choices: Vec<OpenAiChoice>,
    error: Option<OpenAiError>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    delta: OpenAiDelta,
}

#[derive(Deserialize)]
struct OpenAiDelta {
    content: Option<String>,
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

fn parse_openai_line(line: &str, state: &mut OpenAiState) -> Result<ParsedLine> {
    let line = line.trim();
    if line.is_empty() || line.starts_with(':') {
        return Ok(ParsedLine::Ignore);
    }
    let Some(data) = line.strip_prefix("data:") else {
        return Ok(ParsedLine::Ignore);
    };
    let data = data.trim();
    if data == "[DONE]" {
        return Ok(ParsedLine::Done);
    }
    let chunk: OpenAiChunk = serde_json::from_str(data).context("invalid OpenAI stream chunk")?;
    if let Some(error) = chunk.error {
        bail!("OpenAI stream error: {}", error.message);
    }
    let mut text = String::new();
    for choice in chunk.choices {
        if let Some(content) = choice.delta.content {
            text.push_str(&content);
        }
        if let Some(calls) = choice.delta.tool_calls {
            state.record(calls);
        }
    }
    Ok(if text.is_empty() {
        ParsedLine::Ignore
    } else {
        ParsedLine::Delta(text)
    })
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

fn parse_ollama_line(line: &str) -> Result<ParsedLine> {
    if line.trim().is_empty() {
        return Ok(ParsedLine::Ignore);
    }
    let chunk: OllamaChunk = serde_json::from_str(line).context("invalid Ollama stream chunk")?;
    if let Some(error) = chunk.error {
        bail!("Ollama stream error: {error}");
    }
    if chunk.done.unwrap_or(false) {
        return Ok(ParsedLine::Done);
    }
    Ok(chunk.message.map_or(ParsedLine::Ignore, |message| {
        ParsedLine::Delta(message.content)
    }))
}

#[cfg(test)]
mod tests {
    use super::{
        OpenAiState, ParsedLine, completion_url, parse_ollama_line, parse_openai_line,
        request_messages, response_error_detail,
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
            ParsedLine::Ignore
        );
        assert_eq!(
            parse_openai_line("event: message", &mut state).unwrap(),
            ParsedLine::Ignore
        );
        assert_eq!(
            parse_openai_line(
                r#"data: {"choices":[{"delta":{"content":"Hello"}}]}"#,
                &mut state
            )
            .unwrap(),
            ParsedLine::Delta("Hello".to_owned())
        );
        assert_eq!(
            parse_openai_line("data: [DONE]", &mut state).unwrap(),
            ParsedLine::Done
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

        let tooling = request_messages(&messages, true).unwrap();
        assert_eq!(tooling[0]["role"], "system");
        assert_eq!(tooling[2]["tool_calls"][0]["function"]["name"], "read_file");
        assert_eq!(tooling[3]["tool_call_id"], "call_1");

        let plain = request_messages(&messages, false).unwrap();
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
    fn parses_ollama_deltas_and_done() {
        assert_eq!(
            parse_ollama_line(r#"{"message":{"content":"Hi"},"done":false}"#).unwrap(),
            ParsedLine::Delta("Hi".to_owned())
        );
        assert_eq!(
            parse_ollama_line(r#"{"done":true}"#).unwrap(),
            ParsedLine::Done
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
}
