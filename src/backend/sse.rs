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

#[cfg(test)]
mod tests {
    use super::{OpenAiState, ParsedEvent, parse_ollama_line, parse_openai_line, progress_detail};
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
}
