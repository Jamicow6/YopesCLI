//! SSE parsing: turning a provider's streamed lines into `StreamEvent`s.

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use reqwest::Response;
use serde::Deserialize;
use tokio::sync::mpsc;

use super::StreamEvent;
use crate::session::ToolCall;

/// How long a stream may go silent before we stop waiting on it.
///
/// Long reasoning can pause for a while between chunks, so this is generous;
/// without it a dropped connection stalls the response with no way to finish.
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(180);

pub(super) async fn consume_lines<F>(response: Response, mut on_line: F) -> Result<()>
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
pub(super) enum ParsedEvent {
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

pub(super) fn send_parsed(
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
pub(super) struct OpenAiState {
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

    pub(super) fn record_tool_start(
        &mut self,
        index: usize,
        id: String,
        name: String,
    ) -> Vec<ParsedEvent> {
        let builder = self.calls.entry(index).or_default();
        if !id.is_empty() {
            builder.id = id;
        }
        if !name.is_empty() {
            builder.name = name;
        }
        self.progress()
    }

    pub(super) fn record_tool_delta(
        &mut self,
        index: usize,
        partial_json: &str,
    ) -> Vec<ParsedEvent> {
        let builder = self.calls.entry(index).or_default();
        builder.arguments.push_str(partial_json);
        self.progress()
    }

    pub(super) fn record_full_tool_call(
        &mut self,
        name: String,
        arguments: String,
    ) -> Vec<ParsedEvent> {
        let index = self.calls.len();
        let builder = self.calls.entry(index).or_default();
        builder.name = name;
        builder.arguments = arguments;
        self.progress()
    }

    pub(super) fn finish(self, request_id: u64) -> Vec<ToolCall> {
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

pub(super) fn parse_openai_line(
    line: &str,
    state: &mut OpenAiState,
) -> Result<(Vec<ParsedEvent>, bool)> {
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

pub(super) fn parse_ollama_line(line: &str) -> Result<(Vec<ParsedEvent>, bool)> {
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

#[derive(Deserialize)]
struct AnthropicChunk {
    #[serde(rename = "type")]
    kind: String,
    index: Option<usize>,
    message: Option<AnthropicMessage>,
    content_block: Option<AnthropicContentBlock>,
    delta: Option<AnthropicDelta>,
    usage: Option<AnthropicUsage>,
    error: Option<AnthropicError>,
}

#[derive(Deserialize)]
struct AnthropicMessage {
    usage: Option<AnthropicUsage>,
}

#[derive(Deserialize)]
struct AnthropicUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct AnthropicContentBlock {
    #[serde(rename = "type")]
    kind: String,
    id: Option<String>,
    name: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicDelta {
    #[serde(rename = "type")]
    kind: Option<String>,
    text: Option<String>,
    thinking: Option<String>,
    partial_json: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicError {
    message: String,
}

pub(super) fn parse_anthropic_line(
    line: &str,
    state: &mut OpenAiState,
) -> Result<(Vec<ParsedEvent>, bool)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with(':') || line.starts_with("event:") {
        return Ok((Vec::new(), false));
    }
    let Some(data) = line.strip_prefix("data:") else {
        return Ok((Vec::new(), false));
    };
    let data = data.trim();
    if data == "[DONE]" {
        return Ok((Vec::new(), true));
    }
    let chunk: AnthropicChunk =
        serde_json::from_str(data).context("invalid Anthropic stream chunk")?;
    if let Some(err) = chunk.error {
        bail!("Anthropic stream error: {}", err.message);
    }
    let mut events = Vec::new();

    if let Some(usage) = chunk.message.and_then(|m| m.usage) {
        if let Some(in_tokens) = usage.input_tokens.filter(|t| *t > 0) {
            events.push(ParsedEvent::Usage {
                prompt_tokens: in_tokens,
                completion_tokens: 0,
            });
        }
    }
    if let Some(usage) = chunk.usage {
        if let Some(out_tokens) = usage.output_tokens.filter(|t| *t > 0) {
            events.push(ParsedEvent::Usage {
                prompt_tokens: 0,
                completion_tokens: out_tokens,
            });
        }
    }

    match chunk.kind.as_str() {
        "content_block_start" => {
            if let Some(block) = chunk.content_block {
                if block.kind == "tool_use" {
                    let index = chunk.index.unwrap_or(0);
                    let id = block.id.unwrap_or_default();
                    let name = block.name.unwrap_or_default();
                    events.extend(state.record_tool_start(index, id, name));
                }
            }
        }
        "content_block_delta" => {
            if let Some(delta) = chunk.delta {
                match delta.kind.as_deref() {
                    Some("text_delta") => {
                        if let Some(text) = delta.text.filter(|t| !t.is_empty()) {
                            events.push(ParsedEvent::Delta(text));
                        }
                    }
                    Some("thinking_delta") => {
                        if let Some(thinking) = delta.thinking.filter(|t| !t.is_empty()) {
                            events.push(ParsedEvent::Reasoning(thinking));
                        }
                    }
                    Some("input_json_delta") => {
                        if let Some(partial) = delta.partial_json {
                            let index = chunk.index.unwrap_or(0);
                            events.extend(state.record_tool_delta(index, &partial));
                        }
                    }
                    _ => {
                        if let Some(text) = delta.text.filter(|t| !t.is_empty()) {
                            events.push(ParsedEvent::Delta(text));
                        } else if let Some(thinking) = delta.thinking.filter(|t| !t.is_empty()) {
                            events.push(ParsedEvent::Reasoning(thinking));
                        }
                    }
                }
            }
        }
        "message_stop" => {
            return Ok((events, true));
        }
        _ => {}
    }

    Ok((events, false))
}

#[derive(Deserialize)]
struct GeminiChunk {
    candidates: Option<Vec<GeminiCandidate>>,
    #[serde(rename = "usageMetadata")]
    usage_metadata: Option<GeminiUsage>,
    error: Option<GeminiError>,
}

#[derive(Deserialize)]
struct GeminiCandidate {
    content: Option<GeminiContent>,
    #[serde(rename = "finishReason")]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct GeminiContent {
    parts: Option<Vec<GeminiPart>>,
}

#[derive(Deserialize)]
struct GeminiPart {
    text: Option<String>,
    thought: Option<bool>,
    #[serde(rename = "functionCall")]
    function_call: Option<GeminiFunctionCall>,
}

#[derive(Deserialize)]
struct GeminiFunctionCall {
    name: String,
    args: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct GeminiUsage {
    #[serde(rename = "promptTokenCount")]
    prompt_token_count: Option<u64>,
    #[serde(rename = "candidatesTokenCount")]
    candidates_token_count: Option<u64>,
}

#[derive(Deserialize)]
struct GeminiError {
    message: String,
}

pub(super) fn parse_gemini_line(
    line: &str,
    state: &mut OpenAiState,
) -> Result<(Vec<ParsedEvent>, bool)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with(':') || line.starts_with("event:") {
        return Ok((Vec::new(), false));
    }
    let Some(data) = line.strip_prefix("data:") else {
        return Ok((Vec::new(), false));
    };
    let data = data.trim();
    if data == "[DONE]" {
        return Ok((Vec::new(), true));
    }
    let chunk: GeminiChunk = serde_json::from_str(data).context("invalid Gemini stream chunk")?;
    if let Some(err) = chunk.error {
        bail!("Gemini stream error: {}", err.message);
    }
    let mut events = Vec::new();
    let mut done = false;

    if let Some(usage) = chunk.usage_metadata {
        let prompt_tokens = usage.prompt_token_count.unwrap_or(0);
        let completion_tokens = usage.candidates_token_count.unwrap_or(0);
        if prompt_tokens > 0 || completion_tokens > 0 {
            events.push(ParsedEvent::Usage {
                prompt_tokens,
                completion_tokens,
            });
        }
    }

    if let Some(candidates) = chunk.candidates {
        for candidate in candidates {
            if let Some(content) = candidate.content {
                if let Some(parts) = content.parts {
                    for part in parts {
                        if part.thought == Some(true) {
                            if let Some(text) = part.text.filter(|t| !t.is_empty()) {
                                events.push(ParsedEvent::Reasoning(text));
                            }
                        } else if let Some(text) = part.text.filter(|t| !t.is_empty()) {
                            events.push(ParsedEvent::Delta(text));
                        }
                        if let Some(call) = part.function_call {
                            let args_str = match call.args {
                                Some(serde_json::Value::String(s)) => s,
                                Some(v) => {
                                    serde_json::to_string(&v).unwrap_or_else(|_| "{}".to_owned())
                                }
                                None => "{}".to_owned(),
                            };
                            events.extend(state.record_full_tool_call(call.name, args_str));
                        }
                    }
                }
            }
            if candidate.finish_reason.is_some() {
                done = true;
            }
        }
    }

    Ok((events, done))
}

#[cfg(test)]
mod tests {
    use super::{
        OpenAiState, ParsedEvent, parse_anthropic_line, parse_gemini_line, parse_ollama_line,
        parse_openai_line, progress_detail,
    };
    use crate::backend::retry::response_error_detail;

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
        assert!(!done);
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
        assert!(!done);
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
            r#"data: {"choices":[{"delta":{"reasoning_content":"Let me think...","content":"So the"}}],"usage":{"prompt_tokens":3,"completion_tokens":9}}"#,
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
        let big = format!(r#"{{"path":"test.html","content":"{}"}}"#, "x".repeat(2048));
        let chunk = serde_json::json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": { "arguments": big }
                    }]
                }
            }]
        });
        let (events, _) = parse_openai_line(&format!("data: {chunk}"), &mut state).unwrap();
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

    #[test]
    fn parses_anthropic_stream_events() {
        let mut state = OpenAiState::default();
        // ping / message_start
        assert_eq!(
            parse_anthropic_line("event: message_start", &mut state).unwrap(),
            (Vec::new(), false)
        );
        let (events, done) = parse_anthropic_line(
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":15}}}"#,
            &mut state,
        )
        .unwrap();
        assert!(!done);
        assert_eq!(
            events,
            vec![ParsedEvent::Usage {
                prompt_tokens: 15,
                completion_tokens: 0
            }]
        );

        // text delta
        let (events, _) = parse_anthropic_line(
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello world"}}"#,
            &mut state,
        )
        .unwrap();
        assert_eq!(events, vec![ParsedEvent::Delta("Hello world".to_owned())]);

        // thinking delta
        let (events, _) = parse_anthropic_line(
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Hmm..."}}"#,
            &mut state,
        )
        .unwrap();
        assert_eq!(events, vec![ParsedEvent::Reasoning("Hmm...".to_owned())]);

        // tool use start & input delta
        parse_anthropic_line(
            r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_01","name":"read_file"}}"#,
            &mut state,
        )
        .unwrap();
        parse_anthropic_line(
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"Cargo.toml\"}"}}"#,
            &mut state,
        )
        .unwrap();

