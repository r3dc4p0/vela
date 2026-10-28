//! Crash recovery: the unsaved buffers, written where a crash cannot reach them.
//!
//! Atomic saves protect the file being written; they do nothing for changes that
//! were never written. Vibe is used to open a file, change it and move on, so a
//! good deal of work lives in a buffer and nowhere else — and losing it to a
//! closed terminal or a killed process is the one failure the editor cannot
//! apologise for.
//!
//! So every modified buffer is copied into a snapshot under the data directory,
//! a second or two after the edit that changed it. The snapshot is written
//! atomically — temp file, `fsync`, rename — so a crash *during* the write
//! leaves the previous one intact rather than a half-file.
//!
//! A clean exit deletes it. That is deliberate: by then the user has been asked
//! whether to save, and a "discard" that quietly keeps a copy is a lie. What
//! survives is exactly the case that had no chance to ask.
//!
//! On startup the snapshot is read back, and the buffers come back dirty with
//! the cursor where it was. A record whose file has changed on disk since the
//! snapshot was taken is skipped: that file was saved after the copy, so the
//! disk is newer than the snapshot and applying it would go backwards.
//!
//! ## Format
//!
//! One header line, then one record per buffer:
//!
//! ```text
//! vibe-recovery 1
//! <flags> <line> <col> <mtime> <file_len> <path_len> <text_len>
//! <path bytes><text bytes>
//! ```
//!
//! `flags` is a string of markers: `t` for an untitled buffer, `x` for one too
//! large to copy. Lengths are in bytes and the payload follows the header
//! directly, so a path containing a tab or a newline cannot break the parse.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::buffer::Document;

/// Name of the snapshot inside the recovery directory.
const FILE: &str = "recovery";

/// First line of the file, with the format version.
const HEADER: &str = "vibe-recovery 1";

/// The header written before the rename.
///
/// A snapshot is transient, but the one that exists at the moment of the upgrade
/// is the only copy of whatever was unsaved, so it is still read. What is
/// written is always [`HEADER`], so the old spelling drains away on the next
/// write.
const LEGACY_HEADER: &str = "vela-recovery 1";

/// The largest buffer copied into a snapshot.
///
/// Above this the copy would cost more than the recovery is worth — a hundred
/// megabytes rewritten every couple of seconds would be felt as a stutter — so
/// the record keeps the path and says the changes could not be kept. Saying so
/// is the point: silently dropping them is the failure this module exists to
/// prevent.
pub const LIMIT: usize = 8 * 1024 * 1024;

/// One buffer's unsaved state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// The file it came from, or `None` for a buffer that was never saved.
    pub path: Option<PathBuf>,
    /// The buffer's text, empty when `too_large`.
    pub text: String,
    /// Cursor position, as a zero-based line and column.
    pub line: usize,
    pub col: usize,
    /// The file's mtime and length when the buffer was loaded or saved.
    pub mtime: Option<SystemTime>,
    pub file_len: u64,
    /// The buffer was too large to copy; only the path was kept.
    pub too_large: bool,
}

impl Record {
    /// Capture a buffer, if it has anything worth keeping.
    pub fn from_document(doc: &Document) -> Option<Self> {
        if !doc.is_modified() {
            return None;
        }
        let too_large = doc.len_chars() > LIMIT;
        let (line, col) = doc.cursor_line_col();
        Some(Self {
            path: doc.path().map(Path::to_path_buf),
            text: if too_large {
                String::new()
            } else {
                doc.storage().as_string()
            },
            line,
            col,
            mtime: doc.disk_mtime(),
            file_len: doc.disk_len().unwrap_or(0),
            too_large,
        })
    }

    /// True when the file on disk is newer than this record.
    ///
    /// A file saved after the snapshot was taken has a different length or
    /// mtime, which means the disk already holds everything the record knows —
    /// and possibly more.
    pub fn overtaken_by_disk(&self) -> bool {
        let Some(path) = &self.path else {
            return false;
        };
        let Some(mtime) = self.mtime else {
            // No baseline to compare against: recover rather than guess.
            return false;
        };
        let Ok(meta) = fs::metadata(path) else {
            // The file is gone. The buffer still has the text, so keep it.
            return false;
        };
        let len = meta.len();
        let disk_mtime = meta.modified().ok();
        len != self.file_len || disk_mtime.is_some_and(|t| t != mtime)
    }
}

/// Serialise a snapshot.
pub fn encode(records: &[Record]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1024);
    out.extend_from_slice(HEADER.as_bytes());
    out.push(b'\n');
    for record in records {
        let path = record
            .path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut flags = String::new();
        if record.path.is_none() {
            flags.push('t');
        }
        if record.too_large {
            flags.push('x');
        }
        if flags.is_empty() {
            flags.push('-');
        }
        out.extend_from_slice(
            format!(
                "{} {} {} {} {} {} {}\n",
                flags,
                record.line,
                record.col,
                nanos(record.mtime),
                record.file_len,
                path.len(),
                record.text.len()
            )
            .as_bytes(),
        );
        out.extend_from_slice(path.as_bytes());
        out.extend_from_slice(record.text.as_bytes());
    }
    out
}

