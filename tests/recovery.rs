//! Crash recovery: what survives an exit that had no chance to ask.
//!
//! The tests build the application twice against the same data directory: the
//! first run edits and snapshots, the second run is a fresh process that finds
//! the snapshot and has to make sense of it. That is the only way to test a
//! crash without crashing.

mod common;

use std::path::{Path, PathBuf};

use common::*;
use vibe::config::Config;

/// An application that persists into `data`, and recovers from it on the way in.
fn app_in(dir: &Path, data: &Path) -> vibe::app::App {
    let mut app = vibe::app::App::headless(dir.to_path_buf(), Config::default());
    app.persist = true;
    app.data_dir = data.to_path_buf();
    app.recover_unsaved();
    app
}

fn snapshot_file(data: &Path) -> PathBuf {
    data.join("recovery").join("recovery")
}

#[test]
fn a_crash_keeps_the_unsaved_buffer() {
    let scratch = Scratch::new();
    let data = scratch.path().join("data");
    let path = scratch.file("notes.txt", "one\ntwo\nthree\n");

    {
        let mut app = app_in(scratch.path(), &data);
        open(&mut app, &path);
        type_str(&mut app, "changed ");
        // What the event loop does a second and a half later; then the process
        // dies without a clean exit.
        app.snapshot_now();
        assert!(snapshot_file(&data).exists(), "the snapshot was written");
    }

    let mut app = app_in(scratch.path(), &data);
    assert_eq!(app.buffers.len(), 1, "the buffer came back");
    assert_eq!(
        app.buffers[0].line_text(0),
        "changed one",
        "with the text that was never saved"
    );
    assert!(
        app.buffers[0].is_modified(),
        "and marked as unsaved, so saving it is deliberate"
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "one\ntwo\nthree\n",
        "the file on disk was never touched"
    );
    assert!(
        screen(&mut app, 80, 24)
            .iter()
            .any(|r| r.contains("Recovered 1 unsaved buffer")),
        "and the user is told"
    );
}

#[test]
fn the_cursor_comes_back_where_it_was() {
    let scratch = Scratch::new();
    let data = scratch.path().join("data");
    let path = scratch.file("notes.txt", "one\ntwo\nthree\nfour\n");

    {
        let mut app = app_in(scratch.path(), &data);
        open(&mut app, &path);
        for _ in 0..2 {
            plain(&mut app, crossterm::event::KeyCode::Down);
        }
        type_str(&mut app, "X");
        app.snapshot_now();
    }

    let app = app_in(scratch.path(), &data);
    assert_eq!(
        app.buffers[0].cursor_line_col().0,
        2,
        "on the line that was being edited"
    );
}

#[test]
fn a_clean_exit_throws_the_snapshot_away() {
    let scratch = Scratch::new();
    let data = scratch.path().join("data");
    let path = scratch.file("notes.txt", "text\n");

    {
        let mut app = app_in(scratch.path(), &data);
        open(&mut app, &path);
        type_str(&mut app, "x");
        app.snapshot_now();
        assert!(snapshot_file(&data).exists());
        // The user answered the quit prompt; "discard" has to mean discard.
        app.shutdown();
    }

    assert!(
        !snapshot_file(&data).exists(),
        "a clean exit leaves nothing to recover"
    );
    let app = app_in(scratch.path(), &data);
    assert!(
        app.buffers
            .iter()
            .all(|d| !d.line_text(0).contains("xtext")),
        "the discarded change did not come back: {:?}",
        app.buffers
            .iter()
            .map(|d| d.line_text(0))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "text\n",
        "and the file is untouched"
    );
}

#[test]
fn a_saved_buffer_is_not_recovered() {
    let scratch = Scratch::new();
    let data = scratch.path().join("data");
    let path = scratch.file("notes.txt", "text\n");

    {
        let mut app = app_in(scratch.path(), &data);
        open(&mut app, &path);
        type_str(&mut app, "x");
        app.snapshot_now();
        assert!(snapshot_file(&data).exists());
        ctrl(&mut app, 's');
        app.snapshot_now();
        assert!(
            !snapshot_file(&data).exists(),
            "saving everything empties the snapshot"
        );
    }

    let app = app_in(scratch.path(), &data);
    assert!(!app.buffers[0].is_modified(), "nothing to recover");
}

