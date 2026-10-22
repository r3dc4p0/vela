# Architecture

Vela is a layered application. Each layer has one job, and the dependencies only
ever point one way.

```
┌──────────────────────────────────────────────────────────────┐
│ terminal (crossterm)  raw events, alternate screen, restore  │
└───────────────┬──────────────────────────────────────────────┘
                │ Event
┌───────────────▼──────────────────────────────────────────────┐
│ input::translate   key → Action (no state, pure function)    │
└───────────────┬──────────────────────────────────────────────┘
                │ Action / Command
┌───────────────▼──────────────────────────────────────────────┐
│ App   owns buffers, ui state, config, clipboard, search      │
│       the only place where an intent mutates a document      │
└───────┬──────────────────────────────────┬───────────────────┘
        │ Document mutations               │ reads (never writes)
┌───────▼──────────────────┐      ┌────────▼───────────────────┐
│ buffer::Document (rope)  │      │ ui::draw → ratatui Frame   │
│ undo, motions, selection │      │ editor, chrome, overlays   │
└──────────────────────────┘      └────────────────────────────┘
```

## Modules

| Module | Responsibility |
| --- | --- |
| `main` | Argument parsing, logging, terminal lifecycle, signal handlers |
| `app` | Application state, the event loop, command dispatch, file operations, overlays |
| `buffer` | `Document`: text storage, cursors, selection, undo, motions |
| `buffer::storage` | `TextStorage` trait and the rope implementation |
| `buffer::scan` | Streaming literal search over the rope |
| `buffer::motion` | Grapheme- and display-width-aware cursor motions |
| `buffer::undo` | Bounded transaction history with coalescing |
| `editing` | Line-oriented commands (comment, indent, move, duplicate, trim) |
| `input` | Key event → `Action` translation |
| `keymap` | `KeyChord`, binding table, config parsing |
| `commands` | The `Command` enum: ids, titles, categories, default keys |
| `search` | Query model, match set, regex and literal search, replacement |
| `filesystem` | Loading, encoding/EOL detection, atomic saving |
| `config` | TOML schema, defaults, validation |
| `theme` | Colour palettes and overrides |
| `ui` | Layout, editor rendering, chrome, overlays, messages |
| `terminal` | Terminal setup/teardown, keyboard-protocol negotiation |
| `clipboard` | System clipboard with an internal fallback |
| `fuzzy` | Subsequence scoring for the palettes |
| `syntax` | Line-oriented lexer with cross-line state cache |
| `logging` | Rolling file logs |
| `platform` | Paths and environment probing |
| `error` | The `Error` enum and user-facing messages |

## Data flow

A keystroke travels through exactly one path:

```
Event::Key
  → App::handle_key
      → if an overlay captures input: App::handle_overlay_key
      → else input::translate(key, &keymap) → Action
          → InsertText / InsertNewline / InsertTab / Backspace / Move / Command
              → Document mutation (one undoable transaction)
              → App::after_edit (invalidate syntax cache, refresh search)
  → ui::draw (next frame) reads the new state
```

Nothing in `ui` mutates state, and nothing in `buffer` knows about the terminal.
That is what makes the editor testable without a tty: the integration tests build
an `App::headless`, feed it `Event::Key` values and assert on the document and on
the rendered frame.

## Key decisions

### Rope behind a trait

`TextStorage` is the seam between the editor and its text representation.
`RopeStorage` (ropey) is used everywhere, but `Document` only calls the trait
methods, so a gap buffer or a piece table can be dropped in for benchmarking.
`snapshot()` returns an `Arc`-backed rope clone in O(1), which is what makes
background search safe: a worker can scan a snapshot while the user keeps typing.

### Undo as transactions, not keystrokes

Every mutation produces a `Transaction` holding the exact removed/inserted text
plus the cursor and anchor before and after. Two consequences:

- Undo restores the *cursor*, not just the text, so repeated undo/redo does not
  drift.
- Typing coalesces: consecutive inserts at adjacent positions within 500 ms and
  512 characters merge into one entry. Backspaces coalesce the same way. Save,
  undo and redo insert a *barrier* so a run never merges across them.

`apply_batch` exists so that "replace all", "indent selection" and "trim
whitespace" are a single undo step even though they touch many places: edits are
applied from the highest offset downwards (so every range stays valid in the
pre-edit coordinate system) and recorded as one transaction.

`apply_caret_edits` is the multi-cursor variant: one edit per caret, the same
bottom-up application, and one transaction. Each caret is placed after its own
inserted text by accumulating the offset delta of every edit below it, which is
also what makes a batch delete leave every caret at its own deletion point. The
transaction records the secondary carets before and after the edit, so undo
brings the whole caret set back rather than collapsing to one.

Dirty state is `saved_index != index` in the history, not a boolean, which is why
undoing back to the save point clears the modified marker.

