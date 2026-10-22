# Roadmap

Ordered by what a user notices first, not by what is easiest.

## Done

- Buffers with tabs, dirty tracking, atomic saves, save-as, save-all, reload
- Character/word/line/document motions, shift-selection, clipboard, undo/redo
- Find, find-next/prev, replace, replace-all, regex, whole word, case sensitivity
- Goto line/column, fuzzy file finder, command palette, help screen
- Line numbers (absolute and relative), syntax highlighting (17 languages),
  dark/light themes, visible whitespace, indent guides, bracket matching
- Auto-indent, comment toggling, duplicate/delete/move lines, trim whitespace
- Multi-cursor: stacked carets, select-next-occurrence, Alt+Click, one undo step
- Mouse: click, drag-selection with edge autoscroll, double/triple-click,
  free wheel scrolling with an off-screen cursor indicator, tab switching
- Word wrap, toggled at runtime: rows are the unit of scrolling, motion and
  hit-testing, and a single line can fill the window
- Recent files, session restore, configuration with validation, logging
- Terminal restoration on panic and on `SIGTERM`/`SIGINT`/`SIGHUP`/`SIGQUIT`
- Encoding detection (UTF-8, UTF-16 with BOM), LF/CRLF/CR round-tripping
- External-modification detection at save time
- Unit, integration, UI and property tests; Criterion benchmarks

## Next

### 1. Live external-change notifications

Today a file changed by another process is detected when you save. A filesystem
watcher (`notify`, watching the *parent* directory because saves are
rename-based) should mark the buffer as externally modified within a second and
offer reload/diff. Depends on: a small `Watcher` type owning the notify handle
and feeding `App`'s existing job channel.

### 2. Split views

One window, two editor panes sharing the buffer list. The renderer already takes
an `Area`, so the work is layout plus a focus model; the risk is key routing, so
it needs its own tests.

### 3. Incremental regex search on large files

Regex currently rescans the whole buffer on each change. Chunked scanning with a
resumable matcher would keep the same responsiveness as literal search.

## Later

- **LSP client**: diagnostics, completion, go-to-definition. Large surface; the
  architecture (typed errors, background jobs, generation counters) is ready for
  it.
- **Plugin system**: an out-of-process plugin protocol beats in-process
  scripting for crash isolation; `Command` would need a dynamic variant.
- **Git integration**: gutter markers and hunk staging, out of process.
- **Terminal image protocols**: not planned; the editor is text-first.
- **OSC 52 clipboard passthrough**: would fix copy over SSH where `arboard` has
  no display to talk to.
- **Splits in a tabbed layout, minimap, folding**: nice, not scheduled.

## Explicitly not planned

- Modal editing (Vim/Emacs key layers). Vela's premise is conventional shortcuts;
  a modal mode would double the input surface and dilute the design.
- A GUI front end. The terminal is the product.
- Anything that requires an embedded scripting runtime in the editor core: it
  conflicts with the startup-time and crash-isolation goals.
