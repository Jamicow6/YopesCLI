# TODO

Working list for the YopesCLI improvement pass. All work is one branch, one
PR: `feature/provider-keys`, **PR #27** (`gh pr view 27`). Continue from the
"Resume here" section at the bottom.

Legend: `[ ]` open · `[~]` in progress, uncommitted · `[x]` done

## Resume here — state at handoff

Branch `feature/provider-keys` (off `main`), pushed to `origin`, PR #27 open.
Last pushed commits: `31950e7 fix(ui) - Put the caret inside the tool frame,
scroll the help, and unstick the PR` and `882e2d8 fix(logos) - Correct the
pixel colour type and the borrowed logo id`. The working tree is clean —
everything below is on the PR.

What `31950e7` contains (all previously uncommitted work, now on origin):

- `.github/workflows/on-the-line.yml` — Windows jobs (Test/Compile matrix)
  run **only on push to main**; PRs run ubuntu-only. Reason: hosted Windows
  runners were never picked up, so PR runs hung queued and had to be
  cancelled. Verified working: the PR run now shows no Windows jobs.
- `src/app/keys.rs` — fixed the dead `Ctrl+V` arm: `KeyCode::Char('v')` with
  CONTROL was unreachable (the CONTROL match at the top of `handle_key`
  returns first). `Ctrl+V => Action::PasteImage` now lives inside the CONTROL
  match. Also: ↑↓/PgUp/PgDn/Home scroll the help popup while it is open, and
  mouse wheel scrolls it too (`handle_mouse`, help branch before transcript).
- `src/app/mod.rs` — new `App.help_scroll: usize` field (init 0).
- `src/ui/help.rs` — `draw_help` now takes `&mut App`, clamps
  `app.help_scroll` to `lines.len() - visible`, renders with
  `.scroll((app.help_scroll as u16, 0))`, hint says
  `esc close · ↑↓ scroll`, added `ctrl+v paste an image` row.
- `src/ui/mod.rs` — call site updated: `draw_help(frame, app, area)`
  (signature change would not have compiled otherwise).