        // message_stop
        let (_, done) =
            parse_anthropic_line(r#"data: {"type":"message_stop"}"#, &mut state).unwrap();
        assert!(done);

        let calls = state.finish(1);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "toolu_01");
        assert_eq!(calls[0].function.name, "read_file");
        assert_eq!(calls[0].function.arguments, "{\"path\":\"Cargo.toml\"}");
    }

    #[test]
    fn parses_gemini_stream_events() {
        let mut state = OpenAiState::default();
        let (events, done) = parse_gemini_line(
            r#"data: {"candidates":[{"content":{"parts":[{"text":"Gemini reply"}],"role":"model"}}],"usageMetadata":{"promptTokenCount":12,"candidatesTokenCount":6}}"#,
            &mut state,
        )
        .unwrap();
        assert!(!done);
        assert_eq!(
            events,
            vec![
                ParsedEvent::Usage {
                    prompt_tokens: 12,
                    completion_tokens: 6
                },
                ParsedEvent::Delta("Gemini reply".to_owned())
            ]
        );

        // thinking / thought
        let (events, _) = parse_gemini_line(
            r#"data: {"candidates":[{"content":{"parts":[{"thought":true,"text":"Thinking..."}],"role":"model"}}]}"#,
            &mut state,
        )
        .unwrap();
        assert_eq!(
            events,
            vec![ParsedEvent::Reasoning("Thinking...".to_owned())]
        );

        // functionCall
        parse_gemini_line(
            r#"data: {"candidates":[{"content":{"parts":[{"functionCall":{"name":"list_dir","args":{"path":"."}}}],"role":"model"},"finishReason":"STOP"}]}"#,
            &mut state,
        )
        .unwrap();

        let calls = state.finish(2);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "list_dir");
        assert!(calls[0].function.arguments.contains(r#""path":".""#));
    }
}
