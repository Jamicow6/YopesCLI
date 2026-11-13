//! Change previews: what a tool call will do, shown before and after it runs.

use std::{fs, path::Path};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use super::{ChangePreview, MAX_PREVIEW_BYTES, clip, files, parse, resolve};
use crate::{diff::FileDiff, session::ToolCall};

pub fn preview(call: &ToolCall) -> String {
    let arguments = serde_json::from_str::<Value>(&call.function.arguments).ok();
    let string = |key: &str| {
        arguments
            .as_ref()
            .and_then(|value| value.get(key))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    match call.function.name.as_str() {
        "delegate" => format!("subagent · {}", string("task")),
        "list_dir" => {
            let path = string("path");
            if path.is_empty() {
                ".".to_owned()
            } else {
                path
            }
        }
        "read_file" => string("path"),
        "multi_read" => {
            let all = arguments
                .as_ref()
                .and_then(|value| value.get("all"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let count = arguments
                .as_ref()
                .and_then(|value| value.get("paths"))
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            if all {
                format!("all files in {count} folders")
            } else {
                format!("{count} paths")
            }
        }
        "read_image" => string("path"),
        "glob" => clip(&string("pattern"), 160),
        "grep" => clip(&string("pattern"), 160),
        "write_file" => {
            let path = string("path");
            let bytes = arguments
                .as_ref()
                .and_then(|value| value.get("content"))
                .and_then(Value::as_str)
                .map_or(0, str::len);
            if path.is_empty() {
                format!("{bytes} bytes")
            } else {
                format!("{path} · {bytes} bytes")
            }
        }
        "edit_file" => string("path"),
        "multi_edit" => {
            let edits = arguments
                .as_ref()
                .and_then(|value| value.get("edits"))
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            format!("{edits} edits")
        }
        "run_command" => clip(&string("command"), 160),
        "bash_background" => clip(&string("command"), 160),
        "bash_output" | "bash_kill" => {
            let id = arguments
                .as_ref()
                .and_then(|value| value.get("job_id"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            format!("job {id}")
        }
        "web_search" => clip(&string("query"), 160),
        "web_fetch" => clip(&string("url"), 160),
        "todo_write" => {
            let items = arguments
                .as_ref()
                .and_then(|value| value.get("items"))
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            format!("{items} items")
        }
        _ => clip(call.function.arguments.trim(), 160),
    }
}

/// Whether the file a write or edit targets already exists on disk.
pub fn target_existed(call: &ToolCall) -> bool {
    let arguments = match serde_json::from_str::<Value>(&call.function.arguments) {
        Ok(arguments) => arguments,
        Err(_) => return false,
    };
    arguments
        .get("path")
        .and_then(Value::as_str)
        .is_some_and(|path| resolve(path).exists())
}

pub fn command_text(call: &ToolCall) -> Option<String> {
    if !matches!(
        call.function.name.as_str(),
        "run_command" | "bash_background"
    ) {
        return None;
    }
    serde_json::from_str::<Value>(&call.function.arguments)
        .ok()?
        .get("command")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// The change a successful write or edit made, reconstructed from what is on
/// disk after the tool ran.
///
/// `before` and `existed` were captured before the call ran; when the
/// previous contents are unknown the edit is rebuilt by reversing the
/// arguments instead.
pub fn applied_change(call: &ToolCall, before: Option<String>, existed: bool) -> Option<FileDiff> {
    match call.function.name.as_str() {
        "write_file" => {
            let args: files::WriteFileArgs = parse(&call.function.arguments).ok()?;
            let before = before.unwrap_or_default();
            Some(FileDiff::new(args.path, &before, &args.content, !existed))
        }
        "edit_file" => {
            let args: files::EditFileArgs = parse(&call.function.arguments).ok()?;
            if args.old_text.is_empty() {
                return None;
            }
            let after = fs::read_to_string(resolve(&args.path)).ok()?;
            let before = before.or_else(|| {
                files::apply_text_edit(&after, &args.new_text, &args.old_text, args.replace_all)
                    .map(|(text, _)| text)
            })?;
            if before == after {
                return None;
            }
            Some(FileDiff::new(args.path, &before, &after, false))
        }
        _ => None,
    }
}

/// Every change a finished write, edit or multi_edit made, from `targets` and
/// what is on disk after the call. Multi-edit diffs reverse the arguments on
/// the result, matching one file at a time.
pub fn applied_change_all(
    call: &ToolCall,
    before: Option<String>,
    existed: bool,
    targets: &[(String, bool)],
) -> Vec<FileDiff> {
    if call.function.name == "multi_edit" {
        let arguments = call.function.arguments.clone();
        return files::multi_edit_applied(&arguments, targets);
    }
    applied_change(call, before, existed).into_iter().collect()
}

pub(super) fn write_file_preview(arguments: &str) -> ChangePreview {
    let args: files::WriteFileArgs = match parse(arguments) {
        Ok(args) => args,
        Err(error) => return preview_error(error),
    };
    let files::WriteFileArgs { path, content } = args;
    let resolved = resolve(&path);
    let exists = resolved.exists();
    let before = match read_preview(&resolved, &path) {
        Ok(before) => before,
        Err(error) => return preview_error(error),
    };
    ChangePreview {
        diffs: vec![FileDiff::new(path, &before, &content, !exists)],
        before: Some(before),
        note: None,
    }
}

pub(super) fn edit_file_preview(arguments: &str) -> ChangePreview {
    let args: files::EditFileArgs = match parse(arguments) {
        Ok(args) => args,
        Err(error) => return preview_error(error),
    };
    let files::EditFileArgs {
        path,
        old_text,
        new_text,
        replace_all,
    } = args;
    let resolved = resolve(&path);
    let before = match read_preview(&resolved, &path) {
        Ok(before) => before,
        Err(error) => return preview_error(error),
    };
    let attempted = if old_text.is_empty() {
        None
    } else {
        files::apply_text_edit(&before, &old_text, &new_text, replace_all)
    };
    let matches = attempted.as_ref().map_or(0, |(_, count)| *count);
    let note = if old_text.is_empty() {
        Some("old_text cannot be empty; the edit will fail.".to_owned())
    } else if attempted.is_none() {
        Some("old_text was not found; the edit will fail.".to_owned())
    } else if matches > 1 && !replace_all {
        Some(format!(
            "old_text matches {matches} times; add more context or set replace_all."
        ))
    } else {
        None
    };
    let will_apply = attempted.is_some() && (replace_all || matches == 1);
    let after = match attempted {
        Some((after, _)) if will_apply => after,
        _ => before.clone(),
    };
    ChangePreview {
        diffs: vec![FileDiff::new(path, &before, &after, false)],
        before: Some(before),
        note,
    }
}

pub(super) fn preview_error(error: anyhow::Error) -> ChangePreview {
    ChangePreview {
        diffs: vec![],
        before: None,
        note: Some(format!("Could not build a diff: {error:#}")),
    }
}

fn read_preview(path: &Path, display: &str) -> Result<String> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(error) => return Err(error).with_context(|| format!("reading {display}")),
    };
    if !metadata.is_file() {
        bail!("{display} is not a file");
    }
    if metadata.len() > MAX_PREVIEW_BYTES {
        bail!(
            "{display} is too large to preview ({} bytes; the limit is {MAX_PREVIEW_BYTES})",
            metadata.len()
        );
    }
    fs::read_to_string(path).with_context(|| format!("reading {display}"))
}
