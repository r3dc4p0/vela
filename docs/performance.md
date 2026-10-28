# Performance

## Method

- **Machine**: Apple M3, 24 GiB, macOS 27.0.1 (arm64).
- **Toolchain**: rustc 1.90.0, `[profile.bench]` = opt-level 3, thin LTO,
  codegen-units 1, debug symbols on.
- **Benchmarks**: Criterion (`benches/core_bench.rs`), warm-up 1 s, measurement
  3 s. Run with `cargo bench`. Set `VIBE_BENCH_SMALL=1` to skip the 100 MiB cases.
- **Startup**: measured separately by spawning the release binary inside a
  pseudo-terminal with `--smoke` (start, draw one frame, exit) and timing
  wall-clock from `fork` to process exit, 25 runs.

Nothing here is a synthetic microbenchmark of a function that the editor does not
call: every case goes through the same code path the UI uses.

## Results

### Startup

| Case | Time |
| --- | --- |
| Launch → first frame → exit (pty, median of 25) | **7.0 ms** (min 6.3, p95 8.0) |
| Process spawn + config load + `--print-config` | 7.1 ms |
| `App` construction with an empty buffer | 19.4 µs |
| Configuration defaults | 33 ns |

The 7 ms is almost entirely process spawn and dynamic linking: a baseline
`--version` run costs 6.9 ms on the same machine. Vibe's own work before the first
frame is therefore well under 1 ms, and the 100 ms budget is met with two orders
of magnitude of headroom.

### Opening files

| Case | Time | Throughput |
| --- | --- | --- |
| Parse 1 MiB into a rope | 258 µs | 3.9 GiB/s |
| Load 1 MiB from disk (read + decode + detect EOL) | 1.26 ms | 793 MiB/s |
| Parse 100 MiB | 26.6 ms | 3.8 GiB/s |
| Load 100 MiB from disk | 136 ms | 735 MiB/s |

100 MiB opens in 136 ms, well inside the 1 s budget. Files above 4 MiB are loaded
on a worker thread, so the UI paints an "Opening …" notice instead of freezing
even for the largest file.

### Search

| Case | Time | Note |
| --- | --- | --- |
| 1 MiB, needle found everywhere | 1.10 ms | stops at the 20 000-match cap |
| 1 MiB, no match (full scan) | 457 µs | true worst case |
| 1 MiB, case-insensitive | 1.00 ms | Unicode folding path |
| 100 MiB, needle found everywhere | 4.94 ms | capped at 20 000 matches |
| 100 MiB, no match (full scan) | 23.3 ms | true worst case |

A full scan of 100 MiB takes 23 ms, 20× under the 500 ms budget. The
match-everywhere cases are *faster* than the no-match cases because they hit the
match cap early and stop scanning.

### Editing

| Case | Time | Per keystroke |
| --- | --- | --- |
| 100 inserts into the middle of a 1 MiB buffer | 29.3 µs | **293 ns** |
| 1 000 inserts (typing run) | 317 µs | 317 ns |
| Enter with auto-indent | 857 ns | — |
| 50 undo + 50 redo | 27.3 µs | 273 ns per step |
| Cursor up + line start at the end of a 6 MiB buffer | 302 ns | — |

Typing costs a fraction of a microsecond per character, so the 50 ms frame budget
is never the edit path's fault.

### Rendering

| Case | Time |
| --- | --- |
| Full frame, 120×24, 1 MiB buffer | 70.5 µs |
| Full frame, 120×60, 1 MiB buffer | 180 µs |

A frame is 70–180 µs including layout, gutter, syntax tokens, selection and match
highlighting, which leaves the whole 16 ms budget of a 60 Hz redraw unused. Only
the visible window is touched: a 100 MiB file costs the same as a 1 KiB file to
render.

### Saving

| Case | Time | Throughput |
| --- | --- | --- |
| Atomic write of 1 MiB | 9.5 ms | 105 MiB/s |

Saving is dominated by the two `fsync` calls that make the write atomic and
durable. That is deliberate: correctness beats a few milliseconds, and the
alternative (write in place) can destroy the file if the process dies mid-write.

## Where the time goes

- **Open** is dominated by `fs::read` + UTF-8 validation; rope construction is an
  order of magnitude cheaper than the I/O.
- **Search** is dominated by `memchr`'s SIMD scan; the char-offset bookkeeping is
  fused into the same pass. The case-insensitive path is a Unicode-aware window
  scan and costs about 2× the ASCII path.
- **Editing** is dominated by the rope's insert plus the undo record (one small
  `String`). Coalescing keeps the history from growing during a typing run.
- **Rendering** is dominated by cell writes and style resolution; the syntax
  cache makes re-lexing proportional to edited lines, not file size.

## Reproducing

```sh
cargo bench                          # everything (~2 min)
VIBE_BENCH_SMALL=1 cargo bench       # skip the 100 MiB cases
cargo bench -- "search/"             # one group
```

Criterion writes HTML reports to `target/criterion/`. Compare runs across commits
with `cargo bench -- --save-baseline before` / `--baseline before`.

## Limits of these numbers

- Single machine, single run; expect ±10 % between runs (Criterion prints the
  confidence interval, which is included above).
- The 100 MiB cases allocate ~100 MiB of text plus the rope (roughly 3× the file
  size in RSS), so they need a machine with headroom.
- The pty startup measurement includes shell-side `fork`/`exec` cost, which is
  why the baseline (`--version`, 6.9 ms) is quoted next to it.
