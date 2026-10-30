# Architecture

Vibe is a layered application. Each layer has one job, and the dependencies only
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

Caret *direction* is invisible — a selection looks identical whether it was made
left-to-right or right-to-left — but it decides which end an arrow key moves
from. A caret added by `Ctrl+D` therefore copies the primary's direction, and
`move_cursor` aligns any caret that disagrees before applying the motion. Without
both, a selection made backwards puts the two cursors on opposite ends of their
words and the first keystroke splits the group apart.

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

The result set has two budgets, because the scan runs at two different tempos.

While the query is being typed it is re-scanned on every keystroke, so the cap
is a *latency* budget: `LIVE_MATCHES = 20_000` is reached in ~11 ms even for a
three-letter word that occurs three million times in a 2 GB file. Scanning that
file whole would take 1.7 s, per keystroke, which is not a budget at all.

When the search is committed — Enter, `F3`, Replace All — the rest of the file
is worth having, and the cap becomes a *memory* budget instead:
`FULL_MATCHES = 4_000_000`, about 64 MB of offsets. That scan takes about a
second on 2 GB, so it runs on a worker thread against a `Snapshot` of the rope
and arrives through the same job channel as a background file open. Results are
tagged with the generation and revision they were computed from; a result that
arrives after the query or the text moved on is dropped rather than installed.

Two consequences are deliberate. The focused match is preserved *by identity*
across the install — the full list is a superset of the truncated one in the
same order, so the highlight does not jump when the count grows. And while the
tail is still being found, `next` at the end of the live list reports "searching"
instead of wrapping: wrapping there is precisely the bug the split exists to fix,
because it silently hides everything past the cap.

What the bar opens with is a precedence, not a fallback: the selection, else the
last query, else the word under the cursor. Seeding from the *word* whenever the
caret merely touches one widens the search behind the user's back — select `wor`
inside `word` and the query becomes `word`, a different and larger query than
the text that was pointed at. For the same reason the search is anchored on the
*start* of the selection: the caret sits at its end, and a position at the end of
a match reads as "past it", so opening find on a selected match would otherwise
step straight to the following occurrence.

### Opening a large file

A file over 4 MiB is read on a worker thread, and the panel that appears after
120 ms is the difference between "working" and "hung": two gigabytes take about
1.2 s, and nothing on screen changing for 1.2 s reads as a freeze.

Three rules make it honest and cheap.

**The UI thread only pushes a `Vec`.** The worker reads, decodes, normalises and
builds the rope, then sends a finished `Document` through the job channel;
`App::install` puts it in place. Building the rope on the UI thread instead —
which is what used to happen — cost 0.7 s of frozen window on a 2 GB file, in
the very frame the loader was meant to be covering.

**The percentage is measured, not modelled.** `filesystem::load_with` reports
bytes as the read loop runs, and the count is published through an
`Arc<AtomicU64>` shared with the renderer, so painting costs one relaxed load
and no lock. The bar therefore means "this much of the file is in memory".

**A phase that cannot be measured says so.** Turning text into a rope reports
nothing, so the bar stops showing a percentage and sweeps instead. A bar that
reaches 100% and then waits is worse than one that admits it has finished
measuring, because 100% is a claim about completion.

The delay matters as much as the panel: a 5 MiB file opens in ten milliseconds,
and a panel that flashes for one frame reads as a glitch. The status bar says
"Opening …" immediately, so nothing is hidden while the panel waits.

Two passes over the whole file were removed along the way, because they were
most of the load time: the text is *moved* into a `String` instead of copied
(`body.to_vec()` allocated another 2 GB), and a file with no carriage return
skips line-ending detection and normalisation entirely — one `memchr` answers
both questions, where the old code scanned the file three times and allocated a
copy to ask whether `\r\n` appeared alongside a bare `\n`. Together with the
rope build moving off the UI thread, opening `/tmp/vibe-big.txt` (2 GB) went
from 6.7 s to 1.2 s, with the longest frame during the load at 0.44 ms.

### Display-width-correct rendering

The renderer works in terminal columns, not characters: tabs expand to the next
tab stop, control characters become `^X`, wide CJK/emoji occupy two cells and
combining marks zero. Long lines are rendered through a window that starts at the
first visible character, so opening a 100 MB single-line file does not freeze the
UI. Lines longer than 8 KiB are clipped rather than scanned to the end.

### Two drawing styles, one layout

