//! Claude-Code-style skills: `SKILL.md` prompt packs discovered on disk.
//!
//! Locations, in precedence order (the first name wins): `~/.yopes/skills/`,
//! `~/.claude/skills/`, `~/.claude/commands/`, then the same three under the
//! working directory. A skill is a subdirectory holding `SKILL.md`; a bare
//! `.md` file is the older "command" format. Frontmatter is hand parsed — a
//! YAML dependency for six keys is not worth it — so unrecognized keys are
//! ignored and malformed frontmatter leaves the file unloaded as content.
//!
//! The model auto-invokes through the listing `section()` appends to the
//! system prompt; it uses `read_file` on the skill's file when the situation
//! matches, so skills cost nothing until they are used. The user invokes
//! `/name`, and `expand()` turns that into a message carrying the body, so
//! slash invocation works even with tools disabled.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

/// The combined `description` + `when_to_use` line in listings caps here,
/// like the Agent Skills spec says.
const LISTING_CAP: usize = 1536;

#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    /// Loaded from a home location rather than the workspace.
    pub personal: bool,
    /// The model may never auto-load it; the user can still type `/name`.
    pub disable_model_invocation: bool,
    /// Hidden from `/name` invocation; only the model may use it.
    pub user_invocable: bool,
}

struct Location {
    path: PathBuf,
    personal: bool,
}

/// Every installed skill for this workspace, home locations first so a
/// personal skill overrides a project skill of the same name.
pub fn discover(cwd: &Path) -> Vec<Skill> {
    let mut locations = Vec::new();
    if let Some(home) = directories::UserDirs::new().map(|dirs| dirs.home_dir().to_path_buf()) {
        locations.push(Location { path: home, personal: true });
    }
    locations.push(Location { path: cwd.to_path_buf(), personal: false });
    scan(&locations)
}

fn scan(locations: &[Location]) -> Vec<Skill> {
    let mut skills = Vec::new();
    let mut taken = HashSet::new();
    for location in locations {
        // The skill directories follow the Agent Skills layout; commands is
        // the older merged format kept for compatibility with Claude Code.
        for dir in [
            location.path.join(".yopes").join("skills"),
            location.path.join(".claude").join("skills"),
            location.path.join(".claude").join("commands"),
        ] {
            let Ok(entries) = fs::read_dir(&dir) else { continue };
            let mut entries: Vec<(PathBuf, bool)> = entries
                .flatten()
                .filter_map(|entry| {
                    let path = entry.path();
                    let is_dir = path.is_dir();
                    if !is_dir && path.extension().and_then(|ext| ext.to_str()) != Some("md") {
                        return None;
                    }
                    Some((path, is_dir))
                })
                .collect();
            entries.sort();
            for (path, is_dir) in entries {
                // A directory holds its instructions in SKILL.md; a plain
                // `.md` file *is* the skill.
                let file = if is_dir { path.join("SKILL.md") } else { path.clone() };
                let fallback = if is_dir {
                    path.file_name().and_then(|name| name.to_str())
                } else {
                    path.file_stem().and_then(|name| name.to_str())
                };
                let Some(fallback) = fallback else { continue };
                let Ok(text) = fs::read_to_string(&file) else { continue };
                let Some(skill) = parse_skill(&text, fallback, file, location.personal) else {
                    continue;
                };
                if taken.insert(skill.name.to_ascii_lowercase()) {
                    skills.push(skill);
                }
            }
        }
    }
    skills
}

/// Turn the file text into a skill, or None when the name or description is
/// unusable — the model needs both to decide when the skill applies.
fn parse_skill(text: &str, fallback: &str, path: PathBuf, personal: bool) -> Option<Skill> {
    let (fields, body) = frontmatter(text);
    let name = fields.name.clone().unwrap_or_else(|| fallback.to_owned());
    if !valid_name(&name) {
        return None;
    }
    let mut description = fields
        .description
        .clone()
        .unwrap_or_else(|| {
            body.lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .unwrap_or_default()
                .to_owned()
        });
    if let Some(when_to_use) = fields.when_to_use {
        if description.is_empty() {
            description = when_to_use;
        } else {
            description.push(' ');
            description.push_str(&when_to_use);
        }
    }
    if description.is_empty() {
        return None;
    }
    Some(Skill {
        name,
        description: description.chars().take(LISTING_CAP).collect(),
        path,
        personal,
        disable_model_invocation: fields.disable_model_invocation,
        user_invocable: fields.user_invocable,
    })
}

#[derive(Default)]
struct Fields {
    name: Option<String>,
    description: Option<String>,
    when_to_use: Option<String>,
    disable_model_invocation: bool,
    user_invocable: bool,
}

