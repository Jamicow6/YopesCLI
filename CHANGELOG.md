# Changelog

All notable changes to YopesCLI are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[semantic versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Split `src/app.rs` into `src/app/` modules (input, turns, streaming,
  `/connect`, palette, pickers, images, formatting, modes) and `src/ui.rs` into
  `src/ui/` modules, one per panel. No behaviour change.

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
