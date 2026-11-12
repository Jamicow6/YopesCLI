use std::{collections::HashMap, fs};

use super::*;

#[derive(Deserialize)]
struct ListDirArgs {
    #[serde(default)]
    path: Option<String>,
}

pub(super) fn list_dir(arguments: &str) -> Result<String> {
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
struct ReadImageArgs {
    path: String,
}

/// Validate an image and report it; the app reads the pixels itself and sends
/// them as a real image part, which a text tool result cannot carry.
pub(super) fn read_image(arguments: &str) -> Result<String> {
    let args: ReadImageArgs = parse(arguments)?;
    let path = resolve(&args.path);
    let metadata = fs::metadata(&path).with_context(|| format!("reading {}", args.path))?;
    if !metadata.is_file() {
        bail!("{} is not a file", args.path);
    }
    if metadata.len() > MAX_READ_IMAGE_BYTES {
        bail!(
            "{} is too large ({} bytes; the limit is {MAX_READ_IMAGE_BYTES})",
            args.path,
            metadata.len()
        );
    }
    let (width, height) = image::image_dimensions(&path)
        .with_context(|| format!("{} is not a PNG or JPEG image", args.path))?;
    Ok(format!(
        "{} ({width}x{height}) — the image is attached to this result.",
        args.path
    ))
}

#[derive(Deserialize)]
struct ReadFileArgs {
    path: String,
    #[serde(default)]
    start_line: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}
pub(super) fn read_file(arguments: &str) -> Result<String> {
    let args: ReadFileArgs = parse(arguments)?;
    let path = resolve(&args.path);
    let metadata = fs::metadata(&path).with_context(|| format!("reading {}", args.path))?;
    if metadata.is_dir() {
        if args.start_line.is_some() || args.limit.is_some() {
            bail!(
                "{} is a directory; line ranges only apply to files",
                args.path
            );
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
pub(super) struct WriteFileArgs {
    pub(super) path: String,
    pub(super) content: String,
}

pub(super) fn write_file(arguments: &str) -> Result<String> {
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
pub(super) struct EditFileArgs {
    pub(super) path: String,
    pub(super) old_text: String,
    pub(super) new_text: String,
    #[serde(default)]
    pub(super) replace_all: bool,
}

pub(super) fn edit_file(arguments: &str) -> Result<String> {
    let args: EditFileArgs = parse(arguments)?;
    if args.old_text.is_empty() {
        bail!("old_text cannot be empty");
    }
    let path = resolve(&args.path);
    let source = fs::read_to_string(&path).with_context(|| format!("reading {}", args.path))?;
    let Some((updated, matches)) =
        apply_text_edit(&source, &args.old_text, &args.new_text, args.replace_all)
    else {
        bail!("old_text was not found in {}", args.path);
    };
    if matches > 1 && !args.replace_all {
        bail!(
            "old_text matches {matches} times in {}; add more context or set replace_all to true",
            args.path
        );
    }
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

pub(super) fn multi_edit(arguments: &str) -> Result<String> {
    let edits = serde_json::from_str::<MultiEditArgs>(arguments)?
        .edits
        .len();
    let updates = multi_edit_replay(arguments)?;
    for (path, _, after) in &updates {
        fs::write(resolve(path), after.as_bytes()).with_context(|| format!("writing {path}"))?;
    }
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
    if args
        .edits
        .iter()
        .map(|edit| edit.new_text.len())
        .sum::<usize>()
        > MAX_WRITE_BYTES
    {
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
                format!(
                    "reading {}; multi_edit only edits existing files",
                    edit.path
                )
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
    let Some((updated, matches)) =
        apply_text_edit(source, &edit.old_text, &edit.new_text, edit.replace_all)
    else {
        bail!("old_text was not found in {}", edit.path);
    };
    if matches > 1 && !edit.replace_all {
        bail!(
            "old_text matches {matches} times in {}; add more context or set replace_all",
            edit.path
        );
    }
    Ok(updated)
}

/// Replacement that tolerates CRLF/LF differences between the caller's text
/// and the file: both sides are compared after normalizing to LF, inserted
/// text adopts the file's line endings, and only matched spans are rewritten
/// so untouched lines keep their exact bytes.
pub(super) fn apply_text_edit(
    source: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Option<(String, usize)> {
    // Byte-exact text covers the matching-endings case with no extra work.
    if source.contains(old) {
        let matches = source.matches(old).count();
        let updated = if replace_all {
            source.replace(old, new)
        } else {
            source.replacen(old, new, 1)
        };
        return Some((updated, matches));
    }
    // Endings can only be why a multi-line needle misses.
    if !old.contains('\n') {
        return None;
    }
    let (flat_source, map, crlf) = {
        let mut text = String::with_capacity(source.len());
        let mut map = Vec::with_capacity(source.len());
        for (index, ch) in source.char_indices() {
            // Windows text drops every '\r'; the '\n' below it stays, so CRLF
            // reads as LF. map holds the original byte offset of every kept
            // byte, so normalized byte ranges slice back into the source.
            if ch == '\r' {
                continue;
            }
            for _ in 0..ch.len_utf8() {
                map.push(index);
            }
            text.push(ch);
        }
        (text, map, source.contains("\r\n"))
    };
    let flat_old = old.replace("\r\n", "\n").replace('\r', "\n");
    if !flat_source.contains(&flat_old) {
        return None;
    }
    let new_text = {
        let flat_new = new.replace("\r\n", "\n").replace('\r', "\n");
        if crlf {
            flat_new.replace('\n', "\r\n")
        } else {
            flat_new
        }
    };
    let mut updated = String::with_capacity(source.len());
    let mut copied = 0;
    let mut matches = 0;
    for (at, matched) in flat_source.match_indices(&flat_old) {
        let start = map[at];
        let end = if at + matched.len() == map.len() {
            source.len()
        } else {
            map[at + matched.len()]
        };
        updated.push_str(&source[copied..start]);
        updated.push_str(&new_text);
        copied = end;
        matches += 1;
        if !replace_all {
            break;
        }
    }
    updated.push_str(&source[copied..]);
    Some((updated, matches))
}

pub(super) fn multi_edit_preview(arguments: &str) -> ChangePreview {
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

pub(super) fn multi_edit_applied(arguments: &str, targets: &[(String, bool)]) -> Vec<FileDiff> {
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
                if let Some((text, _)) =
                    apply_text_edit(&before, &edit.new_text, &edit.old_text, edit.replace_all)
                {
                    before = text;
                }
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
    let seen = |path: &str, targets: &mut Vec<(String, bool)>| {
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