The chrome can be drawn two ways. `pixel`, the default, is the design the landing
page is drawn from; `cell` is the block-glyph chrome that came before it, kept
behind `ui.style = "cell"`, `--ui cell` and `Ctrl+Shift+U`.

They are two ways to paint one UI, not two UIs. `chrome::Deco` holds the whole
difference — four booleans, no layout: hairline panels instead of boxes, a key
painted brighter than the label it explains, a progress track that is a surface
rather than a run of `░`, and a percentage badge instead of a bare readout.
Everything else is shared, down to the words on screen, and `tests/ui_style.rs`
asserts that: the same file renders the same text in both.

The pixel style is built out of what a cell can hold, which is one glyph. A
border therefore has to be a *fraction* of a cell, and four block elements do
that: `▔` and `▁` draw on the top or bottom eighth, `▏` and `▕` on the left or
right eighth. Three rules make them compose:

- a panel's first and last rows are border rows either way, so a horizontal
  rule is free there. Nowhere else: a rule *between* two bars would cost a whole
  row, or have to underline the text above it, and both of those read as a
  defect rather than a border.
- the title is painted first and the rule skips its cells, which is what makes
  it read as a legend cut into the line instead of a label sitting on top of it.
- the side rules start below the top rule and stop above the bottom one, so the
  corners belong to the horizontals and the seam that leaves is one dot.

Everywhere else, structure is carried by surfaces — the tab strip a shade under
the top bar, the hints a shade under the status bar, the active tab the colour
of the document it opens into — and by vertical hairlines, which cost a column
rather than a row: `▏` between tabs and in front of the line numbers. That is
why the pixel style's gutter is two columns wider than the block style's, and
the only place the two styles lay out differently.

### Indent guides mark a block, not a column of dots

A guide is drawn in the *last* column of its level's indentation — column 3 for
a body indented by four — which is the only column that is still whitespace on
the lines belonging to that level. That is what turns the rules into a block
boundary: they start on the first line of the body, run down every line of it,
and stop at the line that closes it, because that line is outdented and has no
rule at that level. Drawing them at the *first* column of the next level, as
this used to, leaves a one-level block with no rule at all — a function body
looked like plain text.

Two details make it read correctly. A blank line has no indentation of its own,
so it inherits the previous line's rules; without that, every blank line cuts
the block in half. And the rules are spaced by the *file's* indent width
(`Document::indent_style`), not the configured tab width, so a two-space file
gets them two columns apart.

The order matters too: the rules are painted **after** the text, and only into
cells that still hold a space. Painted before, the line painter's own leading
spaces erase every rule it just drew — which is why the feature looked like it
did nothing.

### Mouse hit-testing reads the last frame

Mouse events arrive as screen coordinates, but the mapping from a cell to a
character index depends on the layout of the frame that is currently on screen —
where the editor starts, how wide the gutter is, how far the view is scrolled.
So `ui::draw` publishes the editor and tab rects back into `App`, and
`handle_mouse` maps against those. The tab strip is laid out by a single
`chrome::tab_spans` function that both the painter and the hit-test consume, so
a tab can never be clickable somewhere it was not drawn.

Two position mappings exist on purpose: `buffer_position_at` is strict and
returns nothing for a click in the gutter, while `drag_position_at` clamps, so a
drag that runs off the edge of the text keeps extending to the nearest line
instead of freezing. A drag anchors where the press landed and moves the cursor
with the pointer, which keeps the selection direction — an upwards drag leaves
the cursor on the upper edge, so the next arrow key moves the way the pointer
did.

### The viewport is derived, except when it is not

`EditorRenderer` re-derives the first visible line and column from the cursor on
every frame, which is what keeps motions and edits on screen without the rest of
the code having to think about scrolling. That rule alone makes it impossible to
look anywhere else: any other value of `scroll_top` is overwritten on the next
draw.

`Document::view_detached` is the single exception. A wheel gesture sets it, and
the renderer then leaves both offsets alone — so the view can travel the whole
buffer while the cursor stays put, off screen. Every path that moves the cursor
calls `attach_view`, which is why the invariant is stated as *the view follows
the cursor unless the last thing the user did was scroll*: there is no state to
clear in the input layer, no timeout, and no way for a stale detached view to
survive a keystroke. The status bar reads the same two numbers back to say where
the cursor went (`statusbar_away`), since the caret itself is no longer painted.