#[test]
fn an_untitled_buffer_comes_back_untitled() {
    let scratch = Scratch::new();
    let data = scratch.path().join("data");

    {
        let mut app = app_in(scratch.path(), &data);
        type_str(&mut app, "a scratch pad\nsecond line\n");
        app.snapshot_now();
    }

    let app = app_in(scratch.path(), &data);
    let recovered = app
        .buffers
        .iter()
        .find(|d| d.path().is_none())
        .expect("the scratch buffer came back");
    assert_eq!(recovered.line_text(0), "a scratch pad");
    assert!(recovered.is_modified());
}

#[test]
fn a_file_saved_after_the_snapshot_wins() {
    // The snapshot was taken, then the file was written by something else —
    // another editor, a script, a `git checkout`. The disk is newer, so the
    // recovered copy would go backwards.
    let scratch = Scratch::new();
    let data = scratch.path().join("data");
    let path = scratch.file("notes.txt", "text\n");

    {
        let mut app = app_in(scratch.path(), &data);
        open(&mut app, &path);
        type_str(&mut app, "unsaved ");
        app.snapshot_now();
    }

    std::fs::write(&path, "written by someone else\n").unwrap();

    let app = app_in(scratch.path(), &data);
    assert!(
        app.buffers.iter().all(|d| d.line_text(0) != "unsaved text"),
        "the stale copy was not applied: {:?}",
        app.buffers
            .iter()
            .map(|d| d.line_text(0))
            .collect::<Vec<_>>()
    );
}

#[test]
fn recovery_is_idempotent() {
    let scratch = Scratch::new();
    let data = scratch.path().join("data");
    let path = scratch.file("notes.txt", "text\n");

    {
        let mut app = app_in(scratch.path(), &data);
        open(&mut app, &path);
        type_str(&mut app, "x");
        app.snapshot_now();
    }

    let first = app_in(scratch.path(), &data);
    assert_eq!(first.buffers.len(), 1);
    drop(first);

    let second = app_in(scratch.path(), &data);
    assert_eq!(
        second.buffers.len(),
        1,
        "a second start does not duplicate the recovered buffer"
    );
    assert_eq!(second.buffers[0].line_text(0), "xtext");
    assert!(second.buffers[0].is_modified());
}

#[test]
fn the_recovered_state_is_on_disk_immediately() {
    // Between reading the snapshot and the first timer-driven write the work
    // exists only in memory — and a `--smoke` run exits without ever reaching
    // the event loop. The snapshot has to be written back before that.
    let scratch = Scratch::new();
    let data = scratch.path().join("data");
    let path = scratch.file("notes.txt", "text\n");

    {
        let mut app = app_in(scratch.path(), &data);
        open(&mut app, &path);
        type_str(&mut app, "x");
        app.snapshot_now();
    }

    let _app = app_in(scratch.path(), &data);
    let records = vibe::recovery::read(&data.join("recovery"));
    assert_eq!(records.len(), 1, "the snapshot is still there");
    assert_eq!(records[0].text, "xtext\n", "with the recovered text");
}

#[test]
fn nothing_is_written_without_persistence() {
    // Headless runs — every other integration test — must not touch the disk.
    let scratch = Scratch::new();
    let mut app = editor(scratch.path());
    type_str(&mut app, "edited");
    app.snapshot_now();
    assert!(
        !scratch.path().join("data").exists(),
        "no directory was created"
    );
    assert!(app.buffers[0].is_modified(), "the edit still happened");
}

#[test]
fn the_snapshot_only_holds_unsaved_buffers() {
    let scratch = Scratch::new();
    let data = scratch.path().join("data");
    let a = scratch.file("a.txt", "clean\n");
    let b = scratch.file("b.txt", "dirty\n");

    {
        let mut app = app_in(scratch.path(), &data);
        open(&mut app, &a);
        open(&mut app, &b);
        // Only the second one is edited.
        type_str(&mut app, "changed ");
        app.snapshot_now();
    }

    let app = app_in(scratch.path(), &data);
    let recovered: Vec<String> = app
        .buffers
        .iter()
        .filter(|d| d.is_modified())
        .map(|d| d.line_text(0))
        .collect();
    assert_eq!(
        recovered,
        vec!["changed dirty"],
        "only the buffer with unsaved work came back modified"
    );
}