/// Parse the frontmatter rows, or everything as body when the opening `---`
/// is not the first line or the markers never close.
fn frontmatter(text: &str) -> (Fields, String) {
    let mut lines = text.lines();
    if !matches!(lines.next(), Some(first) if first.trim() == "---") {
        return (Fields::default(), text.to_owned());
    }
    let mut fields = Fields::default();
    fields.user_invocable = true;
    let mut body = Vec::new();
    let mut closed = false;
    for line in lines {
        if line.trim() == "---" {
            closed = true;
        } else if closed {
            body.push(line);
        } else {
            let Some((key, value)) = line.split_once(':') else { continue };
            let value = unquote(value.trim());
            match key.trim().to_ascii_lowercase().as_str() {
                "name" if !value.is_empty() => fields.name = Some(value),
                "description" if !value.is_empty() => fields.description = Some(value),
                "when_to_use" if !value.is_empty() => fields.when_to_use = Some(value),
                "disable-model-invocation" => {
                    fields.disable_model_invocation |= parse_bool(&value) == Some(true);
                }
                "user-invocable" => fields.user_invocable = parse_bool(&value).unwrap_or(true),
                _ => {}
            }
        }
    }
    if closed {
        (fields, body.join("\n"))
    } else {
        (Fields::default(), text.to_owned())
    }
}