Both offsets are bounded by the content, not by the screen: `scroll_top` stops
at the last full page, and `scroll_left` at `Document::visible_width` — the width
of the widest line *among the rows on screen*. Measuring the whole buffer
instead cost eight seconds on an eighteen-million-line file, so the bound is
deliberately local: it is re-measured every frame, which twenty rows can afford,
and it never learns about text the user cannot see.

### Auto-pairs are rules about the text, not a mode

Typing a bracket has to decide something the buffer cannot answer: whether this
`'` opens a string or ends a word. The decision lives in `src/autopair.rs` as a
pure function over the two characters around the caret, the selected text, and
nothing else — so the interesting cases are unit tests rather than keystroke
scripts, and `editing::type_char_at_carets` is only the plumbing that turns a
decision into a `CaretEdit` per caret.

Three decisions carry the feature:

- **The caret is the hard part, not the text.** `(` writes two characters and
  leaves the caret between them, which no existing primitive could express:
  `replace_range` puts the caret after everything it inserted. `CaretEdit` grew
  a `caret_back` — how far to back up from the end of the inserted text — which
  is also what makes Enter between `{}` land on the indented middle line. It
  grew an `anchor_back` for the same reason: surrounding a selection leaves it
  selected, so a second bracket wraps the same text again instead of replacing
  it, and that is a caret *with an anchor* rather than a caret.
- **Stepping over a closer is not an edit.** It is expressed as replacing the
  closer with itself: the batch sees no change, so nothing goes on the undo
  stack, but the caret moves. That is why `apply_edits` places the carets even
  when it changed no text. Undo still sees a pair insertion as one step,
  because it was one `CaretEdit`.
- **Quotes are conditional, brackets are not — and neither pairs inside a
  word.** A quote only pairs where a string can start — after whitespace, an
  opener, or punctuation that is not a word character: `don't`, `3'` and Rust's
  `&'a` are the cases that make an unconditional rule worse than no rule at
  all. A bracket pairs anywhere else, because `(` after a word is still an
  opening bracket. But a caret *between* two word characters is inside a word,
  and a pair there would cut it in half — `hel|lo` must not become `hel(|)lo`.
  A selection is exempt: it is the thing being wrapped, not a point in a word.

The rules never see a paste: `App::insert_text` is the raw path that paste,
recovery and tests use, and `App::type_text` is the one keystrokes go through.
That split is the reason a pasted `(` lands as one character — it came from a
file that already had whatever it needed around it.

### Indentation belongs to the file, not to the settings

`use_spaces` and `tab_width` are global, and a global answer is wrong the moment
somebody else's file is opened: a tab-indented Go file edited with four spaces
gets the wrong indent on every Enter, silently, and the mistake compounds line
by line. `src/indent.rs` reads the file's own convention once, at load;
`Document` carries the result, and `App::indent_style()` prefers it, falling
back to the configuration for new buffers and for files whose text says nothing.

Two decisions:

- **What is detected is the indent *unit*, not the display width.** Whether a
  level is a tab or a run of spaces, and how many spaces. How wide a tab is
  *drawn* stays a view setting: a tab-indented file is correct at any display
  width, and changing it would move every column on screen for a reason that has
  nothing to do with the file.
- **The width comes from the *steps*, not from the indentation itself.** A body
  at eight columns is not an eight-column indent when it sits two levels in; the
  step from the line before it is what says four. When nothing ever steps
  deeper, the shallowest indentation is the only evidence there is. A width
  outside 1–8 is not an indent level at all — a pasted block, a continuation
  line — and the reading is thrown away rather than guessed from.

The scan is bounded (2 000 lines, 512 KiB) because it runs on the load path that
has to stay fast for a two-gigabyte file, and the answer is in the first
screenful of any real file. The status bar names the result — `Spaces: 2`,
`Tabs` — because a detected style can disagree with the settings, and that is
the only place which one is in force is visible.

### Crash recovery is a snapshot, not a journal

Atomic saves protect the file being written. They do nothing for changes that
were never written, and that is the failure an editor cannot apologise for — so
every modified buffer is copied into a snapshot under the data directory
(`recovery/recovery`), a second and a half after the change that made it dirty.

Three decisions carry it:

- **A clean exit deletes the snapshot.** By then the user has been asked whether
  to save, and a "discard" that quietly keeps a copy is a lie. What survives is
  exactly the exit that had no chance to ask: a crash, a `kill`, a terminal that
  went away. A panic unwinds out of the event loop, so `shutdown` never runs and
  the snapshot stays — which is the right bias.
