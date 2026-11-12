//! The tool surface offered to the model: definitions, approval rules, change
//! previews, and the dispatch that runs a call.
//!
//! `mod.rs` holds the contract shared by every tool: the JSON schema in
//! `definitions`, the approval and preview rules, and `execute`. The tools
//! themselves live in one module per family: `files` (read, write, edit),
//! `search` (glob, grep), `shell` (commands and background jobs), `web`
//! (search, fetch) and `todo`.

mod files;
mod search;
mod shell;
mod todo;
mod web;

use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub(crate) use files::{change_preview, edit_targets};

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
pub const TOOL_NAMES: [&str; 15] = [
    "list_dir",
    "read_file",
    "glob",
    "grep",
    "write_file",
    "edit_file",
    "multi_edit",
    "run_command",
    "bash_background",
    "bash_output",
    "bash_kill",
    "web_search",
    "web_fetch",
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
    matches!(
        call.function.name.as_str(),
        "run_command" | "bash_background"
    ) && command_risk(&call.function.arguments)
}

/// The `risk` flag the model set on a `run_command` or `bash_background` call.
pub fn command_risk(arguments: &str) -> bool {
    serde_json::from_str::<Value>(arguments)
        .ok()
        .and_then(|arguments| arguments.get("risk").and_then(Value::as_bool))
        .unwrap_or(false)
}

pub fn system_prompt(tools_enabled: bool) -> String {
    let cwd = env::current_dir().map_or_else(|_| ".".to_owned(), |path| path.display().to_string());
    let os = env::consts::OS;
    let base = if tools_enabled {
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
             Use web_fetch to read the text of a specific page, such as one surfaced by \
             web_search. \
             Start long-running or detached work with bash_background, then read it with \
             bash_output and stop it with bash_kill when it has served its purpose. \
             For multi-step work or when the user asks for a todo list, maintain the \
             checklist with todo_write: send the whole list each call, marking done items, \
             and send an empty list to close the checklist. \
             For an independent subtask use delegate: hand the subagent one self-contained \
             task with all the context it needs — it has no tools and returns a final answer. \
             Format user-facing replies with Markdown by default: use ## headings for sections, \
             bullets or numbered lists for multiple items, **bold** for key terms, and fenced \
             code blocks with a language tag for code or commands. Use inline code for paths, \
             commands, identifiers, and short snippets. Keep paragraphs readable; do not flatten \
             lists or code into plain prose. Keep responses short and concrete. Paths may be relative to the working directory."
        )
    } else {
        format!(
            "You are yopes, a coding assistant running in the user's terminal.\n\
             Working directory: {cwd}\n\
             Operating system: {os}\n\
             Format replies with Markdown by default: use ## headings for sections, bullets or \
             numbered lists for multiple items, **bold** for key terms, and fenced code blocks \
             with language tags for code. Use inline code for paths, commands, and identifiers. \
             Keep paragraphs readable; do not flatten lists or code into plain prose. \
             Keep responses short and concrete."
        )
    };
    let project_instructions =
        load_agents_instructions(&env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    if project_instructions.is_empty() {
        base
    } else {
        format!(
            "{base}\n\nProject Markdown context (AGENTS.md files contain instructions):\n{project_instructions}"
        )
    }
}

