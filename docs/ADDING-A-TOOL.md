# Adding a tool

A tool is a JSON schema the model can call plus a Rust function that runs it.
Adding one touches the same handful of places every time; miss one and the tool
is either invisible to the model or unrunnable.

## 1. Implement it

Put the body in the family module under `src/tools/` (`fs.rs`, `search.rs`,
`shell.rs`, `web.rs`, `todo.rs`) or add a new one and declare it in
`src/tools/mod.rs`.

```rust
pub(super) fn my_tool(arguments: &str) -> Result<String> {
    let args: MyToolArgs = parse(arguments)?;
    let path = resolve(&args.path);
    // ...
    Ok(truncate_output(result))
}
```

Use the shared helpers rather than re-deriving them:

- `parse::<T>(arguments)` — deserializes the JSON arguments, with a readable
  error that clips the bad input.
- `resolve(path)` — joins a relative path onto the working directory.
- `truncate_output(text)` — caps what goes back to the model.
- `clip(text, width)` — shortens a string for a one-line preview.

Return `Err` for anything the model should see and correct. The error text is
the tool result.

## 2. Register it in `src/tools/mod.rs`

Every one of these is a separate match arm or list entry:

| Place | What to add |
| --- | --- |
| `definitions()` | The JSON schema: name, description, `parameters`. |
| `execute()` | The dispatch arm calling your function. |
| `preview()` | A one-line summary for the approval dialog and transcript. |
| `requires_approval()` | Only if the call can destroy data or spend money. |
| `applied_change()` / `applied_change_all()` | Only if it changes files, so the transcript can show a diff. |
| `system_prompt()` | One sentence telling the model when to reach for it. |
| `TOOL_NAMES` | The name, and the count assertion in the tests. |

## 3. Gate it in `src/app.rs`

`Mode::denied` decides what plan mode blocks. A tool that writes, edits, or
runs commands must be denied there; a read-only tool must not be.

## 4. Document it

Add it to the tool list in `README.md` under **Tools**, and to the module map in
[ARCHITECTURE.md](ARCHITECTURE.md) if it introduced a new module.

## 5. Test it

Unit tests live in the family module next to the code, not in `mod.rs`. Cover
the happy path and the argument errors the model is likely to produce. If the
tool touches the filesystem, build a scratch directory with `std::env::temp_dir`
and clean it up.

## Checklist

- [ ] Function in a `src/tools/` family module
- [ ] `definitions()` schema
- [ ] `execute()` arm
- [ ] `preview()` arm
- [ ] `requires_approval()` if destructive
- [ ] `applied_change*()` if it writes files
- [ ] `system_prompt()` sentence
- [ ] `TOOL_NAMES` + count
- [ ] `Mode::denied` in `app.rs`
- [ ] `README.md` tool list
- [ ] Tests