/// Boolean frontmatter values, any letter case, as the spec allows.
fn parse_bool(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

fn unquote(value: &str) -> String {
    if value.len() >= 2 {
        let mut chars = value.chars();
        let first = chars.next();
        if first == chars.next_back() && matches!(first, Some('"') | Some('\'')) {
            return value[1..value.len() - 1].to_owned();
        }
    }
    value.to_owned()
}

/// Slash-command names stay simple so the palette and the model listing stay
/// readable.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The system-prompt section that lets the model auto-invoke skills. Also
/// invoked by user slash commands: those inject the content instead.
pub fn section(cwd: &Path) -> Option<String> {
    let usable: Vec<Skill> = discover(cwd)
        .into_iter()
        .filter(|skill| !skill.disable_model_invocation)
        .collect();
    if usable.is_empty() {
        return None;
    }
    let mut text = String::from(
        "The user installed skills: one-file procedure packs invoked on demand. \
         When the current situation matches a skill below, use read_file on its \
         file and follow the instructions; say you are using the skill. The user \
         may also type /name directly, which injects the content for you.\n",
    );
    for skill in usable {
        let shown = relative(&skill.path, cwd);
        text.push_str(&format!("- /{} — {} ({shown})\n", skill.name, skill.description));
    }
    Some(text)
}

fn relative(path: &Path, cwd: &Path) -> String {
    // Outside the workspace stays absolute; inside prints relative.
    path.strip_prefix(cwd).unwrap_or(path).display().to_string()
}

/// Turn `/name args` into the message content that carries the skill body,
/// or None when the name is no user-invocable skill and the caller should
/// treat it as a builtin or unknown command.
pub fn expand(prompt: &str, cwd: &Path) -> Option<String> {
    let mut words = prompt.split_whitespace();
    let name = words.next()?.strip_prefix('/')?;
    let args = words.collect::<Vec<_>>().join(" ");
    let skill = discover(cwd)
        .into_iter()
        .find(|skill| skill.user_invocable && skill.name.eq_ignore_ascii_case(name))?;
    let body = frontmatter(&fs::read_to_string(&skill.path).ok()?).1;
    let intro = if args.is_empty() {
        format!("The user invoked the /{} skill. Follow its instructions.", skill.name)
    } else {
        format!(
            "The user invoked the /{} skill with: {args}. Follow its instructions.",
            skill.name
        )
    };
    Some(format!(
        "{intro}\n\nSkill content ({}):\n\n{}",
        relative(&skill.path, cwd),
        body.trim()
    ))
}

/// One-line digest for the `/skills` notice, or None when nothing is found.
pub fn summary(cwd: &Path) -> Option<String> {
    let skills = discover(cwd);
    if skills.is_empty() {
        return None;
    }
    let items = skills
        .iter()
        .map(|skill| {
            let origin = if skill.personal { "personal" } else { "project" };
            let description: String = skill.description.chars().take(80).collect();
            format!("/{} ({origin}) — {description}", skill.name)
        })
        .collect::<Vec<_>>();
    Some(items.join(" · "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("yopes-skills-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn parses_frontmatter_quotes_and_booleans() {
        let (fields, body) = fields(
            "---\nname: deploy\n\"description\": Deploys the app\ndisable-model-invocation: YES\nuser-invocable: off\n---\nBody notes.",
        );
        assert_eq!(fields.name.as_deref(), Some("deploy"));
        assert_eq!(fields.description.as_deref(), Some("Deploys the app"));
        assert!(fields.disable_model_invocation);
        assert!(!fields.user_invocable);
        assert_eq!(body, "Body notes.");

        let (fields, _) = fields("---\ndescription: 'quoted ones'\n---\n.");
        assert_eq!(fields.description.as_deref(), Some("quoted ones"));

        // No frontmatter marker: everything is content, defaults hold.
        let (fields, body) = fields("Plain content.");
        assert!(fields.name.is_none());
        assert!(fields.user_invocable);
        assert_eq!(body, "Plain content.");

        // An unclosed marker means the whole file is content too.
        let (fields, body) = fields("---\nname: never");
        assert!(fields.name.is_none());
        assert_eq!(body, "---\nname: never");
    }

    #[test]
    fn falls_back_to_first_body_line_and_combines_when_to_use() {
        let skill = parse_skill(
            "---\n---\n\nThe deploy skill deploys.\nMore text.",
            "deploy",
            PathBuf::new(),
            false,
        )
        .unwrap();
        assert_eq!(skill.name, "deploy");
        assert_eq!(skill.description, "The deploy skill deploys.");

        let skill = parse_skill(
            "---\ndescription: Deploys.\nwhen_to_use: When the user asks to ship.\n---\n.",
            "deploy",
            PathBuf::new(),
            false,
        )
        .unwrap();
        assert_eq!(skill.description, "Deploys. When the user asks to ship.");

        // Unusable names and empty descriptions refuse to load.
        assert!(parse_skill("---\nname: bad name\n---\nBody", "d", PathBuf::new(), false).is_none());
        assert!(parse_skill("---\n---\n\n   \n", "empty", PathBuf::new(), false).is_none());

        // The combined listing respects the cap.
        let long = "x".repeat(LISTING_CAP + 40);
        let skill = parse_skill(
            format!("---\ndescription: {long}\n---\n.").as_str(),
            "long",
            PathBuf::new(),
            false,
        )
        .unwrap();
        assert_eq!(skill.description.chars().count(), LISTING_CAP);
    }

    #[test]
    fn scans_roots_in_precedence_order_and_dedupes() {
        let dir = temp_dir("scan");
        let home = dir.join("home");
        let work = dir.join("work");
        for root in [&dir, &home, &work] {
            fs::create_dir_all(root).unwrap();
        }
        let write = |path: &Path, text: &str| {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        };
        write(&home.join(".claude/skills/deploy/SKILL.md"), "---\ndescription: Home deploy\n---\nHome body.");
        write(&work.join(".claude/skills/deploy/SKILL.md"), "---\ndescription: Project deploy\n---\nProject body.");
        write(&work.join(".yopes/skills/codex/SKILL.md"), "Codex opens paths.");
        write(&work.join(".claude/commands/commit.md"), "---\ndescription: Commit helper\n---\nCommit body.");
        write(&work.join(".claude/skills/skip me.md"), "Hmm."); // invalid name, dropped

        let locations = [
            Location { path: home.clone(), personal: true },
            Location { path: work.clone(), personal: false },
        ];
        let skills = scan(&locations);

        let names: Vec<&str> = skills.iter().map(|skill| skill.name.as_str()).collect();
        assert_eq!(names, ["deploy", "codex", "commit"]);
        // The home skill wins the `/deploy` slot and carries its text.
        assert_eq!(skills[0].path, home.join(".claude/skills/deploy/SKILL.md"));
        assert_eq!(skills[0].description, "Home deploy");
        assert!(skills[0].personal);
        assert_eq!(skills[2].path, work.join(".claude/commands/commit.md"));
        assert_eq!(skills[2].description, "Commit helper");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn section_excludes_model_disabled_and_expand_carries_the_body() {
        let dir = temp_dir("invoke");
        let work = dir.join("work");
        let write = |path: &Path, text: &str| {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        };
        write(
            &work.join(".yopes/skills/hidden/SKILL.md"),
            "---\ndescription: Never auto\ndisable-model-invocation: true\n---\nHidden body.",
        );
        write(&work.join(".yopes/skills/open/SKILL.md"), "---\ndescription: Open sail\n---\nOpen body.");

        let section = section(&work).unwrap();
        assert!(section.contains("/open"), "{section}");
        assert!(!section.contains("/hidden"), "{section}");

        assert!(expand("/missing", &work).is_none());
        // A skill the model may not auto-load is still user invocable.
        let expanded = expand("/hidden", &work).unwrap();
        assert!(expanded.contains("Hidden body."), "{expanded}");
        let expanded = expand("/open extra stuff", &work).unwrap();
        assert!(expanded.contains("Open body."), "{expanded}");
        assert!(expanded.contains("extra stuff"), "{expanded}");

        let _ = fs::remove_dir_all(&dir);
    }
}
