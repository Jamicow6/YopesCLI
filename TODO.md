# TODO

Working list for the YopesCLI improvement pass. Everything lives on one
branch, one PR: `feature/provider-keys`, **PR #27** (`gh pr view 27`).

Legend: `[ ]` open · `[~]` in progress, uncommitted · `[x]` done

## Resume here — state at handoff

Branch `feature/provider-keys` (off `main`), pushed to `origin`, PR #27 open,
working tree clean. Everything below is on the PR.

Recent commits on the PR:

- `422e796` — test-import fix, MSRV `% 2 != 0` in `logos.rs`, README trim
  (D1).
- `d730a33` + `bac9d26` — fix(logos): the logo cache now round-trips
  `Color::Reset` cells. The old `encode` skipped `Color::Reset` spans, so
  fully transparent rows became empty lines that `decode` dropped (CI:
  `the_cache_round_trips` got 2 rows back from 3). A cell is now 7 hex bytes
  — a flag pair marking the reset channels, then the colours — and caches in
  the old 6-byte format fail the `% 7` check and are refetched.
- (this commit) — item 17: `Ctrl+G` toggles mouse capture
  (`Action::ToggleMouseCapture`, `App.mouse_capture`, dispatched in
  `main.rs`). Capture off → the terminal's own selection and copy work;
  capture on → the app's mouse scroll and clicks return. Ctrl+V text paste
  already worked via bracketed paste. Documented in the help popup. The
  in-app selection model was skipped: same outcome for far less code.
- Deferred at the user's request: the tool-frame text leak (screenshot
  2026-11-12 224743). Leading suspects: `frame_lines` in
  `src/ui/approval.rs` pads the right border from `UnicodeWidthStr` math, so
  tabs, `\r`, or CJK/emoji in tool output shift the border; `truncate` in
  `src/ui/mod.rs` over-reserves a column for `…`.

## Batch A — Provider keys, connection lifecycle — done

- [x] A1 Per-provider secret storage: `SavedConfig.provider_keys`
  keyed by base URL (`src/config.rs`).
- [x] A2 Legacy fallback to top-level `secret`; no version bump.
- [x] A3 Stored key reused in `/connect` (`ConnectStage::Unlock`).
- [x] A4 Fasfas reuses its stored key.
- [x] A5 Model switch persists (`config::save_model`).
- [x] A6 `/disconnect [name]` forgets key + models.
- [x] A7 "Change API key" row in the catalog Models list.
- [x] A8 `/version`.

## Batch B — Reliability — done

- [x] B1 Bounded retries: `MAX_ATTEMPTS = 5` (`src/backend.rs`).
- [x] B2 No `eprintln!` from the request path.
- [x] B3 Retry visible: `StreamEvent::Retrying` → footer line.
- [x] B4 Honours `Retry-After`, clamped 1..=60s.
- [x] B5 Read-only commands work while streaming.

## Batch C — Viewing experience — done

- [x] C1 Mode indicator dot in the header (`app/mode.rs`).
- [x] C2 Focus loss handled (`App.focused`).
- [x] C3 Mouse move no redraw.
- [x] C4 Width-aware padding: `pad_to` in `src/ui/mod.rs`, used by
  `ui/pickers.rs` + `ui/connect.rs` (test:
  `padding_counts_display_columns_not_chars`).
- [x] C5 Caret inside the tool frame.
- [x] C6 Help popup scrolls.
- [x] C7 Transcript scroll stays `usize`, clamped to `u16` only at render
  (`ui/transcript.rs`, `ui/composer.rs`); click map verified against the
  painted rows.
- [x] C8 Only `AGENTS.md` in the system prompt.

## Batch D — Documentation — done

- [x] D1 README trimmed to a short tour; detail in the wiki.
- [x] D2 GitHub wiki: 9 pages, pushed (`7cf20b2`); clone at
  `%USERPROFILE%\Temp\yopes-wiki`.
- [x] D3 `/tools` deprecated, still works.
- [x] D4 CHANGELOG entries per batch under `[Unreleased]`.

## Batch E — Provider icons, mascot, temp files — done

- [x] E1 models.dev logos: fetch, rasterise, cache on disk, draw.
- [x] E2 Animated mascot with a width test.
- [x] E3 Temp files: `src/temp.rs` (`%USERPROFILE%\Temp` on Windows).
- [x] E4 The system prompt tells the model where scratch files belong.

## Still open from the user's original list

- [x] Mouse text selection + copy (item 17): `Ctrl+G` toggles mouse capture;
  Ctrl+V text paste worked already via bracketed paste.
- [x] CI green on the PR (linux-only). Watch after each push; known past
  failure classes (E0308/E0004/E0382/E0507, `tokio::select!` guard, dead
  arms, logos pixel types) are all fixed.
- [ ] Mouse text selection with an in-app highlight (optional upgrade of
  item 17) — skipped; the capture toggle covers it.

## Notes / facts for continuation

- No host compilation (AGENTS.md policy): no `cargo build/check/run/test`
  locally. `cargo fmt --all` is the only local gate (it parses). CI checks:
  fmt, clippy `-D warnings`, `cargo test --locked`, release compile.
- PR: #27. Commits use `feat(short-name) - ...` / `bug(short-name) - ...`.
  Branch pushes cancel in-progress runs (fine now that PRs are linux-only;
  runs take ~1-4 min).
- `gh` is authed as Jamicow6. Wiki remote accepts git push (verified).
- models.dev `api.json` provider objects have no logo field; logos are
  fetched per id from `https://models.dev/logos/<id>.svg`.
- `CARGO_NET_OFFLINE=false`; crates.io reachable. `cargo metadata` took
  >120s once; rerun with a bigger timeout if needed.
