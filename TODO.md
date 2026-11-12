# TODO

Working list for the YopesCLI improvement pass. Items are grouped into
batches; each batch is one branch, one PR, one CI run.

Legend: `[ ]` open · `[~]` in progress · `[x]` done

## Batch A — Provider keys, connection lifecycle

The recurring complaint: a connected provider loses its API key, and
re-selecting a provider or switching models asks for the key again.

- [ ] **A1** Per-provider secret storage. `SavedConfig` keeps a single
  top-level `secret`, so connecting a second models.dev provider overwrites the
  first one's key (`src/config.rs::save_connection`, the `_` arm drops the
  secret when `base_url` differs). Add a `provider_keys` map keyed by
  connection target so every provider keeps its own key.
- [ ] **A2** Legacy migration without a version bump. Fold the old top-level
  `secret` into the map on load, keyed by the active `base_url`, so existing
  configs keep working and `CONFIG_VERSION` stays at 1.
- [ ] **A3** Never ask for a key that is already stored. In `/connect`, picking
  a models.dev provider that already has a secret must not prompt for the key
  again: reuse a plaintext secret silently, and for an encrypted secret ask
  only for the password.
- [ ] **A4** The preferred provider (Fasfas) must not re-ask for its key.
  The Discord key is stored per endpoint like any other; re-entering `/connect`
  and picking Fasfas reuses it and goes straight to the model list.
- [ ] **A5** Switching models on a connected endpoint keeps the key and
  persists the new model to disk (`/model` currently changes the model in
  memory only, so the choice is lost on restart).
- [ ] **A6** `/disconnect` drops the active connection and forgets its key.
- [ ] **A7** Saved custom endpoints can be removed from `/connect` (a
  "Remove endpoint" row next to "Change API key").
- [ ] **A8** `/version` prints the running version and build.

## Batch B — Reliability

- [ ] **B1** Bounded retries. `send_with_retry` retries forever on a transient
  failure, so a dead provider hangs the turn instead of erroring
  (`src/backend.rs`).
- [ ] **B2** No `eprintln!` from the request path. The retry notice writes to
  stderr, which lands on top of the TUI in the alternate screen.
- [ ] **B3** Retry status is visible in the TUI (footer notice), and the
  attempt count and delay are reported.
- [ ] **B4** Retry a stream that dies mid-flight, and honour `Retry-After`.
- [ ] **B5** Commands work while the model is responding. Slash commands are
  refused during a turn (`src/app/turn.rs`); the read-only ones (`/help`,
  `/version`, `/model`, `/tools`, `/effort`, `/system`, `/skills`) should work
  while a response streams.

## Batch C — Viewing experience

- [ ] **C1** Mode indicator. Build / Chat / Plan shown with a coloured dot:
  build purple, chat yellow-brown, plan brown. A dot plus the mode name in the
  header, so the active mode is visible at all times.
- [ ] **C2** Focus loss must not cost performance. The terminal emits
  `FocusLost`/`FocusGained`; while unfocused the ticker should stop spinning
  the spinner and stop redrawing (`src/main.rs`).
- [ ] **C3** Mouse motion must not trigger a redraw. Every `Event::Mouse`,
  including `Moved`, currently forces a full transcript rebuild.
- [ ] **C4** Width-aware padding in the pickers and the connect list. Rows pad
  with `{:<width$}`, which counts characters, so CJK and emoji push the
  columns out of line (`src/ui/pickers.rs`, `src/ui/connect.rs`).
- [ ] **C5** The streaming caret must not land outside the tool frame. It is
  appended to the last line, which is the frame's bottom border
  (`src/ui/transcript.rs`).
- [ ] **C6** The help popup scrolls. It is a fixed-height paragraph that
  silently clips on a short terminal (`src/ui/help.rs`).
- [ ] **C7** Transcript scroll must not saturate at 65 535 rows, and the click
  map must agree with what is painted.
- [ ] **C8** Only `AGENTS.md` is project instruction context. Every `.md`
  file is collected today, so a README ends up in the system prompt
  (`src/tools/mod.rs::collect_agents_files`).

## Batch D — Documentation

- [ ] **D1** Trim the README. Move the detail to the wiki, keep the README a
  short tour.
- [ ] **D2** Create the GitHub wiki: tool reference, commands, configuration,
  skills, troubleshooting, architecture.
- [ ] **D3** Deprecate `/tools`. Keep it working for compatibility, stop
  advertising it in the palette and the README.
- [ ] **D4** CHANGELOG entries for every batch above.

## Batch E — Provider icons, mascot, temp files

- [ ] **E1** Render the models.dev provider logo as an icon. The provider SVG
  at `models.dev/logos/<id>.svg` is rasterised to a small bitmap and drawn
  with the same half-block technique the pasted-image renderer uses, cached on
  disk so it is fetched once per provider.
- [ ] **E2** The welcome mascot is more alive than a static cat and a rolling
  ball: the yarn unwinds, the tail moves, the cat reacts.
- [ ] **E3** Temporary files go to `%USERPROFILE%\Temp` on Windows rather than
  `%LOCALAPPDATA%\Temp` (`std::env::temp_dir()`), for tool logs, background
  job logs, and skill scratch space.
- [ ] **E4** The temporary directory is created on demand and reported to the
  model, so scratch files land there too.

## Notes

- No host compilation: `cargo build`, `cargo check`, `cargo run`, and
  `cargo test` are off limits on this machine. CI is the gate.
- `On-the-line` runs `cargo fmt --check`, `cargo clippy --all-targets -- -D
  warnings`, `cargo test`, and a release compile on Linux and Windows.
- `Production` is dispatched by the user only.
