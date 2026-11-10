use std::{
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
pub const TOOL_NAMES: [&str; 5] = [
    "list_dir",
    "read_file",
    "write_file",
    "edit_file",
    "run_command",
];

#[derive(Debug)]
pub struct ChangePreview {
    pub diff: Option<FileDiff>,
    pub note: Option<String>,
}

pub fn requires_approval(name: &str) -> bool {
    matches!(name, "write_file" | "edit_file" | "run_command")
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
             that a file changed unless a tool result confirms it. Keep responses short \
             and concrete. Paths may be relative to the working directory."
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
                "description": "Read a UTF-8 text file, optionally a range of lines.",
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
                "name": "run_command",
                "description": "Run a shell command in the workspace and return its output.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "Shell command to run from the working directory."
                        },
                        "timeout_seconds": {
                            "type": "integer",
                            "description": "Maximum run time in seconds. Defaults to 120."
                        }
                    },
                    "required": ["command"]
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
        "list_dir" => {
            let path = string("path");
            if path.is_empty() {
                ".".to_owned()
            } else {
                path
            }
        }
        "read_file" => string("path"),
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
        "run_command" => clip(&string("command"), 160),
        _ => clip(call.function.arguments.trim(), 160),
    }
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

pub fn change_preview(call: &ToolCall) -> ChangePreview {
    match call.function.name.as_str() {
        "write_file" => write_file_preview(&call.function.arguments),
        "edit_file" => edit_file_preview(&call.function.arguments),
        _ => ChangePreview {
            diff: None,
            note: None,
        },
    }
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
        diff: Some(FileDiff::new(path, &before, &content, !exists)),
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
        diff: Some(FileDiff::new(path, &before, &after, false)),
        note,
    }
}

fn preview_error(error: anyhow::Error) -> ChangePreview {
    ChangePreview {
        diff: None,
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

pub fn execute(call: &ToolCall) -> Result<String> {
    match call.function.name.as_str() {
        "list_dir" => list_dir(&call.function.arguments),
        "read_file" => read_file(&call.function.arguments),
        "write_file" => write_file(&call.function.arguments),
        "edit_file" => edit_file(&call.function.arguments),
        "run_command" => run_command(&call.function.arguments),
        other => bail!("unknown tool `{other}`"),
    }
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
}

fn read_file(arguments: &str) -> Result<String> {
    let args: ReadFileArgs = parse(arguments)?;
    let path = resolve(&args.path);
    let metadata = fs::metadata(&path).with_context(|| format!("reading {}", args.path))?;
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
    use super::{TOOL_NAMES, definitions, preview, requires_approval};
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
    fn only_mutating_tools_need_approval() {
        assert!(!requires_approval("read_file"));
        assert!(!requires_approval("list_dir"));
        assert!(requires_approval("write_file"));
        assert!(requires_approval("edit_file"));
        assert!(requires_approval("run_command"));
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
