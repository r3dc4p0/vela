# Key bindings

Every binding is a *command*. Commands have stable ids (`file.save`,
`search.findNext`) and can be rebound in `config.toml` or run from the command
palette (`Ctrl+Shift+P`).

Run `vibe --list-keys` to print the table for your current configuration.

## Files

| Key | Command id | Action |
| --- | --- | --- |
| `Ctrl+S` | `file.save` | Save the active buffer (prompts when it has no path) |
| `Ctrl+Shift+S` | `file.saveAs` | Save under a new path |
| `Ctrl+O` | `file.open` | Open a path, with Tab completion |
| `Ctrl+N` | `file.new` | New empty buffer |
| `Ctrl+W` | `file.close` | Close the buffer (asks when dirty) |
| `Ctrl+P` | `file.quickOpen` | Fuzzy file finder rooted at the working directory |
| `Ctrl+Shift+R` | `file.reload` | Reload from disk |
| `Ctrl+Q` | `app.quit` | Quit (asks when there are unsaved changes) |
| `Ctrl+Shift+Q` | `app.quitAll` | Save all and quit |

## Editing

| Key | Command id | Action |
| --- | --- | --- |
| `Ctrl+Z` | `edit.undo` | Undo |
| `Ctrl+Y`, `Ctrl+Shift+Z` | `edit.redo` | Redo |
| `Ctrl+X` / `Ctrl+C` / `Ctrl+V` | `edit.cut` / `edit.copy` / `edit.paste` | Clipboard (line-wise when nothing is selected) |
| `Ctrl+Shift+C` | `edit.copy` | Copy, for terminals that turn `Ctrl+C` into `SIGINT` |
| `Ctrl+A` | `edit.selectAll` | Select the whole document |
| `Ctrl+D` | `edit.addCursorToNext` | Add a cursor at the next occurrence of the selection (the first press selects the word under the cursor); matching is literal, so `he` also matches inside `hello` |
| `Ctrl+Shift+Up` / `Ctrl+Shift+Down` | `edit.addCursorAbove` / `edit.addCursorBelow` | Add a cursor on the line above / below, keeping the column. Hold the key to keep stacking — one cursor per key repeat |
| `Ctrl+Alt+Up` / `Ctrl+Alt+Down` | `edit.addCursorAbove` / `edit.addCursorBelow` | The same two commands, for fingers that learned this chord first |
| `Alt+Click` | — | Add a cursor, or remove the one already there |
| `Esc` | — | Collapse every cursor back to one |
| `Ctrl+/` | `edit.toggleComment` | Toggle line comments for the file's language |
| `Ctrl+Shift+D` | `edit.duplicateLine` | Duplicate the current line or selection |
| `Ctrl+Shift+K` | `edit.deleteLine` | Delete the current line or selection |
| `Alt+Up` / `Alt+Down` | `edit.moveLineUp` / `edit.moveLineDown` | Move lines |
| `Tab` / `Shift+Tab` | — | Indent / dedent (inserts at the cursor without a selection); the unit is the one the file itself uses |
| `Ctrl+Shift+W` | `view.wordCount` | Word, character and line count |
| `Ctrl+Shift+A` | `edit.toggleAutoPairs` | Auto-close brackets and quotes on / off |
| `Ctrl+Shift+I` | `edit.setIndentation` | Choose this buffer's indentation: tabs, 2, 4 or 8 spaces |

## Indentation

Each file's indentation is read when it opens, so Tab and Enter match the file
rather than a global setting. When the reading is wrong — or when you want to
change it — `Ctrl+Shift+I` opens a menu with the choice in it, the current one
selected:

```
 Indentation
 Indent this buffer with:
   Tabs
   Spaces: 2
   Spaces: 4
   Spaces: 8
   Other width…
```

Arrow keys move, Enter takes the row under the cursor, Esc cancels. Because the
menu opens on the style in force, opening it to look and pressing Enter changes
nothing. `Other width…` asks for a number from 1 to 16, or `tab`.

The answer applies to **this buffer**, not to the configuration: a file written
with two spaces is a fact about that file. The settings in `[editor]` are what
new buffers start from. The status bar shows which style is in force
(`Spaces: 2`, `Tabs`), so the result is visible immediately.

## Typing

Brackets and quotes complete themselves. There is no mode to be in; the rules
apply to every keystroke, and `Ctrl+Shift+A` turns the whole set off for the
session — the same thing as `auto_pairs = false` in the configuration file.