fn load_agents_instructions(root: &Path) -> String {
    const MAX_FILES: usize = 32;
    const MAX_TOTAL_CHARS: usize = 24_000;
    const MAX_FILE_CHARS: usize = 8_000;
    let mut files = Vec::new();
    collect_agents_files(root, 0, &mut files);
    files.sort_by(|a, b| {
        let depth_a = a.parent().map_or(0, |p| {
            p.strip_prefix(root).unwrap_or(p).components().count()
        });
        let depth_b = b.parent().map_or(0, |p| {
            p.strip_prefix(root).unwrap_or(p).components().count()
        });
        depth_a.cmp(&depth_b).then_with(|| a.cmp(b))
    });
    let mut output = String::new();
    for path in files.into_iter().take(MAX_FILES) {
        if output.len() >= MAX_TOTAL_CHARS {
            break;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let content: String = content.chars().take(MAX_FILE_CHARS).collect();
        let relative = path.strip_prefix(root).unwrap_or(&path).display();
        let remaining = MAX_TOTAL_CHARS.saturating_sub(output.len());
        output.push_str(&format!("\n--- {} ---\n", relative));
        output.push_str(&content.chars().take(remaining).collect::<String>());
        output.push('\n');
    }
    output
}

fn collect_agents_files(directory: &Path, depth: usize, files: &mut Vec<PathBuf>) {
    if depth > 12 || files.len() >= 64 {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut entries = entries.flatten().collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        if name == ".git" || name == "target" || name == "node_modules" || name == ".venv" {
            continue;
        }
        if path.is_dir() {
            collect_agents_files(&path, depth + 1, files);
        } else if path.extension().is_some_and(|extension| extension == "md") {
            files.push(path);
        }
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
                "name": "bash_background",
                "description": "Start one bash shell command in the background and get a job id. Its output lands in a log: read it with bash_output, stop it with bash_kill. Set risk to true for anything destructive or hard to undo so the user is asked first.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "Bash command to start."
                        },
                        "risk": {
                            "type": "boolean",
                            "description": "True when the command is destructive or could discard work, so the user is asked first."
                        },
                        "timeout_seconds": {
                            "type": "integer",
                            "description": "Kill the job after this many seconds. Defaults to 120."
                        }
                    },
                    "required": ["command"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "bash_output",
                "description": "Read a background bash job: everything it printed so far and whether it still runs or has exited. Pass the job id from bash_background.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "job_id": {
                            "type": "integer",
                            "description": "The id bash_background returned."
                        }
                    },
                    "required": ["job_id"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "bash_kill",
                "description": "Stop a background bash job by its id. A finished or unknown job answers so, no need to kill twice.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "job_id": {
                            "type": "integer",
                            "description": "The id bash_background returned."
                        }
                    },
                    "required": ["job_id"]
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
                "name": "web_fetch",
                "description": "Fetch one web page over HTTP GET and return its readable text: scripts, styles and tags are stripped. Use it when search results point at a page worth reading or when the user gives an address.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "Absolute http:// or https:// address."
                        }
                    },
                    "required": ["url"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "todo_write",
                "description": "Maintain the session task checklist. Rewrite the whole list each call: every item is an object with the task text and a done flag. Send an empty items list to close the checklist once everything is done. The list is stored with the session, not as a file. Use it whenever the user asks for a todo list or a multi-step plan.",
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

fn write_file_preview(arguments: &str) -> ChangePreview {
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

fn edit_file_preview(arguments: &str) -> ChangePreview {
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
        "list_dir" => files::list_dir(&call.function.arguments),
        "read_file" => files::read_file(&call.function.arguments),
        "glob" => search::glob(&call.function.arguments),
        "grep" => search::grep(&call.function.arguments),
        "write_file" => files::write_file(&call.function.arguments),
        "edit_file" => files::edit_file(&call.function.arguments),
        "multi_edit" => files::multi_edit(&call.function.arguments),
        "run_command" => shell::run_command(&call.function.arguments),
        "bash_background" => shell::bash_background(&call.function.arguments),
        "bash_output" => shell::bash_output(&call.function.arguments),
        "bash_kill" => shell::bash_kill(&call.function.arguments),
        "web_search" => web::web_search(&call.function.arguments, web_key),
        "web_fetch" => web::web_fetch(&call.function.arguments),
        "todo_write" => todo::todo_write(&call.function.arguments),
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
    use serde_json::json;

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
        assert!(
            applied_change(&call, Some(String::new()), false)
                .unwrap()
                .created
        );
        assert!(
            !applied_change(&call, Some("old".to_owned()), true)
                .unwrap()
                .created
        );
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
                { "path": a, "old_text": "alpha", "new_text": "A", "replace_all": true },
                { "path": a, "old_text": "beta", "new_text": "B", "replace_all": true },
                { "path": b, "old_text": "one", "new_text": "uno" },
            ]
        });
        let output = execute(
            &ToolCall::new("call_20", "multi_edit", batch.to_string()),
            None,
        )
        .unwrap();
        assert!(
            output.contains("Applied 3 edits across 2 files"),
            "{output}"
        );
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "A\nB\nA\n");
        assert_eq!(fs::read_to_string(dir.join("b.txt")).unwrap(), "uno\n");

        // A bad edit refuses the whole batch and leaves every file untouched.
        let broken = json!({
            "edits": [
                { "path": a, "old_text": "A", "new_text": "X", "replace_all": true },
                { "path": b, "old_text": "absent", "new_text": "?" },
            ]
        });
        let error = execute(
            &ToolCall::new("call_21", "multi_edit", broken.to_string()),
            None,
        )
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
    fn edit_file_matches_lf_text_against_crlf_files() {
        use super::execute;
        use std::fs;
        let dir = std::env::temp_dir().join(format!("yopes-crlf-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.txt");
        fs::write(&path, "alpha = 1\r\nbeta = 2\r\nalpha = 1\r\n").unwrap();
        let target = path.display().to_string();

        // An LF-terminated needle matches CRLF text; the inserted lines adopt
        // the file's endings and untouched lines keep their exact bytes.
        let single = json!({
            "path": target,
            "old_text": "beta = 2\n",
            "new_text": "beta = 20\n",
        });
        let output = execute(
            &ToolCall::new("call_23", "edit_file", single.to_string()),
            None,
        )
        .unwrap();
        assert!(output.contains("Replaced 1 occurrence"), "{output}");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "alpha = 1\r\nbeta = 20\r\nalpha = 1\r\n"
        );

        // replace_all converts every inserted line the same way.
        let all = json!({
            "path": target,
            "old_text": "alpha = 1\n",
            "new_text": "gamma = 3\n",
            "replace_all": true,
        });
        execute(
            &ToolCall::new("call_24", "edit_file", all.to_string()),
            None,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "gamma = 3\r\nbeta = 20\r\ngamma = 3\r\n"
        );
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
        let call = ToolCall::new("call_23", "multi_edit", args.to_string());
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
    fn bash_background_output_and_kill_handle_the_lifecycle() {
        use super::execute;
        use super::shell::bash_path;
        use std::{thread, time::Duration};
        if bash_path().is_none() {
            // No bash on this machine yet; the tools refuse gracefully.
            let error = execute(
                &ToolCall::new("call_40", "bash_background", r#"{"command":"echo hi"}"#),
                None,
            )
            .unwrap_err()
            .to_string();
            assert!(error.contains("bash is not available"), "{error}");
            return;
        }
        let job_id = |started: &str| {
            started
                .split("bash job ")
                .nth(1)
                .and_then(|part| part.split_whitespace().next())
                .and_then(|part| part.parse::<u32>().ok())
                .unwrap()
        };
        let started = execute(
            &ToolCall::new(
                "call_41",
                "bash_background",
                r#"{"command":"printf step-one","timeout_seconds":30}"#,
            ),
            None,
        )
        .unwrap();
        let id = job_id(&started);
        let arguments = format!(r#"{{"job_id":{id}}}"#);
        let mut output = String::new();
        for _ in 0..50 {
            output = execute(&ToolCall::new("call_42", "bash_output", &arguments), None).unwrap();
            if output.contains("Exit code") {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        assert!(output.contains("step-one"), "{output}");
        assert!(output.contains("Exit code"), "{output}");

        // An unknown id refuses with the same phrasing everywhere.
        let error = execute(
            &ToolCall::new("call_43", "bash_output", r#"{"job_id":999999}"#),
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("no bash job"), "{error}");

        // Killing a sleeper ends it immediately with no exit code.
        let sleeper = execute(
            &ToolCall::new(
                "call_44",
                "bash_background",
                r#"{"command":"sleep 60","timeout_seconds":60}"#,
            ),
            None,
        )
        .unwrap();
        let id = job_id(&sleeper);
        let arguments = format!(r#"{{"job_id":{id}}}"#);
        let killed = execute(&ToolCall::new("call_45", "bash_kill", &arguments), None).unwrap();
        assert!(killed.contains("Killed"), "{killed}");
        thread::sleep(Duration::from_millis(50));
        let after = execute(&ToolCall::new("call_46", "bash_output", &arguments), None).unwrap();
        assert!(after.contains("Exit code"), "{after}");
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
    fn web_fetch_guards_urls_and_strips_the_page_noise() {
        use super::execute;
        use super::web::strip_html;
        let empty = ToolCall::new("call_30", "web_fetch", r#"{"url":"  "}"#);
        let error = execute(&empty, None).unwrap_err().to_string();
        assert!(error.contains("url"), "{error}");
        let scheme = ToolCall::new("call_31", "web_fetch", r#"{"url":"ftp://example.test"}"#);
        let error = execute(&scheme, None).unwrap_err().to_string();
        assert!(error.contains("http"), "{error}");

        assert_eq!(
            strip_html(
                "<html><head><script>noise()</script><style>p { color: red; }</style>\
                 </head><body>Hi &amp; <b>bye</b></body></html>"
            ),
            "Hi & bye"
        );
        // An unterminated block swallows what follows, and look-alike tags
        // such as <scripts> are kept as plain text.
        assert_eq!(strip_html("<p>keep</p><SCRIPT>drop"), "keep");
        assert_eq!(strip_html("<scripts>x</scripts>"), "x");
    }

    #[test]
    fn glob_segments_match_one_directory_level_only() {
        use super::search::{glob_matches, glob_segment};
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
        let all = ToolCall::new("call_10", "glob", args("**/*.md"));
        let output = execute(&all, None).unwrap();
        assert!(output.contains("README.md"), "{output}");
        assert!(output.contains("sub/note.md"), "{output}");
        assert!(output.contains("sub/deep/last.md"), "{output}");
        assert!(!output.contains("target"), "{output}");

        let shallow = ToolCall::new("call_11", "glob", args("*.md"));
        let output = execute(&shallow, None).unwrap();
        assert!(output.contains("README.md"));
        assert!(!output.contains("note.md"), "{output}");

        let missing = ToolCall::new("call_12", "glob", args("*.toml"));
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
            format!(
                r#"{{"pattern":"needle","path":{:?}{more}}}"#,
                dir.display().to_string()
            )
        };
        let every = execute(&ToolCall::new("call_13", "grep", args("")), None).unwrap();
        assert!(every.contains("top.txt:1:"), "{every}");
        assert!(every.contains("top.txt:3:"), "{every}");
        assert!(every.contains("code.rs:1:"), "{every}");
        assert!(!every.contains("skip.bin"), "{every}");

        let only_rust = execute(
            &ToolCall::new("call_14", "grep", args(r#","include":"*.rs""#)),
            None,
        )
        .unwrap();
        assert!(only_rust.contains("code.rs:1:"));
        assert!(!only_rust.contains("top.txt"), "{only_rust}");

        let insensitive = execute(
            &ToolCall::new("call_15", "grep", args(r#","ignore_case":true"#)),
            None,
        )
        .unwrap();
        assert!(insensitive.contains("top.txt:1:"), "{insensitive}");

        let absent_args = format!(
            r#"{{"pattern":"absent-nowhere","path":{:?}}}"#,
            dir.display().to_string()
        );
        let absent = execute(&ToolCall::new("call_16", "grep", absent_args), None).unwrap();
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
            format!(r#"{{"path":{:?}}}"#, dir.display().to_string()),
        );
        let output = execute(&call, None).unwrap();
        assert!(output.contains("note.txt"), "{output}");
        assert!(output.contains("nested/"), "{output}");
        // Dot also works, and line ranges stay file-only.
        let listing = ToolCall::new("call_8", "read_file", r#"{"path":"."}"#);
        assert!(execute(&listing, None).is_ok());
        let ranged = ToolCall::new("call_9", "read_file", r#"{"path":".","start_line":2}"#);
        let error = execute(&ranged, None).unwrap_err().to_string();
        assert!(error.contains("directory"), "{error}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn todo_write_allows_an_empty_list_to_clear_and_previews_the_count() {
        use super::execute;
        let empty = ToolCall::new("call_5", "todo_write", r#"{"items":[]}"#);
        assert_eq!(execute(&empty, None).unwrap(), "[]");

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