### Multi-cursor model

A `Document` stores one primary caret as `cursor` + `anchor` (every existing
path — status bar, search, scroll, mouse — reads it) and any number of secondary
carets in `extra: Vec<Caret>`. The vector is kept sorted, deduplicated and
non-overlapping with the primary by `normalise_carets`; overlapping selections
are *rejected* rather than merged, because two carets whose selections intersect
cannot be expressed as one non-overlapping edit batch.

Single-caret editing keeps its original code path, including undo coalescing.
Only when `caret_count() > 1` does editing route through `apply_caret_edits`.
Commands that shift offsets in a way they cannot map back — line duplicate,
line delete, batch indent, `apply_batch` in general — clear the secondary carets
instead of leaving stale positions behind.

`Ctrl+D` searches for the selected text verbatim, with no word-boundary
widening: a selection of `he` also matches the `he` inside `hello`. The search
text is whatever the user actually selected, and quietly widening a fragment to
whole words would make it impossible to rename part of a longer identifier.

### Atomic saving

`filesystem::save` writes to a temporary file in the same directory, `fsync`s it,
copies the original permissions, renames it over the target and then `fsync`s the
directory. A crash therefore leaves either the old file or the new one, never a
truncated one. The buffer is only marked clean *after* the rename succeeds — the
editor never claims a save it did not perform.

The file's EOL style and encoding are properties of the document, applied on
write, so a CRLF file stays CRLF after editing.

### Streaming search

`buffer::scan` walks the rope in 64 KiB windows, keeping `needle_len - 1` bytes
of overlap so matches spanning a window boundary are still found. `memmem`
(SIMD-accelerated) does the byte matching; character offsets are computed in the
same pass, so the result is a `Vec<Range<usize>>` of *char* indices without a
second scan. Case-insensitive search uses a Unicode-aware fallback window
because folding changes byte lengths.

The result set is capped (`MAX_MATCHES = 20_000`) and flagged as truncated, so a
query that matches everything cannot exhaust memory.

### Display-width-correct rendering

The renderer works in terminal columns, not characters: tabs expand to the next
tab stop, control characters become `^X`, wide CJK/emoji occupy two cells and
combining marks zero. Long lines are rendered through a window that starts at the
first visible character, so opening a 100 MB single-line file does not freeze the
UI. Lines longer than 8 KiB are clipped rather than scanned to the end.

### One command system

`Command` is an enum with a stable id, a title, a category and default keys. Key
bindings, the palette and the help screen all resolve through it, so a new
command automatically appears in the palette, in the help screen and can be
rebound in the config. `docs/extending.md` shows the three-line recipe.

### Overlays and the borrow checker

Modal input needs `&mut App` and the overlay state at the same time. Rather than
reaching for `unsafe` or a raw pointer, `handle_overlay_key` moves the overlay out
of `App` with `std::mem::replace`, dispatches, then puts it back. Handlers that
must run while the overlay is detached take the data they need as parameters
(`replace_all_with(&str, bool)`) instead of reading `App::ui` — this is a rule,
not an accident: a handler that reads `ui.overlay` while detached silently does
nothing.

### Terminal lifecycle

`TerminalGuard` is a RAII guard: raw mode, alternate screen, bracketed paste and
(optionally) the Kitty keyboard protocol are enabled in `enter` and restored in
`Drop`. A panic hook restores the terminal *before* the panic message prints, and
`signal-hook` handlers do the same for `SIGTERM`, `SIGINT`, `SIGHUP` and
`SIGQUIT`. A crash therefore never leaves the user with a broken shell.

Keyboard enhancement is negotiated by *name* (`$TERM`, `$TERM_PROGRAM`,
`$KITTY_WINDOW_ID`, `$WEZTERM_PANE`) rather than by probing, because crossterm's
probe blocks for up to two seconds when the terminal does not answer — which
would dominate the startup budget.

## Testing strategy

| Layer | How it is tested |
| --- | --- |
| Storage, motions, undo, search, filesystem | Unit tests in each module, including EOL/encoding/BOM/permission edge cases |
| Editing commands | `tests/editing_core.rs` drives the real `App` through real key events |
| Multi-cursor | `tests/multi_cursor.rs`: caret sets, next occurrence, Alt+Click, undo, status bar, painted blocks |
| Workflows | `tests/workflow.rs`: buffers, search, replace, palette, quit, external changes |
| UI | `tests/ui_render.rs` renders into ratatui's `TestBackend` and asserts on the text of each row |
| Invariants | `tests/property.rs` with proptest: undo symmetry, cursor bounds, search range validity, save/load round trips |
| Performance | `benches/core_bench.rs` (Criterion) |

The integration tests use `App::headless`, which never touches the user's profile
directory and uses an internal-only clipboard, so they are deterministic and can
run in parallel.
