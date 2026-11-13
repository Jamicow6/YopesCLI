//! Request-body building: how conversation history, images, system prompts
//! and skills are shaped into the messages array a provider expects.

use anyhow::Result;

use crate::session::{Message, Role};
use crate::{skills, tools};

pub(super) fn request_messages(
    messages: &[Message],
    tooling: bool,
    image_style: ImageStyle,
    custom_prompt: Option<&str>,
    mode_note: Option<&'static str>,
) -> Result<Vec<serde_json::Value>> {
    let mut request = Vec::with_capacity(messages.len() + 1);
    // Images returned by tools are sent after the tool messages, so every
    // tool_call id still gets its own tool message first.
    let mut tool_images: Vec<&crate::session::ImageAttachment> = Vec::new();
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
        if let Some(listing) = skills::section(&std::env::current_dir().unwrap_or_default()) {
            system.push_str("\n\n");
            system.push_str(&listing);
        }
    }
    request.push(serde_json::json!({
        "role": "system",
        "content": system,
    }));
    for message in messages {
        // Parts and Ollama branches below carry the images themselves.
        let openai_parts = image_style == ImageStyle::Parts;
        // A provider failure is transcript decoration, not conversation:
        // sending it back upstream would look like an assistant turn.
        if message.error.is_some() {
            continue;
        }
        if message.role == Role::Tool && !tooling {
            if image_style == ImageStyle::Ollama && !message.images.is_empty() {
                let images = message
                    .images
                    .iter()
                    .map(|image| serde_json::Value::String(image.data.clone()))
                    .collect::<Vec<_>>();
                request.push(serde_json::json!({
                    "role": "user",
                    "content": format!("Tool result:\n{}", message.content),
                    "images": images,
                }));
            } else if image_style == ImageStyle::Parts && !message.images.is_empty() {
                let mut parts = vec![serde_json::json!({
                    "type": "text",
                    "text": format!("Tool result:\n{}", message.content),
                })];
                for image in &message.images {
                    parts.push(serde_json::json!({
                        "type": "image_url",
                        "image_url": { "url": format!("data:image/png;base64,{}", image.data) }
                    }));
                }
                request.push(serde_json::json!({ "role": "user", "content": parts }));
            } else {
                request.push(serde_json::json!({
                    "role": "user",
                    "content": format!("Tool result:\n{}", message.content),
                }));
            }
            continue;
        }
        // A tool message stays text; its images travel in a user message
        // appended after all tool results, which keeps the tool-call pairing
        // valid for every provider.
        if message.role == Role::Tool && openai_parts && !message.images.is_empty() {
            let mut value = serde_json::to_value(message)?;
            if let Some(object) = value.as_object_mut() {
                object.remove("images");
            }
            request.push(value);
            tool_images.extend(message.images.iter());
            continue;
        }
        // Attached screenshots travel so a vision-capable model sees them:
        // multimodal parts for OpenAI-compatible providers, Ollama's own
        // `images` array there.
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
    if !tool_images.is_empty() {
        let mut parts = vec![serde_json::json!({
            "type": "text",
            "text": "Images read by the tools:",
        })];
        for image in tool_images {
            parts.push(serde_json::json!({
                "type": "image_url",
                "image_url": { "url": format!("data:image/png;base64,{}", image.data) }
            }));
        }
        request.push(serde_json::json!({ "role": "user", "content": parts }));
    }
    Ok(request)
}

/// How images ride along in request messages: OpenAI-compatible multimodal
/// text parts, Ollama's `images` base64 array, or not at all.
#[derive(Clone, Copy, Default, Debug, Eq, PartialEq)]
pub(super) enum ImageStyle {
    #[default]
    None,
    Parts,
    Ollama,
}