- `src/ui/transcript.rs` — caret fix (issue #5 finding 3): for `Role::Tool`
  the caret no longer lands on the frame's bottom border; it lands on
  `lines[len-2]` (last body line inside the frame) when the last line starts
  with `╰`, else on the last line (narrow-terminal unframed case).
- `src/tools/mod.rs` — `collect_agents_files` now collects only `AGENTS.md`
  (was: every `.md`, so the README went into the system prompt).

What `882e2d8` fixed (first CI run of the above): `logos.rs::pixel_color`
takes `Option<PremultipliedColorU8>` + `.demultiply()` (Pixmap::pixel returns
premultiplied), and `ui/connect.rs::selected_logo_id` returns
`Option<String>` (it borrowed a local `Vec`).

First action on resume: `gh pr checks 27` — the run for `882e2d8` was
in flight at handoff. If clippy still fails, read it with
`gh run view --job <id> --log-failed` and fix; the known past error classes
are listed at the bottom of this file.

## Batch A — Provider keys, connection lifecycle — all done, on the PR

- [x] A1 Per-provider secret storage: `SavedConfig.provider_keys:
  BTreeMap<String, SavedProvider>` keyed by base URL (`src/config.rs`).
- [x] A2 Legacy fallback: no key in the map → falls back to top-level
  `secret` when `base_url` matches. No version bump, `CONFIG_VERSION` stays 1.
- [x] A3 Stored key reused in `/connect`; encrypted key asks only for the
  password (new `ConnectStage::Unlock` + `handle_connect_unlock`).
- [x] A4 Fasfas (preferred) reuses its stored key; notice → key stage skips
  typing when a plaintext key is stored.
- [x] A5 Model switch persists: `/model` and the model picker call
  `persist_model` → `config::save_model`.
- [x] A6 `/disconnect [name]` forgets key + models, resets active connection
  if it matches (`app/turn.rs::disconnect`, `config::disconnect_provider`).
- [x] A7 "Change API key" row in the catalog Models list
  (`EDIT_KEY_ID = "edit-key"` in `app/mod.rs`).
- [x] A8 `/version` prints `crate::update::current_version()`.

## Batch B — Reliability — all done, on the PR

- [x] B1 Bounded retries: `MAX_ATTEMPTS = 5` in `src/backend.rs
  send_with_retry` (was: infinite loop).
- [x] B2 No `eprintln!` from the request path (was landing on the TUI).
- [x] B3 Retry visible: new `StreamEvent::Retrying { attempt, delay_secs,
  reason }` → `App.retry: Option<RetryNotice>` → footer line
  `retrying in {n}s · attempt {k} · {reason}` in `ui/composer.rs`.
- [x] B4 Honours `Retry-After` (`retry_after()`, clamped 1..=60s).
- [x] B5 Read-only commands work while streaming:
  `command_is_safe_while_streaming` in `app/turn.rs` (help, model, tools,
  system, effort, skills, version, quit); refused command is put back into the
  input with `self.input.set(prompt)`.

## Batch C — Viewing experience — mostly done, on the PR

- [x] C1 Mode indicator: coloured dot in the header next to `yopes`
  (`Mode::color()` in `app/mode.rs`: Build `Rgb(175,122,255)` purple, Chat
  `Rgb(214,168,84)` yellow-brown, Plan `Rgb(196,132,90)` brighter brown).
- [x] C2 Focus loss: `App.focused`, `set_focused()`, `Event::FocusGained/Lost`
  handled in `main.rs`; ticker guard is `if app.focused && (...)`.
- [x] C3 Mouse move no redraw: `if mouse.kind != MouseEventKind::Moved`
  in `main.rs` (needed the `MouseEventKind` import).
- [ ] C4 Width-aware padding (`{:<width$}` counts chars, CJK/emoji break the
  columns) in `src/ui/pickers.rs` + `src/ui/connect.rs` — **open**.
- [x] C5 Caret inside the tool frame — uncommitted, see "Resume here".
- [x] C6 Help popup scrolls — uncommitted, see "Resume here".
- [ ] C7 Transcript scroll u16 ceiling: `app.scroll.min(u16::MAX as usize)`
  still in `src/ui/transcript.rs` (~line 195+ area and `ui/composer.rs`
  input scroll). Fix: keep scroll as usize and only clamp at render, or
  switch the rendered offset math to usize. **open**. Also "click map must
  agree with what is painted" — verify `output_rows` mapping after any change.
- [x] C8 Only `AGENTS.md` in the system prompt — uncommitted, see above.

## Batch D — Documentation — open

- [ ] D1 Trim README.md to a short tour, move detail to the wiki. **open**.
- [ ] D2 Create the GitHub wiki. Wiki exists but is empty:
  `https://github.com/Jamicow6/YopesCLI.wiki.git` (branch `master`, only
  `Home.md`, 31 bytes). Plan: clone to
  `%TEMP%\yopes-wiki` (use `$env:TEMP\opencode` when on this harness), write
  pages: Home, Commands (all 13 in `COMMANDS`), Connect + providers (key
  storage, /disconnect), Tools reference, Skills, Configuration file schema
  (incl. `provider_keys`), Troubleshooting, Architecture (modules map).
  Push to the wiki remote. **open**.
- [ ] D3 Deprecate `/tools`: keep it working, remove from the palette/
  COMMANDS advertising or mark description "(deprecated)". **open**.
- [ ] D4 CHANGELOG entries per batch. Check `CHANGELOG.md` exists and its
  format first. **open**.

## Batch E — Provider icons, mascot, temp files — mostly done, on the PR

- [x] E1 models.dev logos: `src/logos.rs` fetches
  `https://models.dev/logos/<id>.svg` (NOT `/icons/`, that's an HTML SPA),
  rasterises via `usvg` 0.50 + `resvg` 0.50 + `tiny-skia` 0.12
  (default-features off, in Cargo.toml; Cargo.lock updated), renders half-block
  `▄` rows at `LOGO_COLS = 6`, caches on disk in
  `<config dir>/logos/<safe-id>.logo` (custom hex encode/decode). App wiring:
  `App.logos: Logos`, `pending_logos` queue, `Action::FetchLogo`,
  `on_logo`, `request_visible_logos` after catalog load, main-loop dispatch
  after `catalog_receiver.recv()`. UI: logo band above the connect list
  (`ui/connect.rs`, `selected_logo_id`). `ConnectState.provider_id` made pub.
- [x] E2 Mascot: `mascot_frame(step)` in `ui/mod.rs` — tail flicks
  (`TAILS`), blink every 40 steps, yarn `(o)` rolls out and back with a `~`
  thread, all rows padded to `MASCOT_WIDTH = 15`. Replaced `CAT_BASE`/
  `YARN_ROLL` in `ui/transcript.rs`. Has a test
  (`the_mascot_animates_and_keeps_its_width`).
- [x] E3 Temp files: `src/temp.rs` — `temp_root()` is
  `%USERPROFILE%\Temp` on Windows (env USERPROFILE), else `env::temp_dir()`;
  creates on demand. `unique_path(prefix, ext)`, `unique_dir(prefix)`.
  Used by `tools::temporary_log_path()` and `update.rs` (download dir).
  Registered as `mod temp;` in `main.rs`.
- [ ] E4 "Report the temp dir to the model" — **open**: tell the model in the
  system prompt what scratch dir to use (see `src/tools/mod.rs` base prompt
  builder around line 130).

## Still open from the user's original list

- [ ] Mouse text selection + Ctrl+C copy / Ctrl+V paste of *text* (item 17).
  Status: Ctrl+V image paste works (dead arm fixed). Mouse capture is always
  on (`EnableMouseCapture` in `TerminalGuard::new` in `main.rs`), so native
  terminal selection is blocked. Options: (a) in-app selection model — drag
  with `MouseEventKind::Drag`/`Down`/`Up` over transcript rows, copy selection
  to clipboard on Ctrl+C (needs a clipboard write; there is `read_clipboard`
  in main.rs to mirror); (b) toggle mouse capture with a key so the terminal's
  native selection works. Not started.
- [ ] CI green on the PR (linux-only now). Run after the uncommitted work is
  pushed. Known past failures already fixed: E0308 in `turn.rs` (or_else
  borrow), E0004 non-exhaustive `ConnectStage::Unlock` match, E0382 moved
  `unlocked`, E0507 `stored_secret` move via `is_some_and` (use `as_ref()`),
  `no rules expected }` in `tokio::select!` (guard needs `=>`), unreachable
  `Some(Ok(_))` arm in main.rs event match, dead `temp::remove_dir_all`
  (removed), logos.rs pixel type (`pixel()` returns `Option<ColorU8>`, already
  demultiplied — no `.demultiply()` needed) and `Transform::from_scale(f32)`.

## Notes / facts for continuation

- No host compilation (AGENTS.md policy): no `cargo build/check/run/test`
  locally. `cargo fmt --all` is the only local gate (it parses). CI checks:
  fmt, clippy `-D warnings`, `cargo test --locked`, release compile.
- PR: #27. Commits use the trailer above. Branch pushes cancel in-progress
  runs (that is fine now that PRs are linux-only; runs take ~2-4 min).
- `gh` is authed as Jamicow6. Wiki remote accepts git push (verified).
- models.dev `api.json` provider objects have no logo field; only
  `doc, env, id, models, name, npm`. Logos must be fetched per id.
- `yopes --version` works via clap; `/version` added in-app.
- `crate::update::current_version()` is public; `option_env!
  ("YOPES_BUILD_VERSION")` exists in `update.rs` (not used by `/version`).
- `CARGO_NET_OFFLINE=false`; crates.io reachable. `cargo metadata` (not
  `cargo check`) resolved Cargo.lock — took >120s once, rerun with a bigger
  timeout if needed.
- TODO batches were originally planned as one-branch-per-batch; everything
  ended up on the single `feature/provider-keys` PR instead. Merge strategy is
  the user's call.