- **The write is debounced, not per keystroke.** Copying eight megabytes on
  every character would be felt; copying it once the typing stops is not. The
  event loop calls `maybe_snapshot` every iteration, and a fingerprint of
  `(id, revision, dirty)` per buffer says whether anything actually changed —
  no hook on each of the paths that can dirty a buffer.
- **A record the disk has overtaken is skipped.** Each record carries the file's
  mtime and length as they were when the buffer was loaded or saved. If either
  has moved, the file was written after the snapshot was taken: the disk holds
  everything the record knows, and applying it would go backwards.

Recovery writes the state straight back before returning. Between reading the
snapshot and the first timer-driven write, the recovered buffers exist only in
memory — and `--smoke` exits without ever reaching the event loop, so it would
otherwise read the snapshot, apply it, and drop it on the floor. Writing back is
atomic, so the old file is replaced in one step rather than cleared and
rewritten.

Buffers over `recovery::LIMIT` (8 MiB) are not copied — rewriting a hundred
megabytes every couple of seconds would be a stutter — but the record keeps the
path and says so, and the startup message names the file rather than silently
dropping it. The format is length-prefixed (`flags line col mtime len path_len
text_len`, then the bytes) so a path with a tab or a newline cannot be misread,
and a truncated file yields the records that were complete.

### Panes are views, not buffers

A pane is `{ buffer, top, row, left, detached, rect }` — which buffer, where in
it, and the rectangle it was given. The buffer itself stays in `App::buffers`,
so two panes on one file are two windows onto one document: an edit in either
shows in both, because there is only one copy of the text and one cursor.

That is what makes the viewport the interesting part. `Document` still owns
`scroll_top`, `scroll_row`, `scroll_left` and `view_detached`, because the
renderer and every motion work by adjusting them — but they are now a *staging
area* for whichever pane is being drawn. `ui::draw` installs a pane's viewport
into its document before rendering it and harvests it back afterwards, so two
panes on one buffer do not fight over one set of offsets.

Two details make that hand-off sound:

- **The active pane is drawn first, and its stored viewport is not installed
  over the document.** Between frames the document is the fresher of the two:
  that is where the wheel and the motions put the scroll. Every other pane is
  the opposite — its document may be showing another pane's position, or
  another buffer's — so it hands its viewport over first.
- **The focus change harvests before it installs.** `focus_pane` copies the
  document's viewport into the pane being left before installing the new one,
  so a motion that has not been drawn yet is not lost.

After the panes are drawn, the active pane's viewport is installed once more, so
everything that runs between frames — the status bar's "the cursor is off
screen" arrow, the next keystroke — reads the right numbers.

**The cursor belongs to the document; the viewport belongs to the pane.** Two
flags keep that boundary, and both are needed because "the active pane" and
"the pane being painted" are not the same question:

- `follows_cursor` — true only for the active pane. An inactive pane renders at
  exactly the viewport it was left at, never re-deriving it from the cursor. If
  it did, moving the cursor in one pane would drag every other pane on the same
  buffer along with it, and two views could never hold different positions.
- `focused` — true for the active pane *and* only while no overlay is open.
  This is what draws the caret, the cursor line and the secondary carets. The
  caret is the terminal's cursor, not a cell, so it is set once per frame by
  whichever pane draws last: an inactive pane that placed it would appear to
  own the cursor.

The wheel follows the pointer rather than the keyboard: `wheel_vertical_at`
installs the pane under the pointer's viewport, scrolls that document at that
pane's width, harvests it back and restores the active one. Focus does not
move — scrolling is looking, and stealing the keyboard because the pointer
drifted would be worse than the bug it fixes.

`App::active` is derived from the active pane rather than stored: it used to be
a field kept in step with the buffer list, and a pane model would have made that
a second source of truth for the same fact.

The layout divides the editing area into columns with one column of hairline
between each pair, and publishes every pane's rect back into the pane — the same
"hit-testing reads the last frame" rule the tab strip follows. A click lands in
the pane it is over, and the divider column belongs to the pane it introduces,
which is also the pane whose edge carries the accent colour.

### A diff is a list of rows, not two files scrolled together

The obvious way to build a side-by-side diff is to open the two files in two
panes and scroll them together. It does not work, and the reason is worth
stating: two files that differ by an inserted line are one line out of step from
that point on, so scrolling both by the same *line* shows line 41 beside line 42
for the rest of the file. Syncing the offsets is easy; syncing the *meaning* is
the problem.

