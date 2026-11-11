# YopesCLI

A better coding CLI, Built with Rust, Ratatui, and Tokio.

## Highlights

- Streaming OpenAI-compatible providers, including custom endpoints.
- Tool calling for reading, writing, editing, and running commands, with per-tool approval and a diff viewer.
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
| `Esc` | Close the palette, stop streaming, deny a pending tool, or close help |
| `Enter` | Run the pending tool when the approval dialog is open |
| `a` | Always allow the pending tool for this session |
| `v` | Toggle unified / side-by-side diffs (approval dialog, or an empty composer) |
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
- `/tools` — show the current tool mode.
- `/tools on|off|auto|ask` — change how tool calls are approved.
- `/quit` — save and quit.

### Tools

Yopes offers tools to OpenAI-compatible providers (`openai` and `custom`): `list_dir`, `read_file`, `write_file`, `edit_file`, `run_command`, `web_search`, `todo_write`, and `delegate` — the last one hands a self-contained subtask to a subagent on the same provider and model; it has no tools and returns its final answer as the tool result, with an approval prompt before it runs. Ollama sessions remain text-only for now.

Tool modes:

- `on` (default) — reads run automatically; writes and commands ask for approval.
- `off` — tools are not offered to the model.
- `auto` — every tool call runs automatically.
- `ask` — every tool call asks for approval.

When a tool needs approval, yopes shows its arguments and, for `write_file` and `edit_file`, a diff of the change. The default view is a unified diff with line numbers, hunk headers, red deletions, and green insertions. Press `v` for a side-by-side view. Press `Enter` to run the tool, `a` to always allow that tool for the current session, or `Esc` to deny it. Denials and errors are returned to the model, and the model continues the turn after each tool result.

### Connecting a provider

Run `/connect`. **Fasfas AI** sits above everything under a `preferred` divider — a temporary promotion that may be removed in a future release. Choosing it shows the notice to get a key: join the [Discord](https://discord.gg/vvndVbBZRe), then press `Enter`, paste your Fasfas key, and yopes fetches the available models from `https://ai.coffeevps.com/v1/models` as OpenAI-compatible bearer auth.

**Custom** sits below it, separated by a divider. Choose it to connect any OpenAI-compatible endpoint:

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

The updater checks the latest GitHub release, compares it with the running build, downloads the matching archive and `SHA256SUMS`, verifies the checksum, replaces the executable, stops older yopes instances, and starts the new one.

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

