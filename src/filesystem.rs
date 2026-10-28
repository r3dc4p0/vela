//! File loading and saving.
//!
//! Data integrity rules implemented here:
//!
//! * **Never truncate before the new content is ready.** Saves write to a
//!   temporary file in the destination directory, `fsync` it, then `rename` it
//!   over the target. A crash mid-write leaves the original untouched.
//! * **Never guess an encoding destructively.** UTF-8, UTF-8-with-BOM and
//!   UTF-16 with BOM are decoded; anything containing NUL bytes without a
//!   UTF-16 BOM is refused as binary rather than silently mangled.
//! * **Preserve line endings.** The dominant convention is detected on load and
//!   re-applied on save.
//! * **Preserve permissions.** The temporary file inherits the target's mode
//!   before the rename, so `chmod 600` secrets stay private.
//! * **Follow symlinks.** The canonical target is resolved first, so saving a
//!   symlinked file updates the target instead of replacing the link.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::error::{Error, Result};

/// Bytes read per `read` call. Large enough that the syscall overhead vanishes,
/// small enough that progress moves smoothly on a slow disk.
const READ_CHUNK: usize = 1024 * 1024;

/// Line ending convention of a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Eol {
    #[default]
    Lf,
    CrLf,
    Cr,
}

impl Eol {
    pub fn as_str(self) -> &'static str {
        match self {
            Eol::Lf => "\n",
            Eol::CrLf => "\r\n",
            Eol::Cr => "\r",
        }
    }

    /// Short label used in the status bar.
    pub fn label(self) -> &'static str {
        match self {
            Eol::Lf => "LF",
            Eol::CrLf => "CRLF",
            Eol::Cr => "CR",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        match label.to_ascii_lowercase().as_str() {
            "lf" | "unix" => Some(Eol::Lf),
            "crlf" | "dos" | "windows" => Some(Eol::CrLf),
            "cr" | "mac" => Some(Eol::Cr),
            _ => None,
        }
    }

    /// Detect the dominant convention. Ties resolve to LF.
    pub fn detect(text: &str) -> Self {
        EolCounts::of(text).dominant()
    }
}

/// Text encoding a buffer was loaded from / will be saved as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Encoding {
    #[default]
    Utf8,
    Utf16Le,
    Utf16Be,
}

impl Encoding {
    pub fn label(self) -> &'static str {
        match self {
            Encoding::Utf8 => "UTF-8",
            Encoding::Utf16Le => "UTF-16 LE",
            Encoding::Utf16Be => "UTF-16 BE",
        }
    }
}

/// A file that has been read from disk and normalised for editing.
#[derive(Debug, Clone)]
pub struct LoadedFile {
    /// Content with `\n` line endings, BOM stripped, UTF-16 decoded.
    pub text: String,
    pub eol: Eol,
    pub encoding: Encoding,
    pub bom: bool,
    pub mtime: Option<SystemTime>,
    pub len: u64,
    /// True when the file mixed line endings and was normalised.
    pub mixed_eol: bool,
}

/// Read `path` into a [`LoadedFile`].
pub fn load(path: &Path) -> Result<LoadedFile> {
    load_with(path, |_| {})
}

