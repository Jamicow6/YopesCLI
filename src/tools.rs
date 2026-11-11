use std::{
    collections::HashMap,
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{diff::FileDiff, session::ToolCall};

const MAX_LIST_ENTRIES: usize = 300;
const MAX_READ_BYTES: u64 = 4 * 1024 * 1024;
const MAX_READ_LINES: usize = 400;
const MAX_READ_LINE_LIMIT: usize = 2_000;
const MAX_WRITE_BYTES: usize = 2 * 1024 * 1024;
const MAX_PREVIEW_BYTES: u64 = 1024 * 1024;
const MAX_OUTPUT_CHARS: usize = 16_000;
const DEFAULT_TIMEOUT_SECONDS: u64 = 120;
const MAX_TIMEOUT_SECONDS: u64 = 600;

#[cfg(test)]
pub const TOOL_NAMES: [&str; 11] = [
    "list_dir",
    "read_file",
    "glob",
    "grep",
    "write_file",
    "edit_file",
    "multi_edit",
    "run_command",
    "web_search",
    "todo_write",
    "delegate",
];

#[derive(Debug)]
pub struct ChangePreview {
    pub diffs: Vec<FileDiff>,
    pub before: Option<String>,
    pub note: Option<String>,
}

/// Whether a tool call needs the user's approval.
///
/// File writes and edits never ask: their diff is shown in the transcript and
/// they are easy to undo. Only a command the model marks as risky asks.
pub fn requires_approval(call: &ToolCall) -> bool {
    if call.function.name == "delegate" {
        // A subagent request spends extra tokens on the user's account.
        return true;
    }
    call.function.name == "run_command" && command_risk(&call.function.arguments)
}

/// The `risk` flag the model set on a `run_command` call.
pub fn command_risk(arguments: &str) -> bool {
    serde_json::from_str::<Value>(arguments)
        .ok()
        .and_then(|arguments| arguments.get("risk").and_then(Value::as_bool))
        .unwrap_or(false)
}

pub fn system_prompt(tools_enabled: bool) -> String {
    let cwd = env::current_dir().map_or_else(|_| ".".to_owned(), |path| path.display().to_string());
    let os = env::consts::OS;
    if tools_enabled {
        format!(
            "You are yopes, a coding assistant running in the user's terminal.\n\
             Working directory: {cwd}\n\
             Operating system: {os}\n\
             Use the provided tools to inspect and change files and to run commands. \
             Prefer tools over guessing, read a file before editing it, and never claim \
             that a file changed unless a tool result confirms it. Set run_command risk to \
             true for anything destructive or hard to undo so the user is asked first. \
             Use glob to find files by pattern and grep to search their contents, so you \
             do not read whole directories blindly. \
             For several replacements in one or more files, batch them with multi_edit: \
             it applies every edit in order, and one failure refuses the whole batch. \
             Use web_search for anything newer than your knowledge or that needs current \
             facts, and cite the URL you used. \
             For multi-step work or when the user asks for a todo list, maintain the \
             checklist with todo_write: send the whole list each call, marking done items. \
             For an independent subtask use delegate: hand the subagent one self-contained \
             task with all the context it needs — it has no tools and returns a final answer. \
             Keep responses short and concrete. Paths may be relative to the working directory."
        )
    } else {
        format!(
            "You are yopes, a coding assistant running in the user's terminal.\n\
             Working directory: {cwd}\n\
             Operating system: {os}\n\
             Keep responses short and concrete."
        )
    }
}

pub fn definitions() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "list_dir",
                "description": "List files and directories in the workspace.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Directory path relative to the working directory. Defaults to ."
                        }
                    }
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a UTF-8 text file, optionally a range of lines. A directory path lists its entries.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "File path relative to the working directory."
                        },
                        "start_line": {
                            "type": "integer",
                            "description": "First line to read, starting at 1."
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of lines to return."
                        }
                    },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "glob",
                "description": "Find files and directories by glob pattern, relative to the working directory. Supports * (any text within one path segment), ? (one character), and ** (any number of segments). Read-only.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Glob pattern such as src/**/*.rs."
                        },
                        "path": {
                            "type": "string",
                            "description": "Directory to search under, relative to the working directory. Defaults to ."
                        }
                    },
                    "required": ["pattern"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "grep",
                "description": "Search file contents with a plain substring and return file:line:text matches. Binary files, dependency and cache directories are skipped. Read-only.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Text to search for."
                        },
                        "path": {
                            "type": "string",
                            "description": "Directory to search under, relative to the working directory. Defaults to ."
                        },
                        "include": {
                            "type": "string",
                            "description": "Glob for file types, such as *.rs or src/*/*.ts."
                        },
                        "ignore_case": {
                            "type": "boolean",
                            "description": "Match without case sensitivity. Defaults to false."
                        }
                    },
                    "required": ["pattern"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "Create or overwrite a text file. Parent directories are created.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "File path relative to the working directory."
                        },
                        "content": {
                            "type": "string",
                            "description": "Full file content to write."
                        }
                    },
                    "required": ["path", "content"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "edit_file",
                "description": "Replace exact text in an existing UTF-8 file.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "File path relative to the working directory."
                        },
                        "old_text": {
                            "type": "string",
                            "description": "Exact text to find."
                        },
                        "new_text": {
                            "type": "string",
                            "description": "Replacement text."
                        },
                        "replace_all": {
                            "type": "boolean",
                            "description": "Replace every match instead of requiring exactly one."
                        }
                    },
                    "required": ["path", "old_text", "new_text"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "multi_edit",
                "description": "Apply several exact-text edits in one call. Every edit follows the edit_file rules against one in-memory state, so a batch either applies fully or refuses without touching any file. Sequential edits to the same file see each other's results.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "edits": {
                            "type": "array",
                            "description": "The edits, in order.",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "path": {
                                        "type": "string",
                                        "description": "File path relative to the working directory."
                                    },
                                    "old_text": {
                                        "type": "string",
                                        "description": "Exact text to find."
                                    },
                                    "new_text": {
                                        "type": "string",
                                        "description": "Replacement text."
                                    },
                                    "replace_all": {
                                        "type": "boolean",
                                        "description": "Replace every match in that file instead of requiring exactly one."
                                    }
                                },
                                "required": ["path", "old_text", "new_text"]
                            }
                        }
                    },
                    "required": ["edits"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "run_command",
                "description": "Run a shell command in the workspace and return its output. Set risk to true for anything destructive or hard to undo: deleting or overwriting files, git resets or force pushes, installing or uninstalling software, stopping services, or anything reaching outside the working directory. Risky commands ask the user before they run.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "Shell command to run from the working directory."
                        },
                        "risk": {
                            "type": "boolean",
                            "description": "True when the command is destructive or could discard work, so the user is asked first."
                        },
                        "timeout_seconds": {
                            "type": "integer",
                            "description": "Maximum run time in seconds. Defaults to 120."
                        }
                    },
                    "required": ["command"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "web_search",
                "description": "Search the web and return the top results with titles, URLs and snippets. Use it for current information, documentation, release notes, or anything the user asks about that is newer than your knowledge. Cite the URL you used.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query."
                        },
                        "max_results": {
                            "type": "integer",
                            "description": "Maximum number of results. Defaults to 5."
                        }
                    },
                    "required": ["query"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "todo_write",
                "description": "Write the workspace task checklist to todo.md. Rewrite the whole list each call: every item is an object with the task text and a done flag. Use it whenever the user asks for a todo list or a multi-step plan.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "items": {
                            "type": "array",
                            "description": "The full checklist.",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "text": {
                                        "type": "string",
                                        "description": "The task."
                                    },
                                    "done": {
                                        "type": "boolean",
                                        "description": "Whether the task is finished. Defaults to false."
                                    }
                                },
                                "required": ["text"]
                            }
                        }
                    },
                    "required": ["items"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "delegate",
                "description": "Hand one self-contained subtask to a subagent running on the same provider and model, and receive its final answer. The subagent has no tools: it cannot read files, run commands, or search the web, so the task must include every file excerpt, code snippet, and fact it needs. Use it for focused drafts, second opinions, or research recaps, and split large work into several delegate calls.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "task": {
                            "type": "string",
                            "description": "Complete instructions for the subagent, including all context it needs."
                        }
                    },
                    "required": ["task"]
                }
            }
        }
    ])
}

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
        "web_search" => clip(&string("query"), 160),
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
    if call.function.name != "run_command" {
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
            let args: WriteFileArgs = parse(&call.function.arguments).ok()?;
            let before = before.unwrap_or_default();
            Some(FileDiff::new(args.path, &before, &args.content, !existed))
        }
        "edit_file" => {
            let args: EditFileArgs = parse(&call.function.arguments).ok()?;
            if args.old_text.is_empty() {
                return None;
            }
            let after = fs::read_to_string(resolve(&args.path)).ok()?;
            let before = before.or_else(|| {
                if args.replace_all {
                    Some(after.replace(&args.new_text, &args.old_text))
                } else {
                    Some(after.replacen(&args.new_text, &args.old_text, 1))
                }
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
        return multi_edit_applied(&arguments, targets);
    }
    applied_change(call, before, existed).into_iter().collect()
}

fn multi_edit_applied(arguments: &str, targets: &[(String, bool)]) -> Vec<FileDiff> {
    let args: MultiEditArgs = match parse(arguments) {
        Ok(args) => args,
        Err(_) => return Vec::new(),
    };
    let mut reversed: Vec<&MultiEditItem> = args.edits.iter().rev().collect();
    let mut diffs = Vec::new();
    for (path, existed) in targets {
        let after = match fs::read_to_string(resolve(path)) {
            Ok(after) => after,
            Err(_) => continue,
        };
        let mut before = after.clone();
        reversed.retain(|edit| {
            if edit.path == *path {
                before = if edit.replace_all {
                    before.replace(&edit.new_text, &edit.old_text)
                } else {
                    before.replacen(&edit.new_text, &edit.old_text, 1)
                };
                false
            } else {
                true
            }
        });
        if before == after {
            continue;
        }
        diffs.push(FileDiff::new(path.as_str(), &before, &after, !*existed));
    }
    diffs
}

pub fn change_preview(call: &ToolCall) -> ChangePreview {
    match call.function.name.as_str() {
        "write_file" => write_file_preview(&call.function.arguments),
        "edit_file" => edit_file_preview(&call.function.arguments),
        "multi_edit" => multi_edit_preview(&call.function.arguments),
        _ => ChangePreview {
            diffs: Vec::new(),
            before: None,
            note: None,
        },
    }
}

/// Every file a write, edit or multi_edit touches, with whether each existed
/// before the call, so per-file diffs can be recovered after the batch runs.
pub fn edit_targets(call: &ToolCall) -> Vec<(String, bool)> {
    let arguments = match serde_json::from_str::<Value>(&call.function.arguments) {
        Ok(arguments) => arguments,
        Err(_) => return Vec::new(),
    };
    let mut targets: Vec<(String, bool)> = Vec::new();
    let mut seen = |path: &str, targets: &mut Vec<(String, bool)>| {
        if path.is_empty() || targets.iter().any(|(tracked, _)| tracked == path) {
            return;
        }
        let existed = resolve(path).exists();
        targets.push((path.to_owned(), existed));
    };
    if let Some(path) = arguments.get("path").and_then(Value::as_str) {
        seen(path, &mut targets);
    }
    if let Some(edits) = arguments.get("edits").and_then(Value::as_array) {
        for edit in edits {
            if let Some(path) = edit.get("path").and_then(Value::as_str) {
                seen(path, &mut targets);
            }
        }
    }
    targets
}

fn write_file_preview(arguments: &str) -> ChangePreview {
    let args: WriteFileArgs = match parse(arguments) {
        Ok(args) => args,
        Err(error) => return preview_error(error),
    };
    let WriteFileArgs { path, content } = args;
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

fn edit_file_preview(arguments: &str) -> ChangePreview {
    let args: EditFileArgs = match parse(arguments) {
        Ok(args) => args,
        Err(error) => return preview_error(error),
    };
    let EditFileArgs {
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
    let matches = before.matches(&old_text).count();
    let note = if old_text.is_empty() {
        Some("old_text cannot be empty; the edit will fail.".to_owned())
    } else if matches == 0 {
        Some("old_text was not found; the edit will fail.".to_owned())
    } else if matches > 1 && !replace_all {
        Some(format!(
            "old_text matches {matches} times; add more context or set replace_all."
        ))
    } else {
        None
    };
    let will_apply = !old_text.is_empty() && matches > 0 && (replace_all || matches == 1);
    let after = if !will_apply {
        before.clone()
    } else if replace_all {
        before.replace(&old_text, &new_text)
    } else {
        before.replacen(&old_text, &new_text, 1)
    };
    ChangePreview {
        diffs: vec![FileDiff::new(path, &before, &after, false)],
        before: Some(before),
        note,
    }
}

fn preview_error(error: anyhow::Error) -> ChangePreview {
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

pub fn execute(call: &ToolCall, web_key: Option<&str>) -> Result<String> {
    match call.function.name.as_str() {
        "list_dir" => list_dir(&call.function.arguments),
        "read_file" => read_file(&call.function.arguments),
        "glob" => glob(&call.function.arguments),
        "grep" => grep(&call.function.arguments),
        "write_file" => write_file(&call.function.arguments),
        "edit_file" => edit_file(&call.function.arguments),
        "multi_edit" => multi_edit(&call.function.arguments),
        "run_command" => run_command(&call.function.arguments),
        "web_search" => web_search(&call.function.arguments, web_key),
        "todo_write" => todo_write(&call.function.arguments),
        // Handled by the session loop: the subagent needs the backend.
        "delegate" => bail!("delegate cannot run outside the session loop"),
        other => bail!("unknown tool `{other}`"),
    }
}

/// The `task` argument of a `delegate` call, when it is a non-empty string.
pub fn delegate_task(call: &ToolCall) -> Option<String> {
    serde_json::from_str::<Value>(&call.function.arguments)
        .ok()
        .and_then(|arguments| {
            arguments
                .get("task")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .filter(|task| !task.trim().is_empty())
        })
}

#[derive(Deserialize)]
struct ListDirArgs {
    #[serde(default)]
    path: Option<String>,
}

fn list_dir(arguments: &str) -> Result<String> {
    let args: ListDirArgs = parse(arguments)?;
    let requested = args.path.as_deref().unwrap_or(".").trim();
    let requested = if requested.is_empty() { "." } else { requested };
    directory_listing(requested)
}

/// The listing shared by `list_dir` and `read_file` on a directory: reading
/// `.` is the fastest way to see the workspace layout.
fn directory_listing(requested: &str) -> Result<String> {
    let directory = resolve(requested);
    if !directory.is_dir() {
        bail!("{requested} is not a directory");
    }
    let mut entries = fs::read_dir(&directory)
        .with_context(|| format!("listing {requested}"))?
        .filter_map(|entry| entry.ok())
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| {
        let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        (!is_dir, name)
    });

    let mut lines = Vec::new();
    for entry in entries.iter().take(MAX_LIST_ENTRIES) {
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        if is_dir {
            lines.push(format!("{name}/"));
        } else {
            let size = entry.metadata().map_or(0, |meta| meta.len());
            lines.push(format!("{name}  {size} B"));
        }
    }
    if entries.len() > MAX_LIST_ENTRIES {
        lines.push(format!(
            "… {} more entries",
            entries.len() - MAX_LIST_ENTRIES
        ));
    }
    if lines.is_empty() {
        lines.push("(empty)".to_owned());
    }
    Ok(truncate_output(format!(
        "{requested}/\n{}\n",
        lines.join("\n")
    )))
}

#[derive(Deserialize)]
struct ReadFileArgs {
    path: String,
    #[serde(default)]
    start_line: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}fn read_file(arguments: &str) -> Result<String> {
    let args: ReadFileArgs = parse(arguments)?;
    let path = resolve(&args.path);
    let metadata = fs::metadata(&path).with_context(|| format!("reading {}", args.path))?;
    if metadata.is_dir() {
        if args.start_line.is_some() || args.limit.is_some() {
            bail!("{} is a directory; line ranges only apply to files", args.path);
        }
        let requested = args.path.trim();
        return directory_listing(if requested.is_empty() { "." } else { requested });
    }
    if !metadata.is_file() {
        bail!("{} is not a file", args.path);
    }
    if metadata.len() > MAX_READ_BYTES {
        bail!(
            "{} is too large ({} bytes; the limit is {MAX_READ_BYTES})",
            args.path,
            metadata.len()
        );
    }
    let source = fs::read_to_string(&path).with_context(|| format!("reading {}", args.path))?;
    let lines = source.lines().collect::<Vec<_>>();
    let total = lines.len();
    let start = args.start_line.unwrap_or(1).max(1);
    let limit = args
        .limit
        .unwrap_or(MAX_READ_LINES)
        .clamp(1, MAX_READ_LINE_LIMIT);
    if start > total && total > 0 {
        bail!(
            "start_line {start} is past the end of {} ({total} lines)",
            args.path
        );
    }
    let end = start.saturating_sub(1).saturating_add(limit).min(total);
    let mut output = format!("{} ({total} lines)\n", args.path);
    if start > 1 || end < total {
        output.push_str(&format!("lines {start}-{end} of {total}\n"));
    }
    for line in &lines[start.saturating_sub(1)..end] {
        output.push_str(line);
        output.push('\n');
    }
    Ok(truncate_output(output))
}

#[derive(Deserialize)]
struct GlobArgs {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
}

/// Directories the walkers never enter, so caches and dependencies stay out.
const SKIP_DIRS: [&str; 5] = [".git", "target", "node_modules", "__pycache__", ".pytest_cache"];
const MAX_GLOB_ENTRIES: usize = 500;
const MAX_WALKED_ENTRIES: usize = 20_000;
const MAX_WALK_DEPTH: usize = 32;

fn glob(arguments: &str) -> Result<String> {
    let args: GlobArgs = parse(arguments)?;
    let pattern = args.pattern.trim().replace('\\', "/");
    let segments = pattern
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if segments.is_empty() {
        bail!("pattern cannot be empty");
    }
    if Path::new(&segments[0]).is_absolute() {
        bail!("patterns are relative to the working directory");
    }
    let requested = args.path.as_deref().unwrap_or(".").trim();
    let requested = if requested.is_empty() { "." } else { requested };
    let root = resolve(requested);
    if !root.is_dir() {
        bail!("{requested} is not a directory");
    }
    let mut matches = Vec::new();
    let mut walked = 0usize;
    walk_glob(&root, &String::new(), &segments, &mut matches, &mut walked)?;
    matches.sort();
    matches.dedup();
    let mut output = format!("{pattern} ({} matches)\n", matches.len());
    if matches.len() > MAX_GLOB_ENTRIES {
        output.push_str("… truncated\n");
        matches.truncate(MAX_GLOB_ENTRIES);
    }
    output.push_str(&matches.join("\n"));
    if matches.is_empty() {
        output.push_str("(no matches)");
    }
    Ok(truncate_output(output))
}

fn walk_glob(
    directory: &Path,
    display: &str,
    segments: &[String],
    matches: &mut Vec<String>,
    walked: &mut usize,
) -> Result<()> {
    if matches.len() >= MAX_GLOB_ENTRIES || display.matches('/').count() >= MAX_WALK_DEPTH {
        return Ok(());
    }
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        // An unreadable directory never stops the walk.
        Err(_) => return Ok(()),
    };
    let mut names: Vec<(String, bool)> = Vec::new();
    for entry in entries.flatten() {
        *walked += 1;
        if *walked > MAX_WALKED_ENTRIES {
            bail!("search stopped after {MAX_WALKED_ENTRIES} entries; use a narrower pattern");
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        names.push((name, is_dir));
    }
    for (name, is_dir) in names {
        let path = if display.is_empty() {
            name.clone()
        } else {
            format!("{display}/{name}")
        };
        if match_path(&segments, &path) {
            matches.push(path.clone());
        }
        if is_dir && !SKIP_DIRS.contains(&name.as_str()) {
            walk_glob(&directory.join(&name), &path, segments, matches, walked)?;
        }
    }
    Ok(())
}

/// Whether a full relative path matches glob `segments`, joining on `/` with
/// `*` (one segment, any characters), `?` (one character), and `**`.
fn match_path(segments: &[String], path: &str) -> bool {
    let path_segments: Vec<String> = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect();
    glob_matches(segments, &path_segments)
}

fn glob_matches(segments: &[String], path: &[String]) -> bool {
    let Some(head) = segments.first() else {
        return path.is_empty();
    };
    if head == "**" {
        return (0..=path.len()).any(|skip| glob_matches(&segments[1..], &path[skip..]));
    }
    match path.split_first() {
        Some((name, rest)) => {
            glob_segment(&head.chars().collect::<Vec<_>>(), &name.chars().collect::<Vec<_>>())
                && glob_matches(&segments[1..], rest)
        }
        None => false,
    }
}

/// One path segment: `*`, `?`, and literals, case-insensitive like paths on
/// Windows.
fn glob_segment(pattern: &[char], text: &[char]) -> bool {
    let Some((&head, rest)) = pattern.split_first() else {
        return text.is_empty();
    };
    if head == '*' {
        if glob_segment(rest, text) {
            return true;
        }
        return text
            .iter()
            .enumerate()
            .any(|(index, _)| glob_segment(rest, &text[index + 1..]));
    }
    if head == '?' {
        return match text.split_first() {
            Some((_, rest_text)) => glob_segment(rest, rest_text),
            None => false,
        };
    }
    match text.split_first() {
        Some((character, rest_text)) => {
            character.to_ascii_lowercase() == head.to_ascii_lowercase()
                && glob_segment(rest, rest_text)
        }
        None => false,
    }
}

#[derive(Deserialize)]
struct GrepArgs {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    include: Option<String>,
    #[serde(default)]
    ignore_case: bool,
}

const MAX_GREP_MATCHES: usize = 80;
const MAX_GREP_FILE_BYTES: u64 = 1024 * 1024;
const MAX_GREP_LINE_CHARS: usize = 200;

/// Substring search over text files under a directory, with a glob `include`
/// filter for file types. Binary files and dependencies are skipped.
fn grep(arguments: &str) -> Result<String> {
    let args: GrepArgs = parse(arguments)?;
    let pattern = args.pattern.trim();
    if pattern.is_empty() {
        bail!("pattern cannot be empty");
    }
    let requested = args.path.as_deref().unwrap_or(".").trim();
    let requested = if requested.is_empty() { "." } else { requested };
    let root = resolve(requested);
    if !root.is_dir() {
        bail!("{requested} is not a directory");
    }
    let include = args.include.filter(|include| !include.trim().is_empty()).map(|include| {
        include
            .trim()
            .replace('\\', "/")
            .split('/')
            .filter(|segment| !segment.is_empty())
            .map(str::to_owned)
            .collect::<Vec<String>>()
    });
    let needle = if args.ignore_case {
        pattern.to_ascii_lowercase()
    } else {
        pattern.to_owned()
    };
    let display = if requested == "." {
        String::new()
    } else {
        requested.trim_end_matches('/').replace('\\', "/")
    };
    let mut matches = Vec::new();
    let mut walked = 0usize;
    let include_segments = include.as_deref();
    walk_grep(
        &root,
        &display,
        &needle,
        args.ignore_case,
        include_segments,
        &mut matches,
        &mut walked,
    )?;
    if matches.is_empty() {
        return Ok(format!("{pattern}\n(no matches)"));
    }
    let mut output = format!("{pattern} ({} matches)\n", matches.len().min(MAX_GREP_MATCHES));
    output.push_str(&matches.join("\n"));
    if matches.len() >= MAX_GREP_MATCHES {
        output.push_str("\n… more matches truncated");
    }
    Ok(truncate_output(output))
}

#[allow(clippy::too_many_arguments)]
fn walk_grep(
    directory: &Path,
    display: &str,
    needle: &str,
    ignore_case: bool,
    include: Option<&[String]>,
    matches: &mut Vec<String>,
    walked: &mut usize,
) -> Result<()> {
    if matches.len() >= MAX_GREP_MATCHES || display.matches('/').count() >= MAX_WALK_DEPTH {
        return Ok(());
    }
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(_) => return Ok(()),
    };
    let mut names: Vec<(String, bool)> = Vec::new();
    for entry in entries.flatten() {
        *walked += 1;
        if *walked > MAX_WALKED_ENTRIES {
            bail!("search stopped after {MAX_WALKED_ENTRIES} entries; use a narrower path");
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        names.push((name, is_dir));
    }
    for (name, is_dir) in names {
        let path = if display.is_empty() {
            name.clone()
        } else {
            format!("{display}/{name}")
        };
        if is_dir {
            if !SKIP_DIRS.contains(&name.as_str()) {
                walk_grep(
                    &directory.join(&name),
                    &path,
                    needle,
                    ignore_case,
                    include,
                    matches,
                    walked,
                )?;
            }
            continue;
        }
        if !include_accepts(include, &name, &path) {
            continue;
        }
        let metadata = match fs::metadata(directory.join(&name)) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if metadata.len() > MAX_GREP_FILE_BYTES {
            continue;
        }
        let bytes = match fs::read(directory.join(&name)) {
            Ok(bytes) => bytes,
            Err(_) => continue,
        };
        if bytes.contains(&0) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        for (line_index, line) in text.lines().enumerate() {
            let hit = if ignore_case {
                line.to_ascii_lowercase().contains(needle)
            } else {
                line.contains(needle)
            };
            if hit {
                matches.push(format!(
                    "{path}:{}: {}",
                    line_index + 1,
                    clip(line, MAX_GREP_LINE_CHARS)
                ));
                if matches.len() >= MAX_GREP_MATCHES {
                    break;
                }
            }
        }
        if matches.len() >= MAX_GREP_MATCHES {
            break;
        }
    }
    Ok(())
}

/// Whether the include glob accepts the file: one segment matches the file
/// name, several match the joined path.
fn include_accepts(include: Option<&[String]>, file_name: &str, path: &str) -> bool {
    let Some(segments) = include else {
        return true;
    };
    if segments.len() == 1 {
        return glob_segment(
            &segments[0].chars().collect::<Vec<_>>(),
            &file_name.chars().collect::<Vec<_>>(),
        );
    }
    match_path(segments, path)
}

#[derive(Deserialize)]
struct WriteFileArgs {
    path: String,
    content: String,
}

fn write_file(arguments: &str) -> Result<String> {
    let args: WriteFileArgs = parse(arguments)?;
    if args.content.len() > MAX_WRITE_BYTES {
        bail!(
            "content is too large ({} bytes; the limit is {MAX_WRITE_BYTES})",
            args.content.len()
        );
    }
    let path = resolve(&args.path);
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(&path, args.content.as_bytes()).with_context(|| format!("writing {}", args.path))?;
    Ok(format!(
        "Wrote {} bytes to {}.",
        args.content.len(),
        args.path
    ))
}

#[derive(Deserialize)]
struct EditFileArgs {
    path: String,
    old_text: String,
    new_text: String,
    #[serde(default)]
    replace_all: bool,
}

fn edit_file(arguments: &str) -> Result<String> {
    let args: EditFileArgs = parse(arguments)?;
    if args.old_text.is_empty() {
        bail!("old_text cannot be empty");
    }
    let path = resolve(&args.path);
    let source = fs::read_to_string(&path).with_context(|| format!("reading {}", args.path))?;
    let matches = source.matches(&args.old_text).count();
    if matches == 0 {
        bail!("old_text was not found in {}", args.path);
    }
    if matches > 1 && !args.replace_all {
        bail!(
            "old_text matches {matches} times in {}; add more context or set replace_all to true",
            args.path
        );
    }
    let updated = if args.replace_all {
        source.replace(&args.old_text, &args.new_text)
    } else {
        source.replacen(&args.old_text, &args.new_text, 1)
    };
    fs::write(&path, updated.as_bytes()).with_context(|| format!("writing {}", args.path))?;
    let replaced = if args.replace_all { matches } else { 1 };
    Ok(format!(
        "Replaced {replaced} occurrence{} in {}.",
        if replaced == 1 { "" } else { "s" },
        args.path
    ))
}

/// How many `multi_edit` calls may be batched at once.
const MAX_MULTI_EDITS: usize = 50;

#[derive(Deserialize)]
struct MultiEditItem {
    path: String,
    old_text: String,
    new_text: String,
    #[serde(default)]
    replace_all: bool,
}

#[derive(Deserialize)]
struct MultiEditArgs {
    edits: Vec<MultiEditItem>,
}

fn multi_edit(arguments: &str) -> Result<String> {
    let updates = multi_edit_replay(arguments)?;
    for (path, _, after) in &updates {
        fs::write(resolve(path), after.as_bytes()).with_context(|| format!("writing {path}"))?;
    }
    let edits = updates.len();
    let files = updates
        .iter()
        .map(|(path, ..)| path)
        .collect::<std::collections::HashSet<_>>()
        .len();
    Ok(format!(
        "Applied {edits} edit{} across {files} file{}.",
        if edits == 1 { "" } else { "s" },
        if files == 1 { "" } else { "s" },
    ))
}

/// Validate and replay every edit against in-memory copies of the files: the
/// whole batch is refused up front, so no file is touched on a failure.
/// Returns `(path, contents before the batch, contents after)` per file.
fn multi_edit_replay(arguments: &str) -> Result<Vec<(String, String, String)>> {
    let args: MultiEditArgs = parse(arguments)?;
    if args.edits.len() > MAX_MULTI_EDITS {
        bail!(
            "too many edits ({}; the limit is {MAX_MULTI_EDITS})",
            args.edits.len()
        );
    }
    if args.edits.iter().map(|edit| edit.new_text.len()).sum::<usize>() > MAX_WRITE_BYTES {
        bail!("combined new_text is too large (the limit is {MAX_WRITE_BYTES} bytes)");
    }
    let mut order: Vec<String> = Vec::new();
    let mut originals: HashMap<String, String> = HashMap::new();
    let mut sources: HashMap<String, String> = HashMap::new();
    for edit in &args.edits {
        if edit.old_text.is_empty() {
            bail!("{}: old_text cannot be empty", edit.path);
        }
        if !sources.contains_key(&edit.path) {
            let source = fs::read_to_string(resolve(&edit.path)).with_context(|| {
                format!("reading {}; multi_edit only edits existing files", edit.path)
            })?;
            order.push(edit.path.clone());
            originals.insert(edit.path.clone(), source.clone());
            sources.insert(edit.path.clone(), source);
        }
        let updated = replace_one(&sources[&edit.path], edit)?;
        sources.insert(edit.path.clone(), updated);
    }
    Ok(order
        .into_iter()
        .map(|path| {
            let before = originals.remove(&path).unwrap_or_default();
            let after = sources.remove(&path).unwrap_or_default();
            (path, before, after)
        })
        .collect())
}

/// Apply one edit to a string with the same rules as `edit_file`.
fn replace_one(source: &str, edit: &MultiEditItem) -> Result<String> {
    let matches = source.matches(&edit.old_text).count();
    if matches == 0 {
        bail!("old_text was not found in {}", edit.path);
    }
    if matches > 1 && !edit.replace_all {
        bail!(
            "old_text matches {matches} times in {}; add more context or set replace_all",
            edit.path
        );
    }
    if edit.replace_all {
        Ok(source.replace(&edit.old_text, &edit.new_text))
    } else {
        Ok(source.replacen(&edit.old_text, &edit.new_text, 1))
    }
}

fn multi_edit_preview(arguments: &str) -> ChangePreview {
    let updates = match multi_edit_replay(arguments) {
        Ok(updates) => updates,
        Err(error) => return preview_error(error),
    };
    ChangePreview {
        diffs: updates
            .iter()
            .map(|(path, before, after)| {
                FileDiff::new(path.as_str(), before.as_str(), after.as_str(), false)
            })
            .collect(),
        before: None,
        note: None,
    }
}

#[derive(Deserialize)]
struct RunCommandArgs {
    command: String,
    #[serde(default)]
    timeout_seconds: Option<u64>,
}

fn run_command(arguments: &str) -> Result<String> {
    let args: RunCommandArgs = parse(arguments)?;
    let command = args.command.trim();
    if command.is_empty() {
        bail!("command cannot be empty");
    }
    let timeout = args
        .timeout_seconds
        .unwrap_or(DEFAULT_TIMEOUT_SECONDS)
        .clamp(1, MAX_TIMEOUT_SECONDS);

    let log_path = temporary_log_path();
    let file =
        fs::File::create(&log_path).with_context(|| format!("creating {}", log_path.display()))?;
    let stderr = file
        .try_clone()
        .with_context(|| format!("opening {}", log_path.display()))?;

    let mut process = if cfg!(windows) {
        let mut process = ProcessCommand::new("cmd");
        process.args(["/C", command]);
        process
    } else {
        let mut process = ProcessCommand::new("sh");
        process.args(["-c", command]);
        process
    };
    process
        .current_dir(workspace())
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(stderr));

    let mut child = process
        .spawn()
        .with_context(|| format!("running `{command}`"))?;
    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait().context("waiting for the command")? {
            break Some(status);
        }
        if started.elapsed() >= Duration::from_secs(timeout) {
            let _ = child.kill();
            let _ = child.wait();
            timed_out = true;
            break None;
        }
        thread::sleep(Duration::from_millis(25));
    };

    let output = read_log(&log_path);
    let _ = fs::remove_file(&log_path);
    let mut result = String::new();
    if timed_out {
        result.push_str(&format!("Command timed out after {timeout}s.\n"));
    } else if let Some(status) = status {
        if !status.success() {
            result.push_str(&format!("Exit code: {}.\n", status.code().unwrap_or(-1)));
        }
    }
    if output.is_empty() {
        result.push_str("(no output)");
    } else {
        result.push_str(&output);
    }
    Ok(truncate_output(result))
}

#[derive(Deserialize)]
struct WebSearchArgs {
    query: String,
    #[serde(default)]
    max_results: Option<usize>,
}

const TAVILY_SEARCH_URL: &str = "https://api.tavily.com/search";
const MAX_WEB_RESULTS: usize = 10;

/// Search the web with Tavily and format the top results.
///
/// Uses a blocking HTTP client because tool execution runs on
/// `spawn_blocking`, not on the async runtime.
fn web_search(arguments: &str, api_key: Option<&str>) -> Result<String> {
    let Some(api_key) = api_key.map(str::trim).filter(|key| !key.is_empty()) else {
        bail!(
            "web search is not configured; add tavily_api_key to the config or set \
             TAVILY_API_KEY, then restart yopes"
        );
    };
    let args: WebSearchArgs = parse(arguments)?;
    let query = args.query.trim();
    if query.is_empty() {
        bail!("query cannot be empty");
    }
    let max_results = args.max_results.unwrap_or(5).clamp(1, MAX_WEB_RESULTS);
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .context("creating the search client")?;
    let response = client
        .post(TAVILY_SEARCH_URL)
        .bearer_auth(api_key)
        .json(&json!({ "query": query, "max_results": max_results }))
        .send()
        .context("contacting Tavily")?;
    let status = response.status();
    let body: Value = response.json().context("reading the Tavily response")?;
    if !status.is_success() {
        let message = body
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or("no detail");
        bail!("Tavily returned {status}: {message}");
    }
    let results = body
        .get("results")
        .and_then(Value::as_array)
        .context("the Tavily response has no results")?;
    if results.is_empty() {
        return Ok(format!("{query}\n(no results)"));
    }
    let mut output = format!("{query}\n");
    for (index, result) in results.iter().enumerate() {
        let title = result
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("(untitled)");
        let url = result
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let snippet = result
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default();
        output.push_str(&format!(
            "\n{}. {title}\n{url}\n{}\n",
            index + 1,
            snippet.trim()
        ));
    }
    Ok(truncate_output(output))
}

#[derive(Deserialize)]
struct TodoItemArgs {
    text: String,
    #[serde(default)]
    done: bool,
}

#[derive(Deserialize)]
struct TodoWriteArgs {
    items: Vec<TodoItemArgs>,
}

const MAX_TODO_ITEMS: usize = 100;
const MAX_TODO_TEXT: usize = 500;

/// Rewrite the workspace checklist as `todo.md` with checkbox lines.
///
/// The model owns the list content: it sends the whole list every time, so the
/// file is always a faithful mirror of what the model believes.
fn todo_write(arguments: &str) -> Result<String> {
    let args: TodoWriteArgs = parse(arguments)?;
    if args.items.is_empty() {
        bail!("items cannot be empty");
    }
    if args.items.len() > MAX_TODO_ITEMS {
        bail!("too many items; the checklist is capped at {MAX_TODO_ITEMS}");
    }
    let mut file = String::new();
    for item in &args.items {
        let text = item.text.trim();
        if text.is_empty() {
            bail!("item text cannot be empty");
        }
        let text = clip(text, MAX_TODO_TEXT);
        let mark = if item.done { "x" } else { " " };
        file.push_str(&format!("- [{mark}] {text}\n"));
    }
    let path = workspace().join("todo.md");
    fs::write(&path, file).with_context(|| format!("writing {}", path.display()))?;
    let done = args.items.iter().filter(|item| item.done).count();
    Ok(format!(
        "todo.md written: {} item{} ({done} done)",
        args.items.len(),
        if args.items.len() == 1 { "" } else { "s" }
    ))
}

fn workspace() -> PathBuf {
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn resolve(path: &str) -> PathBuf {
    let path = path.trim();
    let path = if path.is_empty() { "." } else { path };
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace().join(path)
    }
}

fn parse<T>(arguments: &str) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    if arguments.trim().is_empty() {
        serde_json::from_str("{}").context("tool arguments must be a JSON object")
    } else {
        serde_json::from_str(arguments)
            .with_context(|| format!("invalid tool arguments: {}", clip(arguments, 200)))
    }
}