| You type | What lands | Why |
| --- | --- | --- |
| `(` `[` `{` | `()`, `[]`, `{}` with the caret between | the pair is one object |
| `(` with text selected | `(selected)`, still selected | a selection is surrounded, not replaced, and stays selected so it can be surrounded again |
| `)` when the next character is `)` | nothing — the caret steps over it | so closing a pair does not double it |
| `"` after a space, `(`, `[` or `{` | `""` with the caret between | a string can start here |
| `"` after a letter, digit, `_`, `&`, `<` or a closing bracket | just `"` | `don't`, `3'` and `&'a` are punctuation, not strings |
| anything between two word characters (`hel\|lo`) | just the character | a pair there would cut the word in half |
| `Enter` between `{}`, `[]` or `()` | the pair opens onto three lines, the closer on its own | the block has an inside |
| `Backspace` between an empty pair | both halves | half a pair is never useful |

Two boundaries are deliberate. **Paste is not typing**: a `(` that arrives from
the clipboard lands as one character, because it came from a file that already
had whatever it needed around it. And **the caret is a point**: typing with no
selection looks at the characters either side of it, which is why `don't` works,
`"don` starts a string, and `hel|lo` is left alone. A selection is not a
point — it is the thing being wrapped, so it is surrounded wherever it is.

Opening a pair onto three lines needs `auto_indent` as well as `auto_pairs`,
since it is indentation: with `auto_indent = false`, `Enter` between braces is
just a newline.

## Mouse

Mouse support is on by default; turn it off with `mouse = false` in the `[ui]`
section.

| Gesture | Action |
| --- | --- |
| Click | Place the cursor |
| Drag | Select from the press point; the view autoscrolls when the pointer moves above or below the text, and an upwards drag leaves the cursor on the upper edge |
| Double-click | Select the word |
| Triple-click | Select the line |
| `Alt+Click` | Add a cursor, or remove the one already there |
| Wheel up / down | Scroll three lines |
| Wheel left / right | Scroll four columns (terminals that report it) |
| Click a tab | Switch to that buffer |

The wheel moves the view, never the cursor: it can travel the whole buffer and
leaves the caret where it was, even off screen. While the caret is out of view
the status bar adds its line in amber, with `↑` or `↓` for the direction to
scroll back. Any cursor movement — a key, a click, typing — brings the view
back to the caret and drops the indicator.

Scrolling stops where the text does: vertically at the last full page, and
horizontally once the longest line ends.

## Navigation

| Key | Command id | Action |
| --- | --- | --- |
| `Ctrl+G` | `nav.gotoLine` | Go to line (`42`) or line and column (`42:7`) |
| `Ctrl+Home` / `Ctrl+End` | `nav.bufferStart` / `nav.bufferEnd` | Document start / end |
| `Home` / `End` | — | Smart home (first non-blank), line end |
| `Ctrl+Left` / `Ctrl+Right` | — | Word left / right |
| `PageUp` / `PageDown` | — | Page |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | `buffer.next` / `buffer.prev` | Next / previous buffer |

## Search

| Key | Command id | Action |
| --- | --- | --- |
| `Ctrl+F` | `search.find` | Incremental find, pre-filled with the selection (else the last query, else the word under the cursor) |
| `Ctrl+H` | `search.replace` | Find and replace |
| `F3` / `Shift+F3` | `search.findNext` / `search.findPrev` | Repeat the last search |
| `Alt+C` / `Alt+W` / `Alt+R` | — | Toggle case sensitivity / whole word / regex |
| `Enter` | — | Next match (find) or replace one (replace, when focused) |
| `Alt+Enter` | — | Replace all |
| `Esc` | — | Close the bar, keeping the search active |

## View and help

| Key | Command id | Action |
| --- | --- | --- |
| `F1` | `help.show` | Help screen, generated from the live keymap |
| `Ctrl+Shift+P` | `help.commandPalette` | Command palette |
| `Ctrl+Shift+T` | `view.toggleTheme` | Dark / light |
| `Ctrl+Shift+U` | `view.toggleUiStyle` | Hairline chrome / block chrome |
| `Ctrl+Shift+G` | `view.toggleIndentGuides` | Rule between indentation blocks on / off |
| `Alt+Z` | `view.toggleWhitespace` | Show tabs, spaces and control characters |
| `Alt+W` | `view.toggleWordWrap` | Wrap long lines at the window width |

### Panes