/// Shapes the conversation history into Anthropic's Messages payload:
/// a top-level system string and alternating user/assistant turns.
///
/// ponytail: complex tool result caching controls are omitted;
/// add when prompt caching is explicitly configured.
pub(super) fn anthropic_request_payload(
    messages: &[Message],
    tooling: bool,
    custom_prompt: Option<&str>,
    mode_note: Option<&'static str>,
) -> Result<(Option<String>, Vec<serde_json::Value>)> {
    let mut system = tools::system_prompt(tooling);
    if let Some(custom) = custom_prompt.filter(|custom| !custom.trim().is_empty()) {
        system.push_str("\n\n");
        system.push_str(custom.trim());
    }
    if let Some(note) = mode_note {
        system.push_str("\n\n");
        system.push_str(note);
    }
    if tooling {
        if let Some(listing) = skills::section(&std::env::current_dir().unwrap_or_default()) {
            system.push_str("\n\n");
            system.push_str(&listing);
        }
    }

    let mut anthropic_messages: Vec<serde_json::Value> = Vec::new();
    for message in messages {
        if message.error.is_some() {
            continue;
        }
        match message.role {
            Role::User => {
                let mut content = Vec::new();
                for image in &message.images {
                    content.push(serde_json::json!({
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/png",
                            "data": image.data,
                        }
                    }));
                }
                if !message.content.is_empty() {
                    content.push(serde_json::json!({
                        "type": "text",
                        "text": message.content,
                    }));
                }
                if content.is_empty() {
                    continue;
                }
                if let Some(last) = anthropic_messages
                    .last_mut()
                    .filter(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
                {
                    if let Some(arr) = last.get_mut("content").and_then(|c| c.as_array_mut()) {
                        arr.extend(content);
                        continue;
                    }
                }
                anthropic_messages.push(serde_json::json!({
                    "role": "user",
                    "content": content,
                }));
            }
            Role::Assistant => {
                let mut content = Vec::new();
                if !message.content.is_empty() {
                    content.push(serde_json::json!({
                        "type": "text",
                        "text": message.content,
                    }));
                }
                if tooling {
                    if let Some(calls) = &message.tool_calls {
                        for call in calls {
                            let input: serde_json::Value =
                                serde_json::from_str(&call.function.arguments)
                                    .unwrap_or_else(|_| serde_json::json!({}));
                            content.push(serde_json::json!({
                                "type": "tool_use",
                                "id": call.id,
                                "name": call.function.name,
                                "input": input,
                            }));
                        }
                    }
                }
                if content.is_empty() {
                    continue;
                }
                anthropic_messages.push(serde_json::json!({
                    "role": "assistant",
                    "content": content,
                }));
            }
            Role::Tool => {
                let call_id = message.tool_call_id.as_deref().unwrap_or_default();
                let tool_result_block = serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": call_id,
                    "content": message.content,
                });
                if let Some(last) = anthropic_messages
                    .last_mut()
                    .filter(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
                {
                    if let Some(arr) = last.get_mut("content").and_then(|c| c.as_array_mut()) {
                        arr.push(tool_result_block);
                        continue;
                    }
                }
                anthropic_messages.push(serde_json::json!({
                    "role": "user",
                    "content": [tool_result_block],
                }));
            }
        }
    }

    if anthropic_messages
        .first()
        .and_then(|m| m.get("role"))
        .and_then(|r| r.as_str())
        == Some("assistant")
    {
        anthropic_messages.insert(
            0,
            serde_json::json!({
                "role": "user",
                "content": [{"type": "text", "text": "Begin."}],
            }),
        );
    }

    let system_opt = if system.trim().is_empty() {
        None
    } else {
        Some(system)
    };
    Ok((system_opt, anthropic_messages))
}

pub(super) fn anthropic_tools() -> serde_json::Value {
    let defs = tools::definitions();
    let tools = defs
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|t| {
                    let f = t.get("function")?;
                    let name = f.get("name")?.as_str()?;
                    let desc = f
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or_default();
                    let schema = f
                        .get("parameters")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({"type": "object"}));
                    Some(serde_json::json!({
                        "name": name,
                        "description": desc,
                        "input_schema": schema,
                    }))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    serde_json::Value::Array(tools)
}

