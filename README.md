# YopesCLI

A better coding CLI, Built with Rust, Ratatui, and Tokio.

## Highlights

- Streaming OpenAI-compatible providers, including custom endpoints.
- A quiet, minimal interface with a popout command palette.
- Provider and model discovery from [models.dev](https://models.dev) plus custom endpoints with `/connect`.
- One-command self-updates with `yopes update`.
- One focused session, restored automatically between runs.
- Responsive editing, scrollback, cancellation, slash commands, and a help overlay.
- Bounded redraws, no per-token disk writes, and no blocking network work on the UI thread.

## Install

Download the latest build for your platform from [Releases](https://github.com/Jamicow6/YopesCLI/releases). To build or run from a checkout, see [CONTRIBUTING.md](CONTRIBUTING.md).

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
- `/connect` — browse models.dev providers or configure a custom OpenAI-compatible endpoint.
- `/model <name>` — switch model for subsequent messages.
- `/model` — show the active model.
- `/quit` — save and quit.

### Connecting a provider

Run `/connect`. **Custom** sits above the models.dev providers, separated by a divider. Choose it to connect any OpenAI-compatible endpoint:

1. Enter the completion endpoint—a base URL or the full `/chat/completions` URL—for example `https://api.example.com/v1`.
2. Choose the authorization type: `Bearer token`, `x-api-key`, or `No auth`.
3. Enter the token when the authorization type requires one.
4. Enter a model ID and an optional display name. On the model list, press `a` to add another model, then choose the model to use and save.

Reconnecting a custom endpoint pre-fills its endpoint, authorization, and models. Re-enter the token to update it; choosing `No auth` removes the saved secret. Custom setup remains available when models.dev cannot be reached.

For models.dev providers, pick a provider, then a model. Keep typing to search; use `Up` / `Down` and `Enter` to choose. When prompted for an API key, you can press `Enter` to use the provider's environment variable (for example `OPENROUTER_API_KEY`) or skip the key.

After entering a key or token, you can optionally set an encryption password:

- Enter a password to encrypt the secret with Argon2id + XChaCha20-Poly1305 before it is written to disk.
- Press `Enter` to store the secret without encryption.

Encrypted secrets prompt for the password when yopes starts. The saved connection lives in the platform configuration directory (`%APPDATA%\Yopes\Yopes\config\config.json` on Windows) and never in the session file.

## Updates

```sh
yopes update
```

The updater checks the latest GitHub release, compares it with the running build, downloads the matching archive and `SHA256SUMS`, verifies the checksum, replaces the executable, and restarts yopes.

## Configuration

CLI flags override environment variables.

| CLI | Environment | Default |
| --- | --- | --- |
| `--provider openai\|ollama` | `YOPES_PROVIDER` | Saved `/connect` connection, `openai` when `OPENAI_API_KEY` is set, otherwise unconfigured (run `/connect`) |
| `--model <name>` | `YOPES_MODEL` | Provider-specific |
| `--base-url <url>` | `YOPES_BASE_URL`, then `OPENAI_BASE_URL` or `OLLAMA_BASE_URL` | Provider-specific, or the endpoint saved by `/connect` |
| `--session <path>` | `YOPES_SESSION` | Platform-local application data directory |

Models.dev providers use Bearer authentication with the provider's environment variable (for example `OPENAI_API_KEY` or `OPENROUTER_API_KEY`) or a key saved with `/connect`. Custom providers use the authorization type, token, and models saved with `/connect`. Saved connections are applied immediately and restored on the next launch; keys and tokens are never written to the session file.

Use `--no-save` for an ephemeral session. Session writes happen after submitted prompts and completed, stopped, or failed responses—not for every streamed token.

## License

Distributed under GPL-3.0-only. See [LICENSE](LICENSE).