/// Read `path`, reporting how many bytes have arrived so far.
///
/// The callback runs on the calling thread — on a worker thread when the file
/// is large — and exists so the UI can show real progress instead of a frozen
/// frame. It is called once per chunk, so a 2 GB file costs two thousand calls
/// rather than two billion.
pub fn load_with(path: &Path, mut on_bytes: impl FnMut(u64)) -> Result<LoadedFile> {
    let meta = fs::metadata(path).map_err(|e| Error::open(path, e))?;
    if meta.is_dir() {
        return Err(Error::IsDirectory(path.to_path_buf()));
    }

    let mut file = File::open(path).map_err(|e| Error::open(path, e))?;
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    // One chunk of buffer, no larger than the file it is about to hold: a
    // twenty-byte file should not allocate a megabyte to be read.
    let mut chunk = vec![0u8; READ_CHUNK.min((meta.len() as usize).max(1))];
    loop {
        let n = file.read(&mut chunk).map_err(|e| Error::open(path, e))?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n]);
        on_bytes(bytes.len() as u64);
    }

    let (encoding, bom) = detect_encoding(&bytes, path)?;

    // The buffer is *moved* into the string, not copied. It used to go through
    // `body.to_vec()`, which for a 2 GB file allocated another 2 GB and copied
    // every byte of it — a quarter of the time it takes to open the file, spent
    // making a duplicate of something that was about to be discarded.
    let bom_len = if bom {
        match encoding {
            Encoding::Utf8 => 3,
            Encoding::Utf16Le | Encoding::Utf16Be => 2,
        }
    } else {
        0
    };
    let text = match encoding {
        Encoding::Utf8 => {
            bytes.drain(..bom_len);
            String::from_utf8(bytes).map_err(|_| Error::UnsupportedEncoding {
                path: path.to_path_buf(),
                encoding: "invalid UTF-8".to_string(),
            })?
        }
        Encoding::Utf16Le => decode_utf16(&bytes[bom_len..], true, path)?,
        Encoding::Utf16Be => decode_utf16(&bytes[bom_len..], false, path)?,
    };

    // A file with no carriage return anywhere — the overwhelming majority — has
    // nothing to detect and nothing to normalise: one `memchr` answers both
    // questions, and the text is left untouched instead of being copied.
    let (eol, mixed_eol, text) = if text.contains('\r') {
        let counts = EolCounts::of(&text);
        (counts.dominant(), counts.mixed(), normalize_eol(&text))
    } else {
        (Eol::Lf, false, text)
    };

    Ok(LoadedFile {
        text,
        eol,
        encoding,
        bom,
        mtime: meta.modified().ok(),
        len: meta.len(),
        mixed_eol,
    })
}

/// Counts of each line-ending convention in a text.
///
/// The dominant convention and "does this file mix them" are two answers to the
/// same question, and they used to be asked in three separate passes over the
/// whole file — one counting, one `replace` that allocated a copy of the file
/// to ask whether a substring existed, and one to normalise.
#[derive(Debug, Default, Clone, Copy)]
struct EolCounts {
    crlf: usize,
    lf: usize,
    cr: usize,
}

impl EolCounts {
    fn of(text: &str) -> Self {
        let bytes = text.as_bytes();
        let mut counts = Self::default();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\r' => {
                    if bytes.get(i + 1) == Some(&b'\n') {
                        counts.crlf += 1;
                        i += 1;
                    } else {
                        counts.cr += 1;
                    }
                }
                b'\n' => counts.lf += 1,
                _ => {}
            }
            i += 1;
        }
        counts
    }

    /// Ties resolve to LF.
    fn dominant(&self) -> Eol {
        if self.crlf >= self.lf && self.crlf >= self.cr && self.crlf > 0 {
            Eol::CrLf
        } else if self.cr > self.lf && self.cr > 0 {
            Eol::Cr
        } else {
            Eol::Lf
        }
    }

    fn mixed(&self) -> bool {
        self.crlf > 0 && (self.lf > 0 || self.cr > 0)
    }
}

fn detect_encoding(bytes: &[u8], path: &Path) -> Result<(Encoding, bool)> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return Ok((Encoding::Utf8, true));
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return Ok((Encoding::Utf16Le, true));
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return Ok((Encoding::Utf16Be, true));
    }
    // No BOM: a NUL byte in the head of the file means binary content. Refusing
    // is safer than opening and re-saving a corrupted file.
    let head = &bytes[..bytes.len().min(8192)];
    if head.contains(&0) {
        return Err(Error::Binary {
            path: path.to_path_buf(),
        });
    }
    Ok((Encoding::Utf8, false))
}

fn decode_utf16(body: &[u8], little_endian: bool, path: &Path) -> Result<String> {
    if body.len() % 2 != 0 {
        return Err(Error::UnsupportedEncoding {
            path: path.to_path_buf(),
            encoding: "truncated UTF-16".to_string(),
        });
    }
    let units: Vec<u16> = body
        .chunks_exact(2)
        .map(|c| {
            if little_endian {
                u16::from_le_bytes([c[0], c[1]])
            } else {
                u16::from_be_bytes([c[0], c[1]])
            }
        })
        .collect();
    String::from_utf16(&units).map_err(|_| Error::UnsupportedEncoding {
        path: path.to_path_buf(),
        encoding: "invalid UTF-16".to_string(),
    })
}

