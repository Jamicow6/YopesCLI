# YopesCLI

A better coding CLI, built with Rust, Ratatui, and Tokio.

## Highlights

- Streaming OpenAI-compatible providers, including custom endpoints.
- Tool calling for reading, writing, editing, and running commands, with per-tool approval and a diff viewer.
- A quiet, minimal interface with a popout command palette.
- Provider and model discovery from [models.dev](https://models.dev) plus custom endpoints with `/connect`.
- One-command self-updates with `yopes update`.
- One focused session, restored automatically between runs.

## Install

Download the latest build for your platform from [Releases](https://github.com/Jamicaw56/YopesCLI/releases): `yopes-Windows-X64.zip` (Windows) or `yopes-Linux-X64.tar.gz` (Linux), then install it with PowerShell on Windows:

```powershell
irm https://raw.githubusercontent.com/Jamicaw56/YopesCLI/main/install.ps1 | iex
```

The script picks the latest release, verifies its SHA256SUMS checksum, installs `yopes.exe` to `%LOCALAPPDATA%\Programs\yopes`, and adds it to your user PATH. To build or run from a checkout, see [CONTRIBUTING.md](CONTRIBUTING.md).

## First run

1. `/connect` — pick a provider and model. Keys are stored per endpoint, so connecting a second time never asks for the key again.
2. Type a message. Tools ask for approval before they change anything; press `Enter` to allow one, `a` for the session, `Esc` to deny.
3. `/` opens the command palette; `F1` shows keyboard help.

## Docs

The [wiki](https://github.com/Jamicaw56/YopesCLI/wiki) holds the detail:

- [Commands](https://github.com/Jamicaw56/YopesCLI/wiki/Commands) — every slash command.
- [Keys](https://github.com/Jamicaw56/YopesCLI/wiki/Keys) — the full keyboard and mouse map.
- [Connect](https://github.com/Jamicaw56/YopesCLI/wiki/Connect) — providers, key storage, `/disconnect`.
- [Tools](https://github.com/Jamicaw56/YopesCLI/wiki/Tools) — the tool surface, approval modes, plan mode.
- [Skills](https://github.com/Jamicaw56/YopesCLI/wiki/Skills) — `SKILL.md` procedure packs.
- [Configuration](https://github.com/Jamicaw56/YopesCLI/wiki/Configuration) — flags, environment, config file schema.
- [Troubleshooting](https://github.com/Jamicaw56/YopesCLI/wiki/Troubleshooting) — common problems.
- [Architecture](https://github.com/Jamicaw56/YopesCLI/wiki/Architecture) — the module map.

In the checkout: [Architecture](docs/ARCHITECTURE.md), [Adding a tool](docs/ADDING-A-TOOL.md), [Changelog](CHANGELOG.md), [Security policy](SECURITY.md).

## Tests

Run `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` before a pull request; CI runs the same checks on Linux and Windows. See [CONTRIBUTING.md](CONTRIBUTING.md#tests).

## License

Distributed under GPL-3.0-only. See [LICENSE](LICENSE).
