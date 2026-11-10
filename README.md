# YopesCLI

A fast, session-first AI harness for the terminal. Built with Rust, Ratatui, and Tokio.

## Highlights

- Streaming OpenAI-compatible and Ollama chat APIs.
- Zero-configuration demo mode with deterministic simulated streaming.
- One focused session, restored automatically between runs.
- Responsive editing, scrollback, cancellation, slash commands, and a help overlay.
- Bounded redraws, no per-token disk writes, and no blocking network work on the UI thread.

## Install on PATH

From this directory:

```sh
cargo install --path .
yopes
```

Cargo installs the executable as `yopes` (`yopes.exe` on Windows) in `$HOME/.cargo/bin`, normally `%USERPROFILE%\.cargo\bin` on Windows. Keep that directory on PATH. Re-run `cargo install --path . --force` after updates.

## Run without installing

```sh
cargo run --release
```

Demo mode starts when no provider is configured. If `OPENAI_API_KEY` exists, OpenAI-compatible mode is selected automatically.

### OpenAI-compatible

```sh
export OPENAI_API_KEY="your-key"
cargo run --release -- --provider openai --model gpt-4o-mini
```

PowerShell:

```powershell
$env:OPENAI_API_KEY = "your-key"
cargo run --release -- --provider openai --model gpt-4o-mini
```

For another compatible endpoint:

```sh
cargo run --release -- \
  --provider openai \
  --base-url https://example.com/v1 \
  --model your-model
```

### Ollama

Start Ollama, pull a model, then run:

```sh
ollama pull llama3.2
cargo run --release -- --provider ollama --model llama3.2
```

`--base-url` defaults to `http://localhost:11434` for Ollama and `https://api.openai.com/v1` for OpenAI-compatible APIs.

## Keys

| Key | Action |
| --- | --- |
| `Enter` | Send the current message |
| `Alt+Enter` / `Shift+Enter` | Insert a newline |
| `Esc` | Stop streaming, or close help |
| `Ctrl+C` / `Ctrl+Q` | Quit |
| `Ctrl+N` | Start a new session |
| `Ctrl+U` | Clear the composer |
| `Ctrl+W` | Delete the previous word |
| `Ctrl+A` / `Ctrl+E` | Move to start / end |
| `Up` / `Down` | Scroll conversation |
| `PageUp` / `PageDown` | Scroll one page |
| `End` | Jump to the latest message |
| `F1` | Toggle help |

Mouse wheel scrolling is supported.

## Commands

- `/help` — show keyboard help.
- `/new` — start a fresh session.
- `/model <name>` — switch model for subsequent messages.
- `/model` — show the active model.
- `/quit` — save and quit.

## Configuration

CLI flags override environment variables.

| CLI | Environment | Default |
| --- | --- | --- |
| `--provider demo\|openai\|ollama` | `YOPES_PROVIDER` | `openai` when `OPENAI_API_KEY` is set, otherwise `demo` |
| `--model <name>` | `YOPES_MODEL` | Provider-specific |
| `--base-url <url>` | `YOPES_BASE_URL`, then `OPENAI_BASE_URL` or `OLLAMA_BASE_URL` | Provider-specific |
| `--session <path>` | `YOPES_SESSION` | Platform-local application data directory |

OpenAI authentication uses `OPENAI_API_KEY`. Keys are read from the environment and are never written to the session file.

Use `--no-save` for an ephemeral session. Session writes happen after submitted prompts and completed, stopped, or failed responses—not for every streamed token.

## Tests

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```

GitHub Actions runs formatting, Clippy, tests, and a locked release build on Linux and Windows through the **On-the-line** workflow. Successful runs provide downloadable `yopes-*` binaries for seven days, so installation can use CI artifacts instead of local compilation.

## Scope

V1 is deliberately session-first: no tool execution, agents, retrieval, eval runner, or session picker. It supports text chat completions only; reasoning fields, images, and tool calls are not rendered yet.

## License

Distributed under GPL-3.0-only. See [LICENSE](LICENSE).