/// Convert CRLF and lone CR to LF.
pub fn normalize_eol(text: &str) -> String {
    if !text.contains('\r') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

/// Convert internal LF text to the requested line ending.
pub fn denormalize_eol(text: &str, eol: Eol) -> String {
    match eol {
        Eol::Lf => text.to_string(),
        Eol::CrLf => text.replace('\n', "\r\n"),
        Eol::Cr => text.replace('\n', "\r"),
    }
}

/// Outcome of a successful save.
#[derive(Debug, Clone, Copy)]
pub struct SaveOutcome {
    pub bytes_written: u64,
}

/// Atomically write `text` to `path`.
pub fn save(
    path: &Path,
    text: &str,
    eol: Eol,
    encoding: Encoding,
    bom: bool,
) -> Result<SaveOutcome> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            return Err(Error::save(
                path,
                io::Error::new(io::ErrorKind::NotFound, "directory does not exist"),
            ));
        }
    }
    if path.is_dir() {
        return Err(Error::IsDirectory(path.to_path_buf()));
    }

    // Resolve symlinks so we update the target file, not the link.
    let target = match fs::canonicalize(path) {
        Ok(p) => p,
        Err(_) => path.to_path_buf(),
    };
    let dir = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    let bytes = encode(text, eol, encoding, bom);

    let tmp_path = dir.join(format!(
        ".{}.vibe-tmp-{}-{}",
        target
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "buffer".to_string()),
        std::process::id(),
        unique_suffix()
    ));

    let result = (|| -> io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);

        // Preserve the original mode (and ownership where permitted) before the
        // rename so there is no window where the file is world readable.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = fs::metadata(&target) {
                let mode = meta.permissions().mode();
                let _ = fs::set_permissions(&tmp_path, fs::Permissions::from_mode(mode));
            }
        }

        fs::rename(&tmp_path, &target)?;
        sync_dir(&dir);
        Ok(())
    })();

    if let Err(e) = result {
        let _ = fs::remove_file(&tmp_path);
        return Err(Error::save(path, e));
    }

    Ok(SaveOutcome {
        bytes_written: bytes.len() as u64,
    })
}

fn unique_suffix() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn encode(text: &str, eol: Eol, encoding: Encoding, bom: bool) -> Vec<u8> {
    let converted = denormalize_eol(text, eol);
    match encoding {
        Encoding::Utf8 => {
            let mut out = Vec::with_capacity(converted.len() + 3);
            if bom {
                out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
            }
            out.extend_from_slice(converted.as_bytes());
            out
        }
        Encoding::Utf16Le | Encoding::Utf16Be => {
            let mut out = Vec::with_capacity(converted.len() * 2 + 2);
            if bom {
                out.extend_from_slice(if encoding == Encoding::Utf16Le {
                    &[0xFF, 0xFE]
                } else {
                    &[0xFE, 0xFF]
                });
            }
            for unit in converted.encode_utf16() {
                let bytes = if encoding == Encoding::Utf16Le {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                };
                out.extend_from_slice(&bytes);
            }
            out
        }
    }
}

/// Flush the directory entry so the rename survives a power loss (Unix only).
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    {
        if let Ok(f) = File::open(dir) {
            let _ = f.sync_all();
        }
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
}