/// Parse a snapshot, stopping at the first record that does not add up.
///
/// A truncated or foreign file yields whatever records were complete rather
/// than nothing: half the work back is better than none, and the format makes
/// it impossible to misread one record as another.
pub fn decode(bytes: &[u8]) -> Vec<Record> {
    let mut out = Vec::new();
    let Some(rest) = bytes
        .strip_prefix(HEADER.as_bytes())
        .or_else(|| bytes.strip_prefix(LEGACY_HEADER.as_bytes()))
    else {
        return out;
    };
    let Some(mut rest) = rest.strip_prefix(b"\n") else {
        return out;
    };

    while !rest.is_empty() {
        let Some(nl) = rest.iter().position(|&b| b == b'\n') else {
            break;
        };
        let header = &rest[..nl];
        rest = &rest[nl + 1..];
        let Ok(header) = std::str::from_utf8(header) else {
            break;
        };
        let fields: Vec<&str> = header.split(' ').collect();
        let [flags, line, col, mtime, file_len, path_len, text_len] = fields[..] else {
            break;
        };
        let (Ok(line), Ok(col), Ok(mtime), Ok(file_len), Ok(path_len), Ok(text_len)) = (
            line.parse(),
            col.parse(),
            mtime.parse::<u64>(),
            file_len.parse::<u64>(),
            path_len.parse::<usize>(),
            text_len.parse::<usize>(),
        ) else {
            break;
        };
        if rest.len() < path_len + text_len {
            break;
        }
        let (path, rest_after) = rest.split_at(path_len);
        let (text, rest_after) = rest_after.split_at(text_len);
        rest = rest_after;
        let (Ok(path), Ok(text)) = (std::str::from_utf8(path), std::str::from_utf8(text)) else {
            break;
        };
        out.push(Record {
            path: (!flags.contains('t')).then(|| PathBuf::from(path)),
            text: text.to_string(),
            line,
            col,
            mtime: from_nanos(mtime),
            file_len,
            too_large: flags.contains('x'),
        });
    }
    out
}

/// Write the snapshot atomically, replacing any previous one.
pub fn write(dir: &Path, records: &[Record]) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let target = dir.join(FILE);
    let temp = dir.join(format!("{FILE}.tmp"));
    let bytes = encode(records);
    {
        let mut file = fs::File::create(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    if let Err(e) = fs::rename(&temp, &target) {
        let _ = fs::remove_file(&temp);
        return Err(e);
    }
    // Make the rename itself durable, not just the bytes.
    if let Ok(dir) = fs::File::open(dir) {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// Read the snapshot, or nothing when there is none or it cannot be read.
pub fn read(dir: &Path) -> Vec<Record> {
    let bytes = match fs::read(dir.join(FILE)) {
        Ok(bytes) => bytes,
        Err(_) => return Vec::new(),
    };
    decode(&bytes)
}

/// Delete the snapshot. Missing is success.
pub fn clear(dir: &Path) {
    let _ = fs::remove_file(dir.join(FILE));
    let _ = fs::remove_file(dir.join(format!("{FILE}.tmp")));
}

fn nanos(time: Option<SystemTime>) -> u64 {
    time.and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

fn from_nanos(n: u64) -> Option<SystemTime> {
    (n != 0).then(|| UNIX_EPOCH + Duration::from_nanos(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(path: Option<&str>, text: &str) -> Record {
        Record {
            path: path.map(PathBuf::from),
            text: text.to_string(),
            line: 3,
            col: 7,
            mtime: Some(UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
            file_len: text.len() as u64,
            too_large: false,
        }
    }

    #[test]
    fn a_snapshot_round_trips() {
        let records = vec![
            record(Some("/tmp/a.rs"), "fn main() {}\n"),
            record(None, "scratch"),
        ];
        let back = decode(&encode(&records));
        assert_eq!(back, records);
    }

    #[test]
    fn awkward_paths_and_text_survive() {
        // A tab, a newline and a space in the path; a NUL-ish body in the text.
        let mut r = record(
            Some("/tmp/odd\tname with space"),
            "line\nwith\nnewlines\tand \u{0}",
        );
        r.text.push('\u{1F600}');
        let back = decode(&encode(&[r.clone()]));
        assert_eq!(back, vec![r]);
    }

    #[test]
    fn a_truncated_snapshot_keeps_the_whole_records() {
        let records = vec![record(Some("/tmp/a"), "one"), record(Some("/tmp/b"), "two")];
        let bytes = encode(&records);
        // Chop the second record in half.
        let cut = bytes.len() - 2;
        let back = decode(&bytes[..cut]);
        assert_eq!(back, vec![records[0].clone()]);
    }

    #[test]
    fn a_snapshot_written_before_the_rename_is_still_read() {
        // The one that exists at the moment of the upgrade may be the only copy
        // of somebody's unsaved work, so the old header is read even though
        // nothing writes it any more.
        let records = vec![record(Some("/tmp/a"), "unsaved")];
        let text = String::from_utf8(encode(&records)).unwrap();
        let old = text.replacen(HEADER, LEGACY_HEADER, 1);
        assert!(old.starts_with(LEGACY_HEADER));
        assert_eq!(decode(old.as_bytes()), records);
    }

    #[test]
    fn a_foreign_file_is_ignored() {
        assert!(decode(b"not a snapshot at all").is_empty());
        assert!(decode(b"vibe-recovery 1\n").is_empty());
        assert!(decode(b"vibe-recovery 1\ngarbage\n").is_empty());
    }

    #[test]
    fn the_flags_say_what_the_record_could_not_keep() {
        let mut untitled = record(None, "x");
        untitled.too_large = true;
        let back = decode(&encode(&[untitled]));
        assert_eq!(back[0].path, None);
        assert!(back[0].too_large);
    }

    #[test]
    fn writing_replaces_the_previous_snapshot() {
        let dir = std::env::temp_dir().join(format!("vibe-rec-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        write(&dir, &[record(Some("/tmp/a"), "first")]).unwrap();
        assert_eq!(read(&dir).len(), 1);
        write(&dir, &[record(Some("/tmp/a"), "second")]).unwrap();
        let back = read(&dir);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].text, "second");
        assert!(
            !dir.join(format!("{FILE}.tmp")).exists(),
            "the temporary file is renamed away, not left behind"
        );
        clear(&dir);
        assert!(read(&dir).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
