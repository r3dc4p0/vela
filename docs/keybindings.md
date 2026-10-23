# Key bindings

Every binding is a *command*. Commands have stable ids (`file.save`,
`search.findNext`) and can be rebound in `config.toml` or run from the command
palette (`Ctrl+Shift+P`).

Run `vela --list-keys` to print the table for your current configuration.

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
| `Ctrl+Alt+Up` / `Ctrl+Alt+Down` | `edit.addCursorAbove` / `edit.addCursorBelow` | Add a cursor on the line above / below, keeping the column |
| `Alt+Click` | — | Add a cursor, or remove the one already there |
| `Esc` | — | Collapse every cursor back to one |
| `Ctrl+/` | `edit.toggleComment` | Toggle line comments for the file's language |
| `Ctrl+Shift+D` | `edit.duplicateLine` | Duplicate the current line or selection |
| `Ctrl+Shift+K` | `edit.deleteLine` | Delete the current line or selection |
| `Alt+Up` / `Alt+Down` | `edit.moveLineUp` / `edit.moveLineDown` | Move lines |
| `Tab` / `Shift+Tab` | — | Indent / dedent (inserts at the cursor without a selection) |
| `Ctrl+Shift+W` | `view.wordCount` | Word, character and line count |

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
| `Ctrl+Shift+U` | `view.toggleUiStyle` | Pixel (hairline) / cell (block) chrome |
| `Alt+Z` | `view.toggleWhitespace` | Show tabs, spaces and control characters |
| `Alt+W` | `view.toggleWordWrap` | Wrap long lines at the window width |

Word wrap is a session setting rather than a per-buffer one: the wrap width is a
property of the window, so toggling it affects every buffer. It can also be set
in configuration with `[editor] word_wrap = true`. While wrapping is on there is
nothing to scroll to horizontally, so the horizontal wheel does nothing; the
vertical wheel and the cursor motions step by *rows*, not by lines, so a single
wrapped line can fill the window and be scrolled through.

`Ctrl+Shift+U` switches the chrome between the two drawing styles: `pixel`, the
default, which draws borders and dividers as hairlines on the edge of a cell, and
`cell`, which draws them as whole cells of `│` and `─`. It is a session setting
too, and can be set in configuration with `[ui] style = "cell"` or per run with
`vela --ui cell`. The hairlines under the bars use the terminal's underline
colour (SGR 58); on a terminal that ignores it those rules take the colour of the
text above them, which is a shade off but still a line.

## Terminal limitations (read this)

Terminals intercept some keys before an application can see them. Vela works
around this where it can and tells you where it cannot.

1. **`Ctrl+C` sends `SIGINT`.** Many terminals deliver `SIGINT` instead of a key
   event, so copy would kill the editor. Vela installs a `SIGINT` handler that
   restores the terminal and exits cleanly — it does *not* swallow the signal,
   because swallowing it would break `Ctrl+C` in any child process. On terminals
   with the Kitty keyboard protocol (`kitty`, `wezterm`, `ghostty`, recent
   `iTerm2`, `foot`) `Ctrl+C` arrives as a normal key and copies. Where it does
   not, use `Ctrl+Shift+C`, which is bound to `edit.copy` and never reaches the
   terminal as a signal.
2. **`Ctrl+Shift+X` is not distinguishable** from `Ctrl+X` on most terminals:
   the terminal sends the same byte. Vela therefore does not rely on it.
3. **`Ctrl+Tab` may be swallowed** by the terminal or the window manager. It is
   bound, but `Ctrl+PageDown`/`Ctrl+PageUp` are not always available either, so
   `Ctrl+Tab` also works from the command palette (`buffer.next`).
4. **Flow control.** `Ctrl+S` is `XOFF` on terminals with software flow control
   enabled. Vela puts the terminal in raw mode, which disables it, but a terminal
   multiplexer may still intercept it. If `Ctrl+S` does nothing, run
   `stty -ixon` or use the command palette.
5. **`Shift+Tab`** is sent as `BackTab` by some terminals and as `Tab`+`Shift` by
   others; both are handled.
6. **`Ctrl+Alt+Up`/`Ctrl+Alt+Down`** need a terminal that reports the Alt
   modifier with arrow keys. Without the enhanced protocol some terminals send a
   plain arrow, which moves the cursor instead of adding one; `Alt+Click` and
   `Ctrl+D` are the fallbacks. Rebind them with `"ctrl+alt+up"` if you prefer a
   different chord.

The status bar shows `~keys` when Vela is running *without* the enhanced keyboard
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