/// Current metadata of `path`, used for external-modification detection.
pub fn metadata_of(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// True when the file changed on disk since `mtime`/`len` were recorded.
///
/// Comparison is timestamp based, which is what every editor does; the length is
/// used as a tie breaker for filesystems with coarse timestamps.
pub fn changed_on_disk(path: &Path, mtime: Option<SystemTime>, len: Option<u64>) -> bool {
    let Some((current_mtime, current_len)) = metadata_of(path) else {
        return false;
    };
    match (mtime, len) {
        (Some(prev), Some(prev_len)) => current_mtime != prev || current_len != prev_len,
        (Some(prev), None) => current_mtime != prev,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn detects_lf() {
        assert_eq!(Eol::detect("a\nb\nc"), Eol::Lf);
    }

    #[test]
    fn detects_crlf_and_cr() {
        assert_eq!(Eol::detect("a\r\nb\r\n"), Eol::CrLf);
        assert_eq!(Eol::detect("a\rb\r"), Eol::Cr);
    }

    #[test]
    fn normalises_and_restores_eol() {
        let text = "a\r\nb\rc\n";
        let n = normalize_eol(text);
        assert_eq!(n, "a\nb\nc\n");
        assert_eq!(denormalize_eol(&n, Eol::CrLf), "a\r\nb\r\nc\r\n");
        assert_eq!(denormalize_eol(&n, Eol::Lf), "a\nb\nc\n");
    }

    #[test]
    fn roundtrips_crlf_file() {
        let dir = tmp();
        let path = dir.path().join("crlf.txt");
        fs::write(&path, b"one\r\ntwo\r\n").unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.eol, Eol::CrLf);
        assert_eq!(loaded.text, "one\ntwo\n");
        save(&path, &loaded.text, loaded.eol, loaded.encoding, loaded.bom).unwrap();
        let raw = fs::read(&path).unwrap();
        assert_eq!(raw, b"one\r\ntwo\r\n");
    }

    #[test]
    fn preserves_utf8_bom() {
        let dir = tmp();
        let path = dir.path().join("bom.txt");
        fs::write(&path, b"\xEF\xBB\xBFhi\n").unwrap();
        let loaded = load(&path).unwrap();
        assert!(loaded.bom);
        assert_eq!(loaded.text, "hi\n");
        save(&path, &loaded.text, loaded.eol, loaded.encoding, loaded.bom).unwrap();
        let raw = fs::read(&path).unwrap();
        assert_eq!(raw, b"\xEF\xBB\xBFhi\n");
    }

    #[test]
    fn reads_utf16le() {
        let dir = tmp();
        let path = dir.path().join("u16.txt");
        let mut f = File::create(&path).unwrap();
        f.write_all(&[0xFF, 0xFE]).unwrap();
        for u in "hé\n".encode_utf16() {
            f.write_all(&u.to_le_bytes()).unwrap();
        }
        drop(f);
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.encoding, Encoding::Utf16Le);
        assert_eq!(loaded.text, "hé\n");
        save(&path, &loaded.text, loaded.eol, loaded.encoding, loaded.bom).unwrap();
        let loaded2 = load(&path).unwrap();
        assert_eq!(loaded2.text, "hé\n");
    }

    #[test]
    fn refuses_binary() {
        let dir = tmp();
        let path = dir.path().join("bin");
        fs::write(&path, b"abc\0def").unwrap();
        assert!(matches!(load(&path), Err(Error::Binary { .. })));
    }

    #[test]
    fn refuses_directory() {
        let dir = tmp();
        assert!(matches!(load(dir.path()), Err(Error::IsDirectory(_))));
    }

    #[test]
    fn save_leaves_no_temp_files() {
        let dir = tmp();
        let path = dir.path().join("f.txt");
        fs::write(&path, b"x").unwrap();
        save(&path, "hello", Eol::Lf, Encoding::Utf8, false).unwrap();
        let entries: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .filter(|n| n.contains("vibe-tmp"))
            .collect();
        assert!(entries.is_empty(), "temp files left behind: {entries:?}");
    }

    #[test]
    fn save_preserves_permissions() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = tmp();
            let path = dir.path().join("secret.txt");
            fs::write(&path, b"x").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            save(&path, "y", Eol::Lf, Encoding::Utf8, false).unwrap();
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn save_through_symlink_keeps_link() {
        #[cfg(unix)]
        {
            let dir = tmp();
            let target = dir.path().join("target.txt");
            let link = dir.path().join("link.txt");
            fs::write(&target, b"old").unwrap();
            std::os::unix::fs::symlink(&target, &link).unwrap();
            save(&link, "new", Eol::Lf, Encoding::Utf8, false).unwrap();
            assert!(fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink());
            assert_eq!(fs::read_to_string(&target).unwrap(), "new");
        }
    }

    #[test]
    fn save_into_missing_directory_fails_cleanly() {
        let dir = tmp();
        let path = dir.path().join("nope").join("f.txt");
        let err = save(&path, "x", Eol::Lf, Encoding::Utf8, false).unwrap_err();
        assert!(err.to_string().contains("cannot save"));
    }

    #[test]
    fn detects_external_change() {
        let dir = tmp();
        let path = dir.path().join("f.txt");
        fs::write(&path, b"one").unwrap();
        let loaded = load(&path).unwrap();
        assert!(!changed_on_disk(&path, loaded.mtime, Some(loaded.len)));
        // Truncate to a different length: reliably detectable even on
        // filesystems with one-second timestamp granularity.
        fs::write(&path, b"one two three").unwrap();
        assert!(changed_on_disk(&path, loaded.mtime, Some(loaded.len)));
    }
}
