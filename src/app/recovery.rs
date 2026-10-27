//! Crash recovery, from the application's side.
//!
//! The format and the file live in [`crate::recovery`]; this is the policy:
//! when to write it, when to throw it away, and what to do with it at startup.
//!
//! Two rules shape everything here.
//!
//! * **A clean exit throws the snapshot away.** By then the user has been asked
//!   whether to save, and a "discard" that quietly keeps a copy is a lie. What
//!   survives is exactly the exit that had no chance to ask: a crash, a kill,
//!   a terminal that went away.
//! * **The snapshot is written from a timer, not from the keystroke.** Writing
//!   eight megabytes on every character would be felt; writing a second and a
//!   half after the last change is not. The cost of that delay is at most one
//!   and a half seconds of typing, which is the deal every editor makes.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use tracing::warn;

use super::App;
use crate::buffer::Document;
use crate::indent;
use crate::recovery::{self, Record};

/// How long the editor waits after a change before copying the buffers out.
const SNAPSHOT_DELAY: Duration = Duration::from_millis(1_500);

impl App {
    /// Where the snapshot lives, or `None` when persistence is off.
    fn snapshot_dir(&self) -> Option<PathBuf> {
        self.persistence_on()
            .then(|| self.data_dir.join("recovery"))
    }

    /// True when this session may read and write the user's state.
    ///
    /// Integration tests run with no data directory at all, so they cannot
    /// recover a previous run's buffers or overwrite the real profile.
    pub fn persistence_on(&self) -> bool {
        self.persist && !self.data_dir.as_os_str().is_empty()
    }

