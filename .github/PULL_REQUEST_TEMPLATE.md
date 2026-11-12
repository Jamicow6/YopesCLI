## What this changes

<!-- One or two sentences. Link the issue it closes, if any. -->

## How it was checked

<!-- CI is the compile gate. Say what you ran locally, if anything. -->

## Checklist

- [ ] Commit message follows `feat(short-name) - ...` or `bug(short-name) - ...`
- [ ] `CHANGELOG.md` updated under `[Unreleased]` for user-visible changes
- [ ] New tool? Every row of the [checklist](docs/ADDING-A-TOOL.md#checklist) is done
- [ ] No `unwrap` or `panic!` outside `#[cfg(test)]`
- [ ] Tests cover the new branch, loop, or parser
