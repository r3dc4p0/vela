# Extending Vibe

Everything in the editor is reachable from three seams: the command system, the
keymap, and the `TextStorage` trait. This guide shows how to use each.

Before you start:

```sh
cargo test            # must stay green
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt
```

## Adding a command

A command is a variant of `Command` in `src/commands.rs`. Adding one touches four
places in that file and one arm in `App::execute`.

1. **Declare the variant** in the `Command` enum, in the right category block:

```rust
pub enum Command {
    // ...
    /// Sort the selected lines.
    SortLines,
}
```

2. **Add it to `ALL`** so the palette and help screen see it:

```rust
pub const ALL: &[Command] = &[
    // ...
    Command::SortLines,
];
```

3. **Give it an id, a title, a category and its default keys**:

```rust
pub fn id(self) -> &'static str {
    match self {
        // ...
        Command::SortLines => "edit.sortLines",
    }
}

pub fn title(self) -> &'static str {
    match self {
        // ...
        Command::SortLines => "Sort Lines",
    }
}

pub fn category(self) -> Category {
    match self {
        // ...
        Command::SortLines => Category::Edit,
    }
}

pub fn default_keys(self) -> &'static [&'static str] {
    match self {
        // ...
        Command::SortLines => &[],     // palette-only, no binding
    }
}
```

4. **Implement the behaviour** in `App::execute` (`src/app/mod.rs`):

```rust
Command::SortLines => {
    self.with_doc(editing::sort_lines);
}
```

`with_doc` runs the function on the active document and, when it reports a
change, invalidates the syntax cache, clears the bracket cache and refreshes the
search matches. Use `apply_batch` inside the editing function so the whole
operation is one undo step:

```rust
// src/editing.rs
pub fn sort_lines(doc: &mut Document) -> bool {
    let Some(lines) = selected_lines(doc) else { return false };
    let mut block: Vec<String> = lines
        .clone()
        .map(|line| doc.line_text(line))
        .collect();
    block.sort();
    let edits = lines
        .zip(block)
        .map(|(line, text)| {
            let start = doc.storage().line_start(line);
            Edit::replace(start..start + doc.line_text(line).chars().count(), text)
        })
        .collect();
    doc.apply_batch(edits, doc.cursor())
}
```

That is all: the command now appears in `Ctrl+Shift+P`, in `F1`, in
`--list-keys` (once bound) and can be rebound from `config.toml`.

Add a unit test next to the other editing tests and, if the command has visible
effects, an integration test in `tests/editing_core.rs`.

## Adding a key binding

Default bindings live in `Command::default_keys`; user bindings live in the
config:

```toml
[keys]
"ctrl+shift+l" = "edit.sortLines"
```

`KeyChord::parse` accepts `ctrl`, `alt`, `shift`, `super` plus a character, a
named key or `f1`–`f12`. A chord that cannot be parsed, or that names an unknown
command, produces a startup warning instead of a failure.

If two commands claim the same chord the later binding in `Keymap::defaults`
wins; the debug build asserts that no default binding is invalid, which catches
typos at test time.

## Adding a UI component

The UI is drawn with `ratatui` into a `Frame`. There are three kinds of drawing
code:

- **Chrome** (`src/ui/chrome.rs`) — the top bar, tabs, status bar, hints and
  notifications. These receive a `&mut Buffer` and a `Rect` and never mutate
  `App`.
- **Editor** (`src/ui/editor.rs`) — the text area. It owns the scroll position
  and paints lines, the gutter, selection, matches and the cursor.
- **Overlays** (`src/ui/overlays.rs`) — modal panels drawn on top. They return
  the cursor position, if any.

To add a new panel:

1. Add a variant to `Overlay` in `src/ui/mod.rs` and handle it in
   `Overlay::kind` / `captures_input` (a panel that takes typing must return
   `true`).
2. Add a `draw_*` function in `src/ui/overlays.rs` and call it from
   `overlays::draw`.
3. Add a state struct (see `PathPrompt` for a small one).
4. Handle its keys in `App::handle_overlay_key` (`src/app/prompts.rs`). Remember
   the overlay is *detached* while you handle a key: take what you need as
   parameters instead of reading `self.ui.overlay`.
5. Add a hint line in `chrome::draw_hints` so the panel is discoverable.
6. Add a rendering test in `tests/ui_render.rs` using `vibe::ui::render_to_lines`.

Colours must come from `Theme`, never from a literal, so both themes keep
working. Add new colours as fields on `Theme` and set them in `Theme::dark`,
`Theme::light` and `Theme::set_color` (for the config override keys).

## Replacing the text storage

`Document` talks to text only through `TextStorage`
(`src/buffer/storage.rs`):

```rust
pub trait TextStorage: Debug + Send {
    fn len_chars(&self) -> usize;
    fn len_lines(&self) -> usize;
    fn len_bytes(&self) -> usize;
    fn char_at(&self, idx: usize) -> Option<char>;
    fn slice_chars(&self, start: usize, end: usize) -> String;
    fn line_slice(&self, line: usize) -> String;
    fn line_start(&self, line: usize) -> usize;
    fn line_of_char(&self, idx: usize) -> usize;
    fn insert(&mut self, char_idx: usize, text: &str);
    fn remove(&mut self, start: usize, end: usize) -> String;
    fn as_string(&self) -> String;
    fn snapshot(&self) -> Snapshot;
    // default methods: find_literal, ...
}
```

All indices are **character** indices. To add a backend:

1. Implement the trait for your type (a `Vec<char>`, a gap buffer, a piece
   table).
2. Add a variant to `Snapshot` and make `snapshot()` return it.
3. Add a constructor to `Document` (or a `Document::with_storage`).
4. Run the full test suite: the buffer tests are written against the trait's
   semantics, so they exercise the new backend directly.

`find_literal` has a default implementation; override it if your backend can do
better than the generic one.

## Extending search

Search is split between `src/search.rs` (query semantics, match set,
replacement) and `src/buffer/scan.rs` (scanning a rope).

- **New query option** (say, "match only in selection"): add the field to
  `Query`, apply it in `find_all`, expose it in `FindBar`/`ReplaceBar`, and wire
  the toggle in `find_key`/`replace_key` through `App::apply_query`.
- **New matcher** (fuzzy, structural): add a function next to `find_regex` that
  returns `Result<Vec<Range<usize>>>` in char indices and dispatch on a new
  `Query` field. `SearchState` only cares about the returned ranges.

Never do matching work in the render path: `SearchState` caches matches keyed by
`(document revision, query)` and rescans only when one of them changes.

## Adding a language

`src/syntax.rs` has a table of `Language` variants with their comment markers,
string delimiters and keyword sets. To add one:

1. Add the variant, extend `from_path` with the file extensions and `name`.
2. Add a `Spec` entry (line comment, block comment, string delimiters).
3. Add the keyword/type/constant sets with `keyword_set("...")`.
4. Add a lexer test asserting a snippet produces the expected token kinds.

The lexer is deliberately line-oriented with a small cross-line state (inside a
block comment or a multi-line string), because it must never be slower than
painting: highlighting is a convenience, correctness of the *text* never depends
on it.

## Where to put tests

| You changed | Add a test in |
| --- | --- |
| `buffer`, `search`, `filesystem`, `editing`, `keymap` | `#[cfg(test)] mod tests` in that module |
| A command's user-visible effect | `tests/editing_core.rs` or `tests/workflow.rs` |
| Rendering | `tests/ui_render.rs` |
| An invariant that must hold for all inputs | `tests/property.rs` |
| Something on the hot path | `benches/core_bench.rs` |