So the alignment is computed once and turned into **rows**. A row is a pair of
line numbers — or one line and a gap — and both columns are drawn from the same
row list at the same index. Sync scrolling then is not a feature that had to be
built: there is one scroll offset, and neither column can drift because neither
column has an offset of its own. It also means a gap is drawn as a gap (a
darker cell with no number) instead of as a blank line that could be mistaken
for content.

`src/diff.rs` computes the rows and knows nothing about the screen;
`src/ui/diff.rs` draws them and knows nothing about alignment; `src/app/diff.rs`
holds the view state — which two buffers, where it is scrolled to, how tall the
last frame was. The view is not a buffer and not a pane: it borrows two
documents that are already open, so it cannot go stale against their text, and
closing either of them closes the view.

**Three decisions carry the performance**, because the textbook implementation
is quadratic:

- **Fingerprints, not text.** Each line is hashed once, in place, without
  allocating — `TextStorage::line_hash` is where a rope implementation can hash
  the line's chunks directly. After that a comparison is an integer compare, and
  the whole of a 100 000-line diff is dominated by the hashing pass (~70 ms).
  128 bits rather than 64 because a collision would silently make two different
  lines equal, and a wrong diff is worse than a slow one.
- **Common prefix and suffix are trimmed first.** Two versions of a file
  usually agree for hundreds of lines at each end, and trimming them is linear.
- **Unique lines are anchors.** What remains is split at lines that occur
  exactly once on both sides, keeping the longest run of them that is in the
  same order (patience diff), and each gap is aligned on its own. A region with
  no unique line is emitted directly: lines paired up, then whatever is left
  over. That pairing is why a rewritten block shows as rows of changed lines
  rather than as every old line followed by every new one.

The result is `O(n log n)` with linear memory and no worst case that blows up.
The price is that the edit script is not guaranteed minimal: a moved block reads
as a deletion plus an insertion rather than as a move, which is what every
side-by-side view shows anyway. Two limits keep the window responsive — 64 MiB
of text and a million rows — and hitting one is reported rather than silently
producing a wrong answer.

**An empty line at the end of a file is not content.** The editor models a file
ending in a newline as having a final empty line, because that is where the
caret goes on `Ctrl+End`. Comparing that line would make `abc` and `abc\n`
different and would draw a phantom last row on every file in the world, so
`diff::content_lines` is what both the engine and the gutters count.

### Word wrap is one function, and everyone calls it

Wrapping looks like a rendering detail and is not: the renderer decides where a
row ends, the cursor motions have to move by those same rows, and the mouse has
to map a cell back to a character through the same break points. Three
implementations would drift, and the symptom — a caret that jumps a character as
it crosses a row boundary — is miserable to attribute.

So `buffer::wrap::LineLayout` is the only implementation, and it is pure: given
a line, a width and a tab width it returns segments, each with a character
offset, a starting column and a width. Segments partition the line, so any
character has exactly one row, and `row_of`/`pos_at` are inverses. The rules are
in that module's documentation; the important one is that whitespace which does
not fit *hangs* past the edge instead of opening the next row with a blank,
which is what stops a narrow window from producing rows containing one space.

The viewport is stored as `(scroll_top, scroll_row)` — the first visible line and
the first visible row within it — rather than as a global visual-row index. A
global index would need the height of every line above it to be useful, which is
O(lines) per frame; `(line, row)` makes every operation proportional to the rows
on screen. `Document` stays terminal-agnostic: the wrap width travels in
`MotionCtx`, the same way `tab_width` and `page` already do, and the mouse
mapping builds a `LayoutCache` for the one click.

Lines longer than `WHOLE_LINE_LIMIT` are never wrapped, anywhere. Wrapping one
would mean copying it to measure every character, which is exactly the cost the
windowed renderer exists to avoid; such a line is one row, and the motions agree.

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

