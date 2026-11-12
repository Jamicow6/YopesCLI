//! Tool policy: which calls need the user's approval, and the system prompt
//! that tells the model how to use the tools.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

use crate::{session::ToolCall, temp};

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
    let temp = temp::platform_temp_root().display().to_string();
    let base = if tools_enabled {
        format!(
            "You are yopes, a coding assistant running in the user's terminal.\n\
             Working directory: {cwd}\n\
             Operating system: {os}\n\
             Scratch files (scripts, downloads, logs) go in {temp}: that is where the user looks for them.\n\
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
        } else if name == "AGENTS.md" {
            // Only the instruction file counts. Collecting every Markdown file
            // would put the README into the system prompt.
            files.push(path);
        }
    }
}
