# YopesCLI

A better coding CLI, Built with Rust, Ratatui, and Tokio.

## Highlights

- Streaming OpenAI-compatible
- A quiet, minimal interface with a popout command palette.
- Provider and model discovery from [models.dev](https://models.dev) with `/connect`.
- One-command self-updates with `yopes update`.
- One focused session, restored automatically between runs.
- Responsive editing, scrollback, cancellation, slash commands, and a help overlay.
- Bounded redraws, no per-token disk writes, and no blocking network work on the UI thread.

## Keys

| Key | Action |
| --- | --- |
| `Enter` | Send the current message, or run the selected palette command |
| `Alt+Enter` / `Shift+Enter` | Insert a newline |
| `/` | Open the command palette below the conversation |
| `Tab` | Complete the selected palette command |
| `Esc` | Close the palette, stop streaming, or close help |
| `Ctrl+C` / `Ctrl+Q` | Quit |
| `Ctrl+N` | Start a new session |
| `Ctrl+U` | Clear the composer |
| `Ctrl+W` | Delete the previous word |
| `Ctrl+A` / `Ctrl+E` | Move to start / end |
| `Up` / `Down` | Move through the palette, or scroll the conversation |
| `PageUp` / `PageDown` | Scroll one page |
| `End` | Jump to the latest message |
| `F1` | Toggle help |

Mouse wheel scrolling is supported. Type `/` to see the available commands in a compact popout box; keep typing to filter them, use `Up` / `Down` to choose, `Tab` to complete, or `Enter` to run.

## Commands

- `/help` — show keyboard help.
- `/new` — start a fresh session.
- `/connect` — browse OpenAI-compatible providers and models from models.dev, then connect.
- `/model <name>` — switch model for subsequent messages.
- `/model` — show the active model.
- `/quit` — save and quit.

### Connecting a provider

Run `/connect` and pick a provider from the popout list. Keep typing to search; use `Up` / `Down` and `Enter` to choose. Next, pick a model from that provider.

When prompted for an API key, you can press `Enter` to use the provider's environment variable (for example `OPENROUTER_API_KEY`) or skip the key. After entering a key, you can optionally set an encryption password:

- Enter a password to encrypt the key with Argon2id + XChaCha20-Poly1305 before it is written to disk.
- Press `Enter` to store the key without encryption.

Encrypted keys prompt for the password when yopes starts. The saved connection lives in the platform configuration directory (`%APPDATA%\Yopes\Yopes\config\config.json` on Windows) and never in the session file.

## Updates

```sh
yopes update
```

The updater checks the latest GitHub release, compares it with the running build, downloads the matching archive and `SHA256SUMS`, verifies the checksum, replaces the executable, and restarts yopes.

## Configuration

CLI flags override environment variables.

| CLI | Environment | Default |
| --- | --- | --- |
| `--provider demo\|openai\|ollama` | `YOPES_PROVIDER` | Saved `/connect` connection, `openai` when `OPENAI_API_KEY` is set, otherwise `demo` |
| `--model <name>` | `YOPES_MODEL` | Provider-specific |
| `--base-url <url>` | `YOPES_BASE_URL`, then `OPENAI_BASE_URL` or `OLLAMA_BASE_URL` | Provider-specific |
| `--session <path>` | `YOPES_SESSION` | Platform-local application data directory |

Provider authentication uses the provider's environment variable (for example `OPENAI_API_KEY` or `OPENROUTER_API_KEY`), or a key saved with `/connect`. Saved connections are applied immediately and restored on the next launch; keys are never written to the session file.

Use `--no-save` for an ephemeral session. Session writes happen after submitted prompts and completed, stopped, or failed responses—not for every streamed token.

## Tests

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```

GitHub Actions uses the **On-the-line** workflow as a compile-only gate on Linux and Windows before the **Production** workflow publishes release builds.

## License

Distributed under GPL-3.0-only. See [LICENSE](LICENSE).