Overlays are opaque because they blank their rectangle first (`chrome::wipe`)
and only then paint. `Buffer::set_style` patches colours but leaves the symbols
in place, so a panel that merely sets a background still shows the editor's text
in every cell it does not write itself — the overlay looks printed *on top of*
the file rather than *over* it. Any new surface that floats above the editor
must wipe before it draws.

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
| Mouse | `tests/mouse.rs`: click, drag and direction, edge autoscroll, multi-click, wheel, tab clicks — coordinates derived from the published layout |
| Scrolling | `tests/scrolling.rs`: the wheel detaching the view from the cursor, the off-screen indicator and its colour, re-attaching on motion and on typing, page clamping |
| Word wrap | `tests/word_wrap.rs`: painting across rows, one line number per line, the toggle and its binding, the view following the caret down a tall line, row-based wheel steps, and click/drag through a wrap boundary |
| Large searches | `tests/search_large.rs`: a document with more matches than the live scan collects — committing finds the rest, `next` reaches the last match before wrapping, `prev` wraps to the true last one, and Replace All does not stop at the cap |
| Loading | `tests/loading.rs`: the panel while a big file opens, its absence for a small one, a bar that tracks bytes, the spinner advancing between frames, and a second open superseding the first |
| Workflows | `tests/workflow.rs`: buffers, search, find seeding from the selection, replace, palette, quit, external changes |
| UI | `tests/ui_render.rs` renders into ratatui's `TestBackend` and asserts on the text of each row |
| Indent guides | `tests/indent_guides.rs`: the rule runs from a block's first line to its last and stops at the closing brace, a blank line inside keeps it and a blank line after does not, a nested block gets its own, the spacing follows the file's indent width, the toggle and its key work, a flat file has none, and a rule never lands on a character |
| Diff | `src/diff.rs`: identical files, an inserted, deleted and rewritten line, a rewritten block (paired rows, not all-removals-then-all-additions), an empty side, nothing in common, duplicated lines with no anchor, a large file with one edit, a block inserted in the middle, and a missing final newline not counting as a change. `tests/diff.rs`: both files side by side, the tint on each side, the gap a one-sided row leaves, columns staying aligned across an insertion, one scroll offset moving both, opening on the first change, `]`/`[` stepping through blocks (and through a tall one), the scroll keys stopping at the ends, `Esc` closing, typing not editing, a buffer compared with itself refused, comparing with an open file and with one that has to be opened, closing a compared file closing the view, and the wheel scrolling |
| Features | `tests/features.rs`: reload (and the confirmation that guards it), recents, session restore, binary refusal, a key override, quick open, trim, regex backreferences, whole-word, UTF-16 and CR round trips, the display toggles, a colour override and the document statistics |
| Indentation | `tests/indent.rs`: a two-space file indents by two and a tab file by a tab, the file beats the settings, a new buffer uses them, `detect_indent = false` turns it off, a file with no indentation falls back, the status bar names the style, Enter and the pair expansion use the file's width, dedent removes one of its levels, two buffers in one session keep their own styles, and the `Ctrl+Shift+I` menu opens on the current style, offers tabs and the three common widths, sends "Other width…" to the width prompt, refuses a bad width without closing, and cancels on Esc |
| Typing | `tests/autopair.rs`: every bracket pairs with the caret inside, a closer is stepped over rather than doubled, a selection is surrounded and stays selected (so it can be surrounded again), backspace removes an empty pair whole, Enter opens a bracket pair onto three lines (and does not touch a quote), quotes stay punctuation after a word or `&`, a paste is never completed, one undo removes a pair, `Ctrl+Shift+A` turns the rules off and on, and each of these holds at every caret |
| Recovery | `tests/recovery.rs`: an unsaved buffer survives a crash with its text and cursor, a clean exit discards the snapshot, a saved buffer is not recovered, an untitled buffer comes back untitled, a file written after the snapshot wins over the copy, a snapshot is applied once, and nothing is written without persistence |
| Panes | `tests/split.rs`: two panes of equal width with a divider between them, one buffer at two scroll positions, an edit visible in both panes, two buffers in two panes, click-to-focus, closing a pane and the refusal to close the last one, a closed buffer leaving no dangling pane index, and a split refused when the panes would be too narrow |
| Drawing styles | `tests/ui_style.rs`: both styles render the same words, the pixel style frames panels in hairlines where the block style boxes them, a title breaks its own rule, no row is spent on a border and no bar is underlined, and the toggle is announced |
| Invariants | `tests/property.rs` with proptest: undo symmetry, cursor bounds (with and without wrapping), search range validity, wrap partitioning and round trips, save/load round trips |
| Performance | `benches/core_bench.rs` (Criterion) |

The integration tests use `App::headless`, which never touches the user's profile
directory and uses an internal-only clipboard, so they are deterministic and can
run in parallel.
