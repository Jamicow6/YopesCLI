# Architecture

YopesCLI is a single-binary terminal UI. One `App` owns all state, one event
loop owns all I/O, and rendering is a pure function of `App`.

## Module map

| Module | Responsibility |
| --- | --- |
| `main.rs` | CLI subcommand dispatch, the `tokio::select!` event loop, `dispatch` (turns an `Action` into a spawned task). |
| `clipboard.rs` | Platform-gated clipboard reads (image first, then text). |
| `terminal.rs` | `TerminalGuard` (raw mode, alternate screen, RAII restore) and the Ctrl+C interrupt future. |
| `app/` | The state machine. `mod.rs` holds `App` and the live-request state; `action.rs` holds `Action` and the report types, `commands.rs` the slash-command table, `input.rs` the composer buffer, `keys.rs` key routing, `turn.rs` the turn lifecycle, `stream.rs` streaming events, `connect/` the `/connect` wizard (`state.rs` the stage machine, `keys.rs`/`text.rs`/`finish.rs` its handlers), plus `pickers.rs`, `palette.rs`, `images.rs`, `format.rs`, and `mode.rs`. |
| `ui/` | Rendering only. Reads `&App`, writes ratatui frames. `mod.rs` holds `draw`, the palette constants, and text helpers; `transcript.rs`, `composer.rs`, `pickers.rs`, `approval.rs`, `connect.rs`, and `help.rs` draw one panel each; `markdown/` renders Markdown (`mod.rs` blocks, `inline.rs` spans, `table.rs` tables). |
| `backend/` | HTTP client for OpenAI-compatible and Ollama endpoints: `mod.rs` (`Backend`, `StreamEvent`), `messages.rs` request bodies, `retry.rs` backoff, `sse.rs` stream parsing. |
| `config/` | CLI arguments (`cli.rs`), `Config` and the provider types (`mod.rs`), the saved-config JSON and persistence (`saved.rs`). |
| `credentials.rs` | Sealing and opening API keys (argon2 + chacha20poly1305). |
| `catalog.rs` | Provider and model discovery from models.dev. |
| `session.rs` | `Session`, `Message`, `ToolCall`, save/load, token estimation, compaction. |
| `diff.rs` | Line diffing, hunks, and side-by-side rows. |
| `skills.rs` | `SKILL.md` discovery, frontmatter parsing, slash expansion. |
| `tools/` | The tool surface: `definitions.rs` schemas, `policy.rs` approval rules and system prompt, `preview.rs` change previews, `mod.rs` registry and execution, one module per family (`files`, `search`, `shell`, `web`, `todo`). |
| `update.rs` | `yopes update` self-update. |

## Data flow

```
terminal event ──► App::handle_key ──► Action
                                        │
                                   dispatch()  (main.rs)
                                        │
                        ┌───────────────┼───────────────┐
                        ▼               ▼               ▼
                   Backend::stream   tools::execute   catalog::fetch
                        │               │               │
                        ▼               ▼               ▼
                   StreamEvent      ToolEvent        FetchResult
                        └───────────────┼───────────────┘
                                        ▼
                              App::on_stream_event
                              App::on_tool_result
                              App::on_catalog
                                        │
                                        ▼
                                     Action ──► dispatch() again
```

`App` never performs I/O. It returns an `Action`; `main` performs the work on a
task and feeds the result back through a channel. This keeps the UI thread free
and makes every state transition testable without a terminal.

## Redraw policy

Redrawing re-lays-out the whole transcript, so streaming text redraws at a fixed
cadence (`STREAM_REDRAW_MILLIS`) while user input and lifecycle events get an
immediate frame. See the loop in `main.rs`.

## Where to add things

- **A tool** — see [ADDING-A-TOOL.md](ADDING-A-TOOL.md).
- **A slash command** — add a `CommandSpec` to the palette table in
  `app/commands.rs`, then handle it in `App::handle_key`'s command branch in
  `app/keys.rs`.
- **A provider** — `ProviderKind` in `config/mod.rs`, then the request/parse
  branch in `backend/`.
- **A UI panel** — a `draw_*` function in the matching `ui/` module, called
  from `ui::draw` in `ui/mod.rs`. Rendering helpers shared between panels live
  in `ui/mod.rs` so submodules reach them through `use super::*`.

Both `app/`, `ui/`, `backend/`, `config/`, and `tools/` follow one rule:
shared types, constants, and imports stay in the folder's `mod.rs`, and each
submodule imports them with `use super::*`. New methods on `App` are
`pub(crate)`; free functions in a submodule are `pub(super)` unless nothing
outside that module calls them.

## Testing

Every module except `main.rs` and `ui/`'s drawing layer carries `#[cfg(test)]`
unit tests. `main.rs` is the event loop and is exercised by hand; the `ui/`
tests cover the pure helpers (wrapping, diff layout, frame widths).

CI runs `cargo fmt --check`, `cargo clippy -- -D warnings`, and `cargo test` on
Linux and Windows. Agents must not compile on the host machine — see
[AGENTS.md](../AGENTS.md).
