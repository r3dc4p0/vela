# Vela

A modern, fast terminal text editor with the keyboard shortcuts you already know.

Vela is a modal-free editor for the terminal: `Ctrl+S` saves, `Ctrl+F` finds,
`Ctrl+Z` undoes, `Ctrl+P` jumps to a file, `Ctrl+Shift+P` opens a command
palette. It opens instantly, handles large files without stalling, and never
loses your work.

[Website](https://r3dc4p0.github.io/vela/) · [Architecture](docs/architecture.md) ·
[Key bindings](docs/keybindings.md) · [Roadmap](docs/roadmap.md)

```
┌──────────────────────────────────────────────────────────────────────┐
│ VELA · src/buffer/mod.rs ●                        2 buffers   v0.5.1 │
│ ▏ storage.rs ▏ mod.rs ●                                              │
│  1 ▏ use std::ops::Range;                                            │
│  2 ▏                                                                 │
│  3 ▏ /// A text buffer backed by a rope.                             │
│  4 ▏ pub struct Document {                                           │
│  5 ▏     storage: Box<dyn TextStorage>,                              │
│  6 ▏     cursor: usize,                                              │
│  7 ▏     history: History,                                           │
│  8 ▏ }                                                               │
│    ▏ ~                                                               │
│    ▏ ~                                                               │
│ ● Rust  LF  UTF-8                                 Ln 4, Col 12   12% │
│ Ctrl+s save   ·   Ctrl+f find   ·   Ctrl+p open file   ·   F1 help   │
└──────────────────────────────────────────────────────────────────────┘
```

## Features

**Editing** — character, word, line and document motions; shift-selection;
cut/copy/paste; undo/redo with coalescing and save-point tracking; duplicate,
delete and move lines; indent/dedent; comment toggling; auto-indent; bracket
matching; trailing-whitespace trimming; optional final newline.

**Typing** — brackets and quotes complete themselves: `(` writes `()` with the
caret between them, a closer already under the caret is stepped over instead of
doubled, backspace inside an empty pair removes both halves, and Enter between
`{}` opens the block onto three lines. Quotes only pair where a string can
start, so `don't` and Rust's `&'a` are left alone, and nothing pairs between
the two halves of a word — `hel|lo` stays `hel(lo`. Selecting text and typing a
bracket surrounds it rather than replacing it, and leaves it selected so a
second bracket wraps it again. One setting, `auto_pairs`, or `Ctrl+Shift+A` to
toggle it for the session.

**Multi-cursor** — stack carets with `Ctrl+Alt+Up`/`Ctrl+Alt+Down`, add one per
occurrence of the selection with `Ctrl+D`, or place them with `Alt+Click`.
Typing, deleting, indenting and newlines apply to every caret as a single undo
step, and `Esc` collapses back to one. Occurrences are matched literally, so a
selection of `he` also finds the `he` inside `hello`.

**Mouse** — click to place the cursor, drag to select (autoscrolling above and
below the text), double-click for a word and triple-click for a line, and click
a tab to switch buffers. The wheel scrolls the view rather than the cursor, so
it can look anywhere in the buffer; when the cursor leaves the screen the
status bar keeps reporting its line, in amber. `Alt+Click` adds a cursor.
Disable the lot with `mouse = false`.

**Panes** — `Ctrl+\` splits the editing area and shows the same file twice, at
two places in it, scrolling independently. `Ctrl+Alt+Left`/`Right` moves between
panes and a click focuses the one under the pointer; `Ctrl+Alt+W` closes it. Load
another buffer into a pane and you have two files side by side. Edits are shared
because there is only ever one document.

**Look** — the chrome is drawn the way the design on the website is drawn: a
border is a fraction of a cell rather than a whole one, so panels are framed by
hairlines on the edge of their rows, tabs are split by a rule, and the
line-number gutter is ruled off from the text. The percentage in the status bar
is a badge. If you prefer the older block-glyph chrome, `Ctrl+Shift+U`,
`--ui cell` or `[ui] style = "cell"` brings it back.

**Files** — multiple buffers with tabs; atomic saves (temp file + `fsync` +
`rename`) that preserve permissions and symlinks; UTF-8/UTF-16 with BOM
detection; LF/CRLF/CR line endings detected per file and written back unchanged;
binary files refused rather than corrupted; external-modification detection;
recent files; session restore. Unsaved buffers are copied to the data directory
a second after they change, so a crash, a `kill` or a closed terminal does not
take them with it — the next start brings them back, dirty, at the cursor they
had. A clean exit discards the copy, because by then you have been asked. Big files open on a worker thread behind a
progress panel that counts the bytes actually read, so the window keeps painting
while a two-gigabyte file loads.

**Search** — incremental search with case, whole-word and regex toggles;
find & replace including replace-all as a single undo step; match counter;
`F3`/`Shift+F3` to repeat. `Ctrl+F` searches what you selected, verbatim — a
partial selection stays partial. Typing is answered from a fast first pass, and
the rest of the file is scanned in the background once you press Enter or `F3`,
so every match is reachable — including in files too large to scan per
keystroke.

**Navigation** — go to line/column, fuzzy file finder (`Ctrl+P`), command
palette (`Ctrl+Shift+P`), help screen generated from the live keymap.

**Interface** — line-number gutter (absolute or relative), syntax highlighting
for 17 languages, dark and light themes, visible whitespace, indent guides,
word wrap (`Alt+W`), contextual shortcut hints, a notification area, and
graceful degradation down to a one-row terminal. Wrapping is not a cosmetic
layer: the wheel, the arrow keys, page keys and the mouse all move by visual
rows, so a single long line can fill the window and be edited like any other.

## Install

```sh
cargo install --path .
```

Requires Rust 1.82 or newer. The editor is a single static binary with no
runtime dependencies.

## Usage

```sh
vela                    # start with an empty buffer
vela notes.md           # open a file (created on first save if missing)
vela a.rs b.rs          # open several files
vela --print-config     # print the effective configuration as TOML
vela --list-keys        # print every command and its bindings
```

| Flag | Meaning |
| --- | --- |
| `-c`, `--config <FILE>` | Use a specific configuration file |
| `--ui <pixel\|cell>` | How the chrome is drawn: hairlines or block glyphs |
| `--print-config` | Print the effective configuration and exit |
| `--list-keys` | Print the key bindings and exit |
| `--smoke` | Start, draw one frame and exit (startup check for CI) |
| `--no-log` | Do not write a log file |
| `-v`, `--verbose` | Verbose logging |
| `-h`, `--help` | Usage |
| `-V`, `--version` | Version |

## Shortcuts

See [docs/keybindings.md](docs/keybindings.md) for the complete table, including
the terminal-specific caveats.

The essentials:

| Key | Action |
| --- | --- |
| `Ctrl+S` / `Ctrl+Shift+S` | Save / Save As |
| `Ctrl+O` / `Ctrl+N` / `Ctrl+W` | Open / New / Close buffer |
| `Ctrl+P` | Fuzzy file finder |
| `Ctrl+Shift+P` | Command palette |
| `Ctrl+F` / `Ctrl+H` | Find / Replace |
| `F3` / `Shift+F3` | Find next / previous |
| `Ctrl+Z` / `Ctrl+Y` | Undo / Redo |
| `Ctrl+X` / `Ctrl+C` / `Ctrl+V` | Cut / Copy / Paste |
| `Ctrl+A` | Select all |
| `Ctrl+D` | Add a cursor at the next occurrence |
| `Ctrl+Alt+Up` / `Ctrl+Alt+Down` | Add a cursor above / below |
| `Alt+Click` | Add or remove a cursor |
| `Ctrl+G` | Go to line |
| `Ctrl+Tab` | Next buffer |
| `Alt+W` | Toggle word wrap |
| `F1` | Help |

> **Terminals are not all the same.** `Ctrl+C` is delivered as `SIGINT` by many
> terminals before the application sees it, and `Ctrl+Shift+X` / `Ctrl+Tab`
> cannot be distinguished without the Kitty keyboard protocol. Vela enables the
> protocol when the terminal supports it and falls back to unambiguous
> alternatives otherwise; the status bar shows `~keys` when the fallback is
> active. See [docs/keybindings.md](docs/keybindings.md).

## Configuration

Configuration lives at `$XDG_CONFIG_HOME/vela/config.toml`
(`~/.config/vela/config.toml`), or `~/Library/Application Support/vela/config.toml`
on macOS. Every key is optional; see [`config.example.toml`](config.example.toml)
for the full set.

```toml
[editor]
tab_width = 4
use_spaces = true
auto_indent = true
syntax_highlighting = true
relative_numbers = false
show_whitespace = false
word_wrap = false

[ui]
theme = "vela-dark"        # or "vela-light"
style = "pixel"            # or "cell" — block borders instead of hairlines
show_hints = true
mouse = true

[keys]
"ctrl+shift+p" = "help.commandPalette"
"f5" = "file.reload"
```

Invalid values never abort startup: they are clamped or ignored and reported in
the notification area.

## Performance

Measured on the development machine (Apple silicon, macOS 27, release build,
`cargo bench`). See [docs/performance.md](docs/performance.md) for methodology.

| Operation | Target | Measured |
| --- | --- | --- |
| Cold start (process → first frame) | < 100 ms | **7 ms** |
| Open 1 MiB | < 50 ms | **1.3 ms** |
| Open 100 MiB | < 1 s | **136 ms** |
| Literal search, full scan of 100 MiB | < 500 ms | **23 ms** |
| Typing latency (1 MiB buffer) | imperceptible | **0.3 µs/keystroke** |
| Full frame, 120×24 | < 16 ms | **70 µs** |

## Architecture

Vela separates state, mutation and rendering:

```
terminal event → input::translate → Action → App::execute → Document
                                                   ↓
                                              ui::draw (read-only)
```

`App` is the only place where an intent becomes a document mutation, and the
renderer never mutates anything. Text lives in a rope behind a `TextStorage`
trait, so the buffer implementation can be replaced without touching the rest of
the editor. See [docs/architecture.md](docs/architecture.md).

## Development

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo bench
```

See [CONTRIBUTING.md](CONTRIBUTING.md) and [docs/extending.md](docs/extending.md)
for how to add commands, key bindings, UI components and buffer backends.

## Status and limitations

Vela is a working editor, not a finished product. Known limitations are listed
in [docs/roadmap.md](docs/roadmap.md); the important ones:

- No split views yet.
- Line-oriented commands (duplicate/delete/move line, comment toggle, indent,
  trim whitespace) act on the primary cursor and collapse the others; typing,
  deleting and newlines do apply to every cursor.
- Syntax highlighting is a fast line-oriented lexer, not a full parser: it can
  mis-colour unusual constructs (raw strings, nested comments) but never affects
  the text itself.
- Regex search is not incremental on very large files (it rescans on demand).
- Live external-change notifications require a manual save to be detected; a
  filesystem watcher is on the roadmap.

## License

MIT. See [LICENSE](LICENSE).