| Key | Command id | Action |
| --- | --- | --- |
| `Ctrl+\` | `view.splitPane` | Split the pane, showing the same buffer to its right |
| `Ctrl+Alt+W` | `view.closePane` | Close the pane with the focus |
| `Ctrl+Alt+Left` | `view.focusPaneLeft` | Focus the pane on the left (wraps) |
| `Ctrl+Alt+Right` | `view.focusPaneRight` | Focus the pane on the right (wraps) |

A split divides the editing area into columns; there is one column of hairline
between each pair, drawn in the accent colour on the edge of the pane that has
the focus. Clicking anywhere in a pane focuses it, including on that hairline.

Panes are views, not buffers. Splitting shows the *same* buffer twice, at the
same place, and the two scroll independently — which is the point: one file,
two places in it. Loading another buffer into a pane (clicking its tab, `Ctrl+N`,
or opening a file) gives the two panes different buffers, and an edit in either
pane of a shared buffer appears in both, because there is only ever one
document.

A pane closes with `Ctrl+Alt+W`; the last one cannot be closed. A split is
refused when the panes would be narrower than 24 columns, and says so rather
than drawing something unreadable.

Word wrap is a session setting rather than a per-buffer one: the wrap width is a
property of the window, so toggling it affects every buffer. It can also be set
in configuration with `[editor] word_wrap = true`. While wrapping is on there is
nothing to scroll to horizontally, so the horizontal wheel does nothing; the
vertical wheel and the cursor motions step by *rows*, not by lines, so a single
wrapped line can fill the window and be scrolled through.

`Ctrl+Shift+U` switches how the chrome is drawn. `pixel`, the default, spends a
fraction of a cell on a border: panels are framed by hairlines on the edge of
their rows and columns, tabs are split by `▏`, and the line-number gutter is
ruled off from the text. `cell` is the block-glyph chrome, where a border is a
whole cell of `│` and `─`. It is a session setting, and can be set in
configuration with `[ui] style = "cell"` or per run with `vibe --ui cell`.

## Terminal limitations (read this)

Terminals intercept some keys before an application can see them. Vibe works
around this where it can and tells you where it cannot.

1. **`Ctrl+C` sends `SIGINT`.** Many terminals deliver `SIGINT` instead of a key
   event, so copy would kill the editor. Vibe installs a `SIGINT` handler that
   restores the terminal and exits cleanly — it does *not* swallow the signal,
   because swallowing it would break `Ctrl+C` in any child process. On terminals
   with the Kitty keyboard protocol (`kitty`, `wezterm`, `ghostty`, recent
   `iTerm2`, `foot`) `Ctrl+C` arrives as a normal key and copies. Where it does
   not, use `Ctrl+Shift+C`, which is bound to `edit.copy` and never reaches the
   terminal as a signal.
2. **`Ctrl+Shift+X` is not distinguishable** from `Ctrl+X` on most terminals:
   the terminal sends the same byte. Vibe therefore does not rely on it.
3. **`Ctrl+Tab` may be swallowed** by the terminal or the window manager. It is
   bound, but `Ctrl+PageDown`/`Ctrl+PageUp` are not always available either, so
   `Ctrl+Tab` also works from the command palette (`buffer.next`).
4. **Flow control.** `Ctrl+S` is `XOFF` on terminals with software flow control
   enabled. Vibe puts the terminal in raw mode, which disables it, but a terminal
   multiplexer may still intercept it. If `Ctrl+S` does nothing, run
   `stty -ixon` or use the command palette.
5. **`Shift+Tab`** is sent as `BackTab` by some terminals and as `Tab`+`Shift` by
   others; both are handled.
6. **`Ctrl+Shift+Up`/`Ctrl+Shift+Down`** add a cursor above or below, and holding
   the key keeps adding — one cursor per key repeat. **`Ctrl+Alt+Up`/`Ctrl+Alt+Down`**
   do the same, but need a terminal that reports the Alt modifier with arrow
   keys: without the enhanced protocol some terminals send a plain arrow, which
   moves the cursor instead of adding one — which is why `Ctrl+Shift` exists.
   `--list-keys` shows both. If neither arrives, `Alt+Click` and `Ctrl+D` are the
   fallbacks, and any chord can be rebound in the configuration.

The status bar shows `~keys` when Vibe is running *without* the enhanced keyboard
protocol, so you can tell at a glance which set of caveats applies.

## Rebinding

```toml
[keys]
"ctrl+shift+p" = "help.commandPalette"
"f5"           = "file.reload"
"ctrl+k"       = "edit.deleteLine"
"ctrl+q"       = ""            # an empty value removes the binding
```

Binding strings are `modifier+key`, where modifiers are `ctrl`, `alt`, `shift`
and `super`, and the key is a character, a named key (`enter`, `esc`, `tab`,
`space`, `backspace`, `delete`, `insert`, `home`, `end`, `pageup`, `pagedown`,
`up`, `down`, `left`, `right`) or a function key (`f1`–`f12`). A binding that
names an unknown command is reported at startup and ignored; it never prevents
the editor from starting.
