# Contributing to Vela

Thanks for taking the time to contribute. This document is short on purpose.

## Ground rules

1. **Do not break the tests, the lints, or the terminal.** Before opening a pull
   request, all of these must pass:

   ```sh
   cargo fmt --check
   cargo clippy --all-targets --all-features -- -D warnings
   cargo test
   ```

2. **Never silently discard user data.** Any change touching saving, loading,
   closing or quitting must keep the invariants in
   [docs/architecture.md](docs/architecture.md): no truncation before the new
   bytes are on disk, no "saved" message before the rename succeeded, no closing
   a dirty buffer without confirmation.

3. **Do not suppress warnings to go green.** Fix the cause. If a lint is genuinely
   wrong, `#[allow(...)]` it at the narrowest possible scope with a comment
   explaining why.

4. **No `unsafe` without a written justification.** The crate currently contains
   none; keep it that way unless there is no alternative.

## Workflow

```sh
git clone <repo> && cd vela
cargo build
cargo run -- examples/hello.txt
```

The debug build is fast enough to develop against. Use `cargo run -- -v` to get
a debug-level log at `$XDG_STATE_HOME/vela/logs/` (`--no-log` to skip logging
entirely).

## What to work on

[docs/roadmap.md](docs/roadmap.md) lists the intended order of work. Issues
labelled `good first issue` are self-contained. If you want to change something
architectural (the transaction model, the rendering pipeline, the command
system), open an issue first — those changes are hard to review after the fact.

## Style

- Rust 2021, `rustfmt` defaults, no manual formatting of code in comments.
- Public items get a doc comment that says *why*, not just what.
- Error messages are lowercase, specific and actionable: "could not write
  /path/x: permission denied", not "save failed".
- Comments explain decisions that are not obvious from the code. Avoid narrating
  what the next line does.

## Tests

Every behaviour change needs a test that fails before the change and passes
after. See the table at the end of [docs/extending.md](docs/extending.md) for
where a test belongs:

- module unit tests for logic,
- `tests/editing_core.rs` / `tests/workflow.rs` for user-visible behaviour,
- `tests/ui_render.rs` for anything painted on screen,
- `tests/property.rs` for invariants that must hold for all inputs,
- `benches/core_bench.rs` for the hot path.

Integration tests must use `App::headless` so they never read or write the real
user profile, and must not depend on the system clipboard.

## Commits and pull requests

- One logical change per commit; the message says what changed and why.
- A pull request description lists what was verified and how
  (`cargo test --test workflow`, manual check in `wezterm`, ...).
- If you fix a bug, describe the reproduction in the commit message. It becomes
  the regression test's documentation.

## Licence

Contributions are accepted under the MIT licence, the same as the project.