fn clip(text: &str, width: usize) -> String {
    let mut characters = text.chars();
    let mut result = characters.by_ref().take(width).collect::<String>();
    if characters.next().is_some() {
        result.pop();
        result.push('…');
    }
    result
}

fn truncate_output(mut text: String) -> String {
    if text.chars().count() <= MAX_OUTPUT_CHARS {
        return text;
    }
    text = text.chars().take(MAX_OUTPUT_CHARS).collect();
    text.push_str("\n… output truncated");
    text
}

fn temporary_log_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    env::temp_dir().join(format!("yopes-tool-{}-{nanos}.log", std::process::id()))
}

fn read_log(path: &Path) -> String {
    let Ok(file) = fs::File::open(path) else {
        return String::new();
    };
    let mut buffer = Vec::new();
    let _ = file
        .take((MAX_OUTPUT_CHARS * 4) as u64)
        .read_to_end(&mut buffer);
    String::from_utf8_lossy(&buffer).replace("\r\n", "\n")
}

#[cfg(test)]
mod tests {
    use super::{TOOL_NAMES, applied_change, definitions, preview, requires_approval};
    use crate::session::ToolCall;

    #[test]
    fn definitions_cover_every_tool() {
        let definitions = definitions();
        let names = definitions
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|definition| definition.pointer("/function/name"))
            .filter_map(|name| name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, TOOL_NAMES);
    }

    #[test]
    fn writes_never_ask_and_only_risky_commands_do() {
        let call = |name: &str, arguments: &str| ToolCall::new("call_1", name, arguments);
        assert!(!requires_approval(&call(
            "write_file",
            r#"{"path":"a.txt","content":"x"}"#
        )));
        assert!(!requires_approval(&call(
            "edit_file",
            r#"{"path":"a.txt","old_text":"x","new_text":"y"}"#
        )));
        assert!(!requires_approval(&call(
            "read_file",
            r#"{"path":"a.txt"}"#
        )));
        assert!(!requires_approval(&call(
            "run_command",
            r#"{"command":"cargo fmt --all"}"#
        )));
        assert!(!requires_approval(&call(
            "run_command",
            r#"{"command":"ls","risk":false}"#
        )));
        assert!(requires_approval(&call(
            "run_command",
            r#"{"command":"rm -rf build","risk":true}"#
        )));
    }

    #[test]
    fn write_diffs_are_new_only_when_the_file_did_not_exist_before() {
        let call = ToolCall::new(
            "call_1",
            "write_file",
            r#"{"path":"new.txt","content":"hello"}"#,
        );
        assert!(applied_change(&call, Some(String::new()), false).unwrap().created);
        assert!(!applied_change(&call, Some("old".to_owned()), true).unwrap().created);
    }

    #[test]
    fn multi_edit_applies_or_refuses_whole_batches() {
        use super::execute;
        use std::fs;
        let dir = std::env::temp_dir().join(format!("yopes-multi-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.txt"), "alpha\nbeta\nalpha\n").unwrap();
        fs::write(dir.join("b.txt"), "one\n").unwrap();
        let a = dir.join("a.txt").display().to_string();
        let b = dir.join("b.txt").display().to_string();
        let batch = json!({
            "edits": [
                { "path": a, "old_text": "alpha", "new_text": "A" },
                { "path": a, "old_text": "beta", "new_text": "B", "replace_all": true },
                { "path": b, "old_text": "one", "new_text": "uno" },
            ]
        });
        let output = execute(&ToolCall::new("call_20", "multi_edit", &batch.to_string()), None).unwrap();
        assert!(output.contains("Applied 3 edits across 2 files"), "{output}");
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "A\nB\nA\n");
        assert_eq!(fs::read_to_string(dir.join("b.txt")).unwrap(), "uno\n");

        // A bad edit refuses the whole batch and leaves every file untouched.
        let broken = json!({
            "edits": [
                { "path": a, "old_text": "A", "new_text": "X" },
                { "path": b, "old_text": "absent", "new_text": "?" },
            ]
        });
        let error = execute(&ToolCall::new("call_21", "multi_edit", &broken.to_string()), None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("was not found"), "{error}");
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "A\nB\nA\n");
        assert_eq!(fs::read_to_string(dir.join("b.txt")).unwrap(), "uno\n");
        assert!(!requires_approval(&ToolCall::new(
            "call_22",
            "multi_edit",
            r#"{"edits":[{"path":"a.txt","old_text":"x","new_text":"y"}]}"#
        )));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn multi_edit_preview_reports_every_file_nothing_written() {
        use super::{change_preview, execute};
        use std::fs;
        let dir = std::env::temp_dir().join(format!("yopes-multi-preview-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("n.txt"), "aa\nbb\n").unwrap();
        fs::write(dir.join("m.txt"), "cc\n").unwrap();
        let args = json!({
            "edits": [
                { "path": dir.join("n.txt").display().to_string(), "old_text": "aa", "new_text": "xx" },
                { "path": dir.join("m.txt").display().to_string(), "old_text": "cc", "new_text": "dd" },
            ]
        });
        let call = ToolCall::new("call_23", "multi_edit", &args.to_string());
        let change = change_preview(&call);
        assert_eq!(change.diffs.len(), 2);
        assert!(change.before.is_none());
        // Only the preview ran; the files are still as they were.
        assert_eq!(fs::read_to_string(dir.join("n.txt")).unwrap(), "aa\nbb\n");
        assert_eq!(fs::read_to_string(dir.join("m.txt")).unwrap(), "cc\n");
        assert!(execute(&call, None).is_ok());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn web_search_needs_a_key_and_previews_the_query() {
        use super::execute;
        let search = ToolCall::new("call_3", "web_search", r#"{"query":"rust 2026 release"}"#);
        assert_eq!(preview(&search), "rust 2026 release");

        let error = execute(&search, None).unwrap_err().to_string();
        assert!(error.contains("TAVILY_API_KEY"));
        let empty = ToolCall::new("call_4", "web_search", r#"{"query":"  "}"#);
        let error = execute(&empty, Some("tvly-test")).unwrap_err().to_string();
        assert!(error.contains("query"));
    }

    #[test]
    fn glob_segments_match_one_directory_level_only() {
        use super::{glob_matches, glob_segment};
        let segment = |pattern: &str, text: &str| {
            glob_segment(
                &pattern.chars().collect::<Vec<_>>(),
                &text.chars().collect::<Vec<_>>(),
            )
        };
        assert!(segment("*", "anything"));
        assert!(segment("*.rs", "main.rs"));
        assert!(!segment("*.rs", "sub/main.rs"));
        assert!(segment("?at", "cat"));
        assert!(!segment("?at", "chat"));
        assert!(segment("a*b", "ab"));
        assert!(segment("a*b", "acb"));
        assert!(!segment("a*b", "aXc"));
        assert!(segment("*.RS", "main.rs"), "matching is case-insensitive");

        let path = |pattern: &[&str], text: &[&str]| {
            glob_matches(
                &pattern
                    .iter()
                    .map(|segment| (*segment).to_owned())
                    .collect::<Vec<_>>(),
                &text
                    .iter()
                    .map(|segment| (*segment).to_owned())
                    .collect::<Vec<_>>(),
            )
        };
        assert!(path(&["**"], &[]));
        assert!(path(&["**"], &["a", "b"]));
        assert!(path(&["a", "**", "c"], &["a", "b", "c"]));
        assert!(path(&["src", "*.rs"], &["src", "main.rs"]));
        assert!(!path(&["src", "*.rs"], &["src", "nested", "main.rs"]));
        assert!(path(&["src", "**", "*.rs"], &["src", "nested", "main.rs"]));
    }

    #[test]
    fn globs_skip_dependency_directories() {
        use super::execute;
        let dir = std::env::temp_dir().join(format!("yopes-glob-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub").join("deep")).unwrap();
        std::fs::write(dir.join("README.md"), "# x").unwrap();
        std::fs::write(dir.join("sub").join("note.md"), "x").unwrap();
        std::fs::write(dir.join("sub").join("deep").join("last.md"), "x").unwrap();
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::write(dir.join("target").join("stale.md"), "x").unwrap();

        let args = |pattern: &str| {
            format!(
                r#"{{"pattern":{:?},"path":{:?}}}"#,
                pattern,
                dir.display().to_string()
            )
        };
        let all = ToolCall::new("call_10", "glob", &args("**/*.md"));
        let output = execute(&all, None).unwrap();
        assert!(output.contains("README.md"), "{output}");
        assert!(output.contains("sub/note.md"), "{output}");
        assert!(output.contains("sub/deep/last.md"), "{output}");
        assert!(!output.contains("target"), "{output}");

        let shallow = ToolCall::new("call_11", "glob", &args("*.md"));
        let output = execute(&shallow, None).unwrap();
        assert!(output.contains("README.md"));
        assert!(!output.contains("note.md"), "{output}");

        let missing = ToolCall::new("call_12", "glob", &args("*.toml"));
        let output = execute(&missing, None).unwrap();
        assert!(output.contains("(no matches)"), "{output}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn grep_matches_lines_and_respects_includes() {
        use super::execute;
        use std::fs;
        let dir = std::env::temp_dir().join(format!("yopes-grep-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("top.txt"), "needle here\nother\nneedles again").unwrap();
        fs::write(dir.join("sub").join("code.rs"), "let needle = 1;").unwrap();
        // A binary file that mentions the pattern must stay silent.
        fs::write(dir.join("skip.bin"), [0u8, b'n', b'e', b'e', b'd']).unwrap();

        let args = |more: &str| {
            format!(r#"{{"pattern":"needle","path":{:?}{more}}}"#, dir.display().to_string())
        };
        let every = execute(&ToolCall::new("call_13", "grep", &args("")), None).unwrap();
        assert!(every.contains("top.txt:1:"), "{every}");
        assert!(every.contains("top.txt:3:"), "{every}");
        assert!(every.contains("code.rs:1:"), "{every}");
        assert!(!every.contains("skip.bin"), "{every}");

        let only_rust =
            execute(&ToolCall::new("call_14", "grep", &args(r#","include":"*.rs""#)), None)
                .unwrap();
        assert!(only_rust.contains("code.rs:1:"));
        assert!(!only_rust.contains("top.txt"), "{only_rust}");

        let insensitive = execute(
            &ToolCall::new("call_15", "grep", &args(r#","ignore_case":true"#)),
            None,
        )
        .unwrap();
        assert!(insensitive.contains("top.txt:1:"), "{insensitive}");

        let absent = execute(
            &ToolCall::new("call_16", "grep", r#"{"pattern":"absent-nowhere"}"#),
            None,
        )
        .unwrap();
        assert!(absent.contains("(no matches)"), "{absent}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_file_lists_a_directory_instead_of_failing() {
        use super::execute;
        use std::fs;
        let dir = std::env::temp_dir().join(format!("yopes-read-dir-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("nested")).unwrap();
        fs::write(dir.join("note.txt"), "hi").unwrap();
        let call = ToolCall::new(
            "call_7",
            "read_file",
            &format!(r#"{{"path":{:?}}}"#, dir.display().to_string()),
        );
        let output = execute(&call, None).unwrap();
        assert!(output.contains("note.txt"), "{output}");
        assert!(output.contains("nested/"), "{output}");
        // Dot also works, and line ranges stay file-only.
        let listing = ToolCall::new("call_8", "read_file", r#"{"path":"."}"#);
        assert!(execute(&listing, None).is_ok());
        let ranged = ToolCall::new(
            "call_9",
            "read_file",
            r#"{"path":".","start_line":2}"#,
        );
        let error = execute(&ranged, None).unwrap_err().to_string();
        assert!(error.contains("directory"), "{error}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn todo_write_rejects_an_empty_list_and_previews_the_count() {
        use super::execute;
        let empty = ToolCall::new("call_5", "todo_write", r#"{"items":[]}"#);
        let error = execute(&empty, None).unwrap_err().to_string();
        assert!(error.contains("items cannot be empty"));

        let call = ToolCall::new(
            "call_6",
            "todo_write",
            r#"{"items":[{"text":"a"},{"text":"b","done":true}]}"#,
        );
        assert_eq!(preview(&call), "2 items");
        assert!(!requires_approval(&call));
    }

    #[test]
    fn previews_name_the_target_without_dumping_content() {
        let write = ToolCall::new(
            "call_1",
            "write_file",
            r#"{"path":"test.html","content":"<h1>hi</h1>"}"#,
        );
        assert_eq!(preview(&write), "test.html · 11 bytes");

        let command = ToolCall::new("call_2", "run_command", r#"{"command":"cargo fmt --all"}"#);
        assert_eq!(preview(&command), "cargo fmt --all");
    }
}