    /// A cheap fingerprint of everything the snapshot would contain.
    ///
    /// Document revisions change on every edit and the dirty flag flips on
    /// save, so comparing this against the generation the last snapshot was
    /// written from says whether another write is needed — without hooking
    /// every path that can change a buffer.
    fn recovery_generation(&self) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for doc in &self.buffers {
            for part in [
                doc.id() as u64,
                doc.revision(),
                u64::from(doc.is_modified()),
            ] {
                h ^= part;
                h = h.wrapping_mul(0x100_0000_01b3);
            }
        }
        h
    }

    /// Write the snapshot if the buffers have changed since the last one.
    ///
    /// Called from the event loop, which runs at least every 200 ms.
    pub fn maybe_snapshot(&mut self) {
        if !self.config.files.recover_unsaved {
            return;
        }
        let generation = self.recovery_generation();
        if generation == self.snapshot_generation {
            return;
        }
        if self
            .snapshot_at
            .is_some_and(|at| at.elapsed() < SNAPSHOT_DELAY)
        {
            return;
        }
        self.write_snapshot(generation);
    }

    /// Write the snapshot now, whatever the timer says.
    pub fn snapshot_now(&mut self) {
        let generation = self.recovery_generation();
        self.write_snapshot(generation);
    }

    fn write_snapshot(&mut self, generation: u64) {
        let Some(dir) = self.snapshot_dir() else {
            return;
        };
        let records: Vec<Record> = self
            .buffers
            .iter()
            .filter_map(Record::from_document)
            .collect();
        if records.is_empty() {
            recovery::clear(&dir);
        } else if let Err(e) = recovery::write(&dir, &records) {
            // A snapshot that cannot be written is worth a log line, not a
            // notification: the user cannot act on it, and a toast over every
            // keystroke would be worse than the silence.
            warn!("could not write the recovery snapshot: {e}");
        }
        self.snapshot_generation = generation;
        self.snapshot_at = Some(Instant::now());
    }

    /// Throw the snapshot away.
    ///
    /// Called on a clean exit: the user was given the chance to save, so there
    /// is nothing left that a crash could have taken from them.
    pub fn clear_snapshot(&mut self) {
        if let Some(dir) = self.snapshot_dir() {
            recovery::clear(&dir);
        }
    }

    /// Bring back whatever the last session left unsaved.
    ///
    /// Runs once, at startup, after the buffers have been opened or restored.
    pub fn recover_unsaved(&mut self) {
        if !self.config.files.recover_unsaved {
            return;
        }
        let Some(dir) = self.snapshot_dir() else {
            return;
        };
        let records = recovery::read(&dir);
        if records.is_empty() {
            return;
        }

        let mut recovered = 0usize;
        let mut overtaken = 0usize;
        let mut too_large: Vec<String> = Vec::new();
        let mut first: Option<usize> = None;

        for record in records {
            if record.too_large {
                if let Some(path) = &record.path {
                    too_large.push(display_name(path));
                }
                continue;
            }
            if record.overtaken_by_disk() {
                // Saved after the snapshot was taken: the disk is newer.
                overtaken += 1;
                continue;
            }
            let index = self.apply_record(record);
            first.get_or_insert(index);
            recovered += 1;
        }

        // Write the state straight back. The recovered buffers exist only in
        // memory until they do, and the gap between reading the snapshot and
        // the first timer-driven write is exactly when a crash would take them
        // twice — a `--smoke` run, which exits without ever reaching the loop,
        // would otherwise consume the snapshot and lose it. The write is atomic,
        // so the old file is replaced in one step rather than cleared and
        // rewritten.
        self.snapshot_now();

        if let Some(index) = first {
            self.focus_buffer(index);
        }
        self.ensure_panes();

        if recovered > 0 {
            self.warn(format!(
                "Recovered {recovered} unsaved buffer{} — save or close {}",
                if recovered == 1 { "" } else { "s" },
                if recovered == 1 { "it" } else { "them" }
            ));
        }
        if overtaken > 0 {
            self.info(format!(
                "{overtaken} recovered buffer{} discarded: the file was saved after",
                if overtaken == 1 { "" } else { "s" }
            ));
        }
        for name in too_large {
            self.warn(format!(
                "{name} was too large to keep unsaved changes for — check it"
            ));
        }
    }

    /// Put one recovered record back into the buffer list.
    fn apply_record(&mut self, record: Record) -> usize {
        let mut doc = Document::from_text(&record.text, record.path.clone());
        // Same reading as a normal open: the recovered text is the file, so it
        // is the evidence. Not stored in the record, because it is derivable
        // from the text the record already carries.
        doc.set_indent_style(indent::detect(&record.text, self.indent_options()));
        // It is *not* what is on disk: that is the whole point of the record.
        doc.invalidate_save_point();
        doc.set_disk_meta(record.mtime, record.path.as_ref().map(|_| record.file_len));
        doc.goto_line_col(record.line, record.col);

        // Reuse the buffer the session restore already opened for this path,
        // rather than leaving the file in the list twice.
        let existing = record.path.as_ref().and_then(|path| {
            self.buffers
                .iter()
                .position(|b| b.path() == Some(path.as_path()))
        });
        match existing {
            Some(idx) => {
                let old = self.buffers[idx].id();
                self.syntax_cache.remove(&old);
                self.buffers[idx] = doc;
                idx
            }
            // Otherwise it takes the place a freshly opened file would: the
            // pristine untitled buffer at startup is exactly what `adopt`
            // replaces, so recovery does not leave a blank tab behind.
            None => self.adopt(doc),
        }
    }
}

/// The last component of a path, for a message that has to fit in a bar.
fn display_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn app(dir: &std::path::Path) -> App {
        let mut app = App::headless(dir.to_path_buf(), Config::default());
        app.persist = true;
        app.data_dir = dir.join("data");
        app
    }

    #[test]
    fn a_clean_buffer_produces_no_record() {
        let dir = std::env::temp_dir().join(format!("vela-rec-a-{}", std::process::id()));
        let mut app = app(&dir);
        assert!(Record::from_document(&app.buffers[0]).is_none());
        app.insert_text("changed");
        assert!(Record::from_document(&app.buffers[0]).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_generation_follows_the_buffers() {
        let dir = std::env::temp_dir().join(format!("vela-rec-g-{}", std::process::id()));
        let mut app = app(&dir);
        let clean = app.recovery_generation();
        app.insert_text("x");
        let edited = app.recovery_generation();
        assert_ne!(clean, edited, "an edit changes the generation");
        app.snapshot_now();
        let snapshotted = app.recovery_generation();
        assert_eq!(
            edited, snapshotted,
            "writing the snapshot does not change the buffers"
        );
        app.buffers[0].mark_saved();
        assert_ne!(
            snapshotted,
            app.recovery_generation(),
            "saving changes the generation"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn persistence_is_off_without_a_data_directory() {
        let mut app = App::headless(std::env::temp_dir(), Config::default());
        app.persist = true;
        assert!(
            !app.persistence_on(),
            "headless runs must never touch the profile"
        );
    }
}