/// Shapes the conversation history into Gemini's generateContent payload:
/// systemInstruction and contents with alternating user/model roles.
///
/// ponytail: safety settings and generationConfig overrides are skipped;
/// add when temperature or safety threshold customization is needed.
pub(super) fn gemini_request_payload(
    messages: &[Message],
    tooling: bool,
    custom_prompt: Option<&str>,
    mode_note: Option<&'static str>,
) -> Result<(Option<serde_json::Value>, Vec<serde_json::Value>)> {
    let mut system = tools::system_prompt(tooling);
    if let Some(custom) = custom_prompt.filter(|custom| !custom.trim().is_empty()) {
        system.push_str("\n\n");
        system.push_str(custom.trim());
    }
    if let Some(note) = mode_note {
        system.push_str("\n\n");
        system.push_str(note);
    }
    if tooling {
        if let Some(listing) = skills::section(&std::env::current_dir().unwrap_or_default()) {
            system.push_str("\n\n");
            system.push_str(&listing);
        }
    }

    let mut call_names: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for msg in messages {
        if let Some(calls) = &msg.tool_calls {
            for call in calls {
                call_names.insert(call.id.clone(), call.function.name.clone());
            }
        }
    }

    let mut contents: Vec<serde_json::Value> = Vec::new();
    for message in messages {
        if message.error.is_some() {
            continue;
        }
        match message.role {
            Role::User => {
                let mut parts = Vec::new();
                for image in &message.images {
                    parts.push(serde_json::json!({
                        "inlineData": {
                            "mimeType": "image/png",
                            "data": image.data,
                        }
                    }));
                }
                if !message.content.is_empty() {
                    parts.push(serde_json::json!({ "text": message.content }));
                }
                if parts.is_empty() {
                    continue;
                }
                if let Some(last) = contents
                    .last_mut()
                    .filter(|c| c.get("role").and_then(|r| r.as_str()) == Some("user"))
                {
                    if let Some(arr) = last.get_mut("parts").and_then(|p| p.as_array_mut()) {
                        arr.extend(parts);
                        continue;
                    }
                }
                contents.push(serde_json::json!({
                    "role": "user",
                    "parts": parts,
                }));
            }
            Role::Assistant => {
                let mut parts = Vec::new();
                if !message.content.is_empty() {
                    parts.push(serde_json::json!({ "text": message.content }));
                }
                if tooling {
                    if let Some(calls) = &message.tool_calls {
                        for call in calls {
                            let args: serde_json::Value =
                                serde_json::from_str(&call.function.arguments)
                                    .unwrap_or_else(|_| serde_json::json!({}));
                            parts.push(serde_json::json!({
                                "functionCall": {
                                    "name": call.function.name,
                                    "args": args,
                                }
                            }));
                        }
                    }
                }
                if parts.is_empty() {
                    continue;
                }
                if let Some(last) = contents
                    .last_mut()
                    .filter(|c| c.get("role").and_then(|r| r.as_str()) == Some("model"))
                {
                    if let Some(arr) = last.get_mut("parts").and_then(|p| p.as_array_mut()) {
                        arr.extend(parts);
                        continue;
                    }
                }
                contents.push(serde_json::json!({
                    "role": "model",
                    "parts": parts,
                }));
            }
            Role::Tool => {
                let call_id = message.tool_call_id.as_deref().unwrap_or_default();
                let func_name = call_names
                    .get(call_id)
                    .map(|s| s.as_str())
                    .unwrap_or("tool");
                let part = serde_json::json!({
                    "functionResponse": {
                        "name": func_name,
                        "response": {
                            "name": func_name,
                            "content": message.content,
                        }
                    }
                });
                if let Some(last) = contents
                    .last_mut()
                    .filter(|c| c.get("role").and_then(|r| r.as_str()) == Some("user"))
                {
                    if let Some(arr) = last.get_mut("parts").and_then(|p| p.as_array_mut()) {
                        arr.push(part);
                        continue;
                    }
                }
                contents.push(serde_json::json!({
                    "role": "user",
                    "parts": [part],
                }));
            }
        }
    }

    let system_instruction = if system.trim().is_empty() {
        None
    } else {
        Some(serde_json::json!({
            "parts": [{ "text": system }]
        }))
    };
    Ok((system_instruction, contents))
}

