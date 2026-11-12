use super::*;

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

pub(super) fn glob(arguments: &str) -> Result<String> {
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

pub(super) fn glob_matches(segments: &[String], path: &[String]) -> bool {
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
pub(super) fn glob_segment(pattern: &[char], text: &[char]) -> bool {
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
pub(super) fn grep(arguments: &str) -> Result<String> {
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

