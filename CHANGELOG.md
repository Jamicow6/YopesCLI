# Changelog

All notable changes to YopesCLI are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[semantic versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Rich Markdown rendering in the transcript: headings, emphasis, links, fenced
  code with language labels, nested lists and task boxes, blockquotes, rules,
  and compact tables. The system prompt now asks the model for terminal-friendly
  Markdown.
- Per-provider key storage: API keys are saved per endpoint and reused across
  connections, so connecting a second time never asks for the key again.
- `/disconnect [name]` forgets a provider's stored key and model list, and
  resets the active connection when it matches.
- `/version` shows the version the app is running.
- `/model` switches persist with the connection.
- Provider logos from models.dev, rasterised and cached on disk, drawn above
  the connect list.
- An animated mascot: flicking tail, blinking, and a rolling ball of yarn.
- Visible retries: a footer line shows the attempt, the delay, and the reason
  while a failed request retries (bounded, honouring `Retry-After`).
- Read-only commands work while a response is streaming.
- The system prompt tells the model where scratch files belong
  (`%USERPROFILE%\Temp` on Windows).
- Help popup scrolls with ↑↓/PgUp/PgDn and the mouse wheel.
- `Ctrl+G` toggles mouse capture: with capture off the terminal's own text
  selection and copy work over the transcript.

### Changed

- Split `src/app.rs` into `src/app/` modules (input, turns, streaming,
  `/connect`, palette, pickers, images, formatting, modes) and `src/ui.rs` into
  `src/ui/` modules, one per panel. No behaviour change.
- Only `AGENTS.md` files feed the project instructions; other Markdown files
  in the workspace are no longer sent to the model.
- Palette rows pad by display width, so CJK text and emoji keep their columns.
- The tool frame caret stays inside the frame's bottom border.
- `/tools` is deprecated and marked as such; it still works.
- Releases build for Linux and Windows only; the hosted macOS runners
  starved the publish job, so the macOS archive is gone until capacity
  improves. macOS still works from a checkout (`cargo build --release`).

## [0.1.0]

First public release.

### Added

- Streaming chat against OpenAI-compatible endpoints and Ollama, with custom
  base URLs.
- Provider and model discovery from [models.dev](https://models.dev), plus the
  Fasfas model list, through `/connect`.
- Tool calling with per-tool approval and a diff viewer: `list_dir`,
  `read_file`, `glob`, `grep`, `write_file`, `edit_file`, `multi_edit`,
  `run_command`, `bash_background`, `bash_output`, `bash_kill`, `web_search`,
  `web_fetch`, `todo_write`, and `delegate`.
- Tool modes `on`, `off`, `auto`, and `ask`, plus build, plan, and chat turn
  modes.
- A native task list in a Progress panel, saved with the session.
- `SKILL.md` procedure packs in the Agent Skills format, discovered from
  `.yopes/skills`, `~/.yopes/skills`, and the Claude Code skill directories.
- One focused session, restored between runs, with `/sessions` to switch and
  `/compact` to summarize older turns.
- Slash commands, a popout command palette, a help overlay, scrollback, and
  cancellation.
- `/effort` reasoning-effort control, saved with the connection.
- `yopes update` self-update.

[Unreleased]: https://github.com/Jamicow6/YopesCLI/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/Jamicow6/YopesCLI/releases/tag/v0.1.0