pub(super) fn gemini_tools() -> serde_json::Value {
    let defs = tools::definitions();
    let decls = defs
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|t| {
                    let f = t.get("function")?;
                    let name = f.get("name")?.as_str()?;
                    let desc = f
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or_default();
                    let schema = f
                        .get("parameters")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({"type": "object"}));
                    Some(serde_json::json!({
                        "name": name,
                        "description": desc,
                        "parameters": schema,
                    }))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    serde_json::json!([{ "functionDeclarations": decls }])
}

#[cfg(test)]
mod tests {
    use super::request_messages;
    use crate::backend::ImageStyle;
    use crate::session::{Message, Role, ToolCall};

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
            request_messages(&messages, true, ImageStyle::None, Some("  "), None).unwrap(),
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
        assert_eq!(urls[0]["url"], "data:image/png;base64,aWNvbg==");
        assert!(parts.last().unwrap().get("images").is_none());

        // Ollama wants a flat base64 array on the message itself.
        let ollama = request_messages(&messages, true, ImageStyle::Ollama, None, None).unwrap();
        let last = ollama.last().unwrap();
        assert_eq!(last["content"], "what changed?");
        assert_eq!(last["images"][0], "aWNvbg==");
        // Plain requests keep the raw attachment field out.
        let plain = request_messages(&messages, false, ImageStyle::None, None, None).unwrap();
        assert!(plain.last().unwrap().get("images").is_none());
        assert_eq!(plain.last().unwrap()["role"], "user");
    }

    #[test]
    fn anthropic_payload_shapes_tools_and_roles() {
        use super::anthropic_request_payload;

        let mut assistant = Message::new(Role::Assistant, "Checking...");
        assistant.tool_calls = Some(vec![ToolCall::new(
            "call_42",
            "read_file",
            r#"{"path":"foo.rs"}"#,
        )]);
        let messages = vec![
            Message::new(Role::User, "read foo"),
            assistant,
            Message::tool_result("call_42", "fn main() {}"),
        ];

        let (system, payload) = anthropic_request_payload(&messages, true, None, None).unwrap();
        assert!(system.is_some());
        assert_eq!(payload.len(), 3);
        assert_eq!(payload[0]["role"], "user");
        assert_eq!(payload[1]["role"], "assistant");
        assert_eq!(payload[1]["content"][1]["type"], "tool_use");
        assert_eq!(payload[1]["content"][1]["name"], "read_file");
        assert_eq!(payload[2]["role"], "user");
        assert_eq!(payload[2]["content"][0]["type"], "tool_result");
        assert_eq!(payload[2]["content"][0]["tool_use_id"], "call_42");
    }

    #[test]
    fn gemini_payload_shapes_system_instruction_and_contents() {
        use super::gemini_request_payload;

        let mut assistant = Message::new(Role::Assistant, "Running tool...");
        assistant.tool_calls = Some(vec![ToolCall::new(
            "call_99",
            "list_dir",
            r#"{"path":"."}"#,
        )]);
        let messages = vec![
            Message::new(Role::User, "list files"),
            assistant,
            Message::tool_result("call_99", "file.txt"),
        ];

        let (instruction, contents) = gemini_request_payload(&messages, true, None, None).unwrap();
        assert!(instruction.is_some());
        assert_eq!(contents.len(), 3);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(contents[1]["parts"][1]["functionCall"]["name"], "list_dir");
        assert_eq!(contents[2]["role"], "user");
        assert_eq!(
            contents[2]["parts"][0]["functionResponse"]["name"],
            "list_dir"
        );
    }
}
