use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use reqwest::{Client, Response};
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::{
    config::{Config, ProviderKind},
    session::Message,
};

#[derive(Debug)]
pub enum StreamEvent {
    Delta { request_id: u64, text: String },
    Done { request_id: u64 },
    Error { request_id: u64, message: String },
}

#[derive(Clone)]
pub struct Backend {
    provider: ProviderKind,
    provider_label: String,
    model: String,
    base_url: String,
    api_key: Option<String>,
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
            client,
        })
    }

    pub fn provider_label(&self) -> &str {
        &self.provider_label
    }

    pub fn model(&self) -> &str {
        &self.model
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
    ) {
        self.provider = provider;
        self.provider_label = provider_label;
        self.model = model;
        self.base_url = base_url.trim_end_matches('/').to_owned();
        self.api_key = api_key;
    }

    pub async fn stream(
        self,
        request_id: u64,
        messages: Vec<Message>,
        events: mpsc::UnboundedSender<StreamEvent>,
    ) {
        let result = match self.provider {
            ProviderKind::Demo => stream_demo(request_id, &messages, &events).await,
            ProviderKind::OpenAi => self.stream_openai(request_id, &messages, &events).await,
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
        events: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<()> {
        let request = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "stream": true,
        });
        let mut builder = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .json(&request);
        if let Some(api_key) = &self.api_key {
            builder = builder.bearer_auth(api_key);
        }
        let response = builder
            .send()
            .await
            .context("connecting to OpenAI-compatible API")?;
        let response = check_response(response, "OpenAI-compatible API").await?;
        consume_lines(response, |line| {
            let event = parse_openai_line(line)?;
            send_parsed(request_id, event, events)
        })
        .await
    }

    async fn stream_ollama(
        &self,
        request_id: u64,
        messages: &[Message],
        events: &mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<()> {
        let request = serde_json::json!({
            "model": self.model,
            "messages": messages,
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

async fn stream_demo(
    request_id: u64,
    messages: &[Message],
    events: &mpsc::UnboundedSender<StreamEvent>,
) -> Result<()> {
    let prompt = messages
        .last()
        .map_or("Hello", |message| message.content.as_str());
    let prompt = prompt.chars().take(800).collect::<String>();
    let response = format!(
        "Demo stream connected.\n\nYour prompt: {prompt}\n\nThis is a local simulated response. Run `yopes --provider openai` with OPENAI_API_KEY, or `yopes --provider ollama` for real model output."
    );
    for chunk in response.split_inclusive(char::is_whitespace) {
        if events
            .send(StreamEvent::Delta {
                request_id,
                text: chunk.to_owned(),
            })
            .is_err()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(12)).await;
    }
    Ok(())
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
}

#[derive(Deserialize)]
struct OpenAiError {
    message: String,
}

fn parse_openai_line(line: &str) -> Result<ParsedLine> {
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
    let text = chunk
        .choices
        .into_iter()
        .filter_map(|choice| choice.delta.content)
        .collect::<String>();
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
    use super::{ParsedLine, parse_ollama_line, parse_openai_line, response_error_detail};

    #[test]
    fn parses_openai_deltas_and_done() {
        assert_eq!(
            parse_openai_line(": keep-alive").unwrap(),
            ParsedLine::Ignore
        );
        assert_eq!(
            parse_openai_line("event: message").unwrap(),
            ParsedLine::Ignore
        );
        assert_eq!(
            parse_openai_line(r#"data: {"choices":[{"delta":{"content":"Hello"}}]}"#).unwrap(),
            ParsedLine::Delta("Hello".to_owned())
        );
        assert_eq!(parse_openai_line("data: [DONE]").unwrap(), ParsedLine::Done);
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
        assert!(parse_openai_line(r#"data: {"error":{"message":"bad key"}}"#).is_err());
        assert!(parse_ollama_line(r#"{"error":"model missing"}"#).is_err());
        assert_eq!(
            response_error_detail(r#"{"error":{"message":"nope"}}"#),
            "nope"
        );
    }
}
