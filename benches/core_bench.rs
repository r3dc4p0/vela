//! Criterion benchmarks for the performance-critical paths.
//!
//! Run with `cargo bench`. The large-file cases allocate ~100 MiB of text plus
//! the rope built from it, so give the machine some room; set
//! `VELA_BENCH_SMALL=1` to run a reduced set (useful in CI).

use std::hint::black_box;
use std::path::PathBuf;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion, Throughput};

use vela::app::App;
use vela::buffer::{Document, Motion, MotionCtx};
use vela::config::Config;
use vela::filesystem::{self, Encoding, Eol};
use vela::search::{find_all, Query};

/// One chunk of realistic-looking prose, ~60 bytes per line.
fn prose(bytes: usize) -> String {
    let line = "The quick brown fox jumps over the lazy dog, then keeps running.\n";
    let mut out = String::with_capacity(bytes + line.len());
    while out.len() < bytes {
        out.push_str(line);
    }
    out.truncate(bytes);
    // Do not leave a broken UTF-8 sequence at the end.
    while !out.is_char_boundary(out.len()) {
        out.pop();
    }
    out
}

fn small_mode() -> bool {
    std::env::var("VELA_BENCH_SMALL").is_ok()
}

fn bench_startup(c: &mut Criterion) {
    let dir = std::env::temp_dir();
    let mut group = c.benchmark_group("startup");

    group.bench_function("config_default", |b| {
        b.iter(|| black_box(Config::default()));
    });

    group.bench_function("app_new_empty", |b| {
        b.iter_batched(
            || (dir.clone(), Config::default()),
            |(cwd, cfg)| black_box(App::headless(cwd, cfg)),
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

fn bench_open(c: &mut Criterion) {
    let mut group = c.benchmark_group("open");

    let one_mib = prose(1024 * 1024);
    group.throughput(Throughput::Bytes(one_mib.len() as u64));
    group.bench_function("parse_1mib", |b| {
        b.iter(|| black_box(Document::from_text(black_box(&one_mib), None)));
    });

    let dir = std::env::temp_dir();
    let path: PathBuf = dir.join("vela-bench-1mib.txt");
    std::fs::write(&path, &one_mib).unwrap();
    group.bench_function("load_1mib_from_disk", |b| {
        b.iter(|| black_box(filesystem::load(black_box(&path)).unwrap()));
    });
    let _ = std::fs::remove_file(&path);

    if !small_mode() {
        let big = prose(100 * 1024 * 1024);
        group.throughput(Throughput::Bytes(big.len() as u64));
        group.sample_size(10);
        group.bench_function("parse_100mib", |b| {
            b.iter(|| black_box(Document::from_text(black_box(&big), None)));
        });

        let big_path = dir.join("vela-bench-100mib.txt");
        std::fs::write(&big_path, &big).unwrap();
        group.bench_function("load_100mib_from_disk", |b| {
            b.iter(|| black_box(filesystem::load(black_box(&big_path)).unwrap()));
        });
        let _ = std::fs::remove_file(&big_path);
    }

    group.finish();
}

fn bench_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("search");

    let one_mib = prose(1024 * 1024);
    let doc = Document::from_text(&one_mib, None);
    let query = Query::literal("lazy dog");
    group.throughput(Throughput::Bytes(one_mib.len() as u64));
    group.bench_function("literal_1mib", |b| {
        b.iter(|| black_box(find_all(doc.storage(), &query, 20_000).unwrap()));
    });

    let rare = Query::literal("not-present-anywhere");
    group.bench_function("literal_1mib_no_match", |b| {
        b.iter(|| black_box(find_all(doc.storage(), &rare, 20_000).unwrap()));
    });

    let case_insensitive = Query {
        text: "Lazy Dog".to_string(),
        case_sensitive: false,
        whole_word: false,
        regex: false,
    };
    group.bench_function("literal_1mib_case_insensitive", |b| {
        b.iter(|| black_box(find_all(doc.storage(), &case_insensitive, 20_000).unwrap()));
    });

    if !small_mode() {
        let big = prose(100 * 1024 * 1024);
        let big_doc = Document::from_text(&big, None);
        group.throughput(Throughput::Bytes(big.len() as u64));
        group.sample_size(10);
        group.bench_function("literal_100mib", |b| {
            b.iter(|| black_box(find_all(big_doc.storage(), &query, 20_000).unwrap()));
        });
        group.bench_function("literal_100mib_no_match", |b| {
            b.iter(|| black_box(find_all(big_doc.storage(), &rare, 20_000).unwrap()));
        });
    }

    group.finish();
}

fn bench_editing(c: &mut Criterion) {
    let mut group = c.benchmark_group("edit");
    let text = prose(1024 * 1024);

    // A persistent buffer, 100 inserts per iteration: this measures the edit
    // path only, without the cost of tearing down a 1 MiB rope after each
    // iteration (which would dominate the number).
    group.bench_function("insert_100_chars_persistent", |b| {
        let mut doc = Document::from_text(&text, None);
        doc.set_cursor(doc.len_chars() / 2);
        b.iter(|| {
            for _ in 0..100 {
                doc.insert_text("x");
            }
            black_box(doc.len_chars())
        });
    });

    group.bench_function("insert_1k_run", |b| {
        b.iter_batched(
            || {
                let mut doc = Document::from_text(&text, None);
                doc.set_cursor(doc.len_chars() / 2);
                doc
            },
            |mut doc| {
                for _ in 0..1000 {
                    doc.insert_text("a");
                }
                black_box(doc.len_chars())
            },
            BatchSize::LargeInput,
        );
    });

    group.bench_function("newline_and_auto_indent", |b| {
        b.iter_batched(
            || {
                let mut doc = Document::from_text("    indented line\n", None);
                doc.set_cursor(doc.len_chars());
                doc
            },
            |mut doc| {
                doc.insert_newline();
                black_box(doc.len_chars())
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("undo_redo_cycle", |b| {
        b.iter_batched(
            || {
                let mut doc = Document::from_text(&text, None);
                doc.set_cursor(doc.len_chars() / 2);
                for _ in 0..50 {
                    doc.insert_text("word ");
                }
                doc
            },
            |mut doc| {
                for _ in 0..50 {
                    doc.undo();
                }
                for _ in 0..50 {
                    doc.redo();
                }
                black_box(doc.len_chars())
            },
            BatchSize::LargeInput,
        );
    });

    group.bench_function("cursor_motion_at_end_of_6mib", |b| {
        let big = prose(6 * 1024 * 1024);
        let mut doc = Document::from_text(&big, None);
        doc.set_cursor(doc.len_chars());
        let ctx = MotionCtx {
            tab_width: 4,
            page: 40,
            wrap_width: None,
        };
        b.iter(|| {
            doc.move_cursor(Motion::Up, false, ctx);
            doc.move_cursor(Motion::LineStart, false, ctx);
            black_box(doc.cursor())
        });
    });

    group.finish();
}

fn bench_render(c: &mut Criterion) {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let mut group = c.benchmark_group("render");
    let text = prose(1024 * 1024);
    let dir = std::env::temp_dir();

    for (label, rows) in [("24_rows", 24u16), ("60_rows", 60)] {
        let mut app = App::headless(dir.clone(), Config::default());
        app.buffers[0].reset_contents(&text);
        let mut terminal = Terminal::new(TestBackend::new(120, rows)).unwrap();
        group.bench_function(label, |b| {
            b.iter(|| {
                terminal.draw(|f| vela::ui::draw(f, &mut app)).unwrap();
            });
        });
    }

    group.finish();
}

fn bench_save(c: &mut Criterion) {
    let mut group = c.benchmark_group("save");
    let text = prose(1024 * 1024);
    let dir = std::env::temp_dir();
    let path = dir.join("vela-bench-save.txt");
    group.throughput(Throughput::Bytes(text.len() as u64));
    group.bench_function("atomic_write_1mib", |b| {
        b.iter(|| {
            filesystem::save(&path, black_box(&text), Eol::Lf, Encoding::Utf8, false).unwrap();
        });
    });
    let _ = std::fs::remove_file(&path);
    group.finish();
}

criterion_group!(
    benches,
    bench_startup,
    bench_open,
    bench_search,
    bench_editing,
    bench_render,
    bench_save
);
criterion_main!(benches);
