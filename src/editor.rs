// The built-in text viewer and editor: the file side of it, kept apart from the window so it
// can be tested. Reading refuses binary files and anything too large to edit comfortably;
// saving writes beside the file and renames over it, keeps its permissions, and won't
// overwrite changes another program made since the file was opened.

use std::fs;
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::i18n::tr;

/// Larger files open in the system's default app instead.
pub const MAX_BYTES: u64 = 5 * 1024 * 1024;

pub enum Opened {
    Text(String),
    /// Not valid UTF-8: shown, but saving it back could change bytes, so it can't be edited.
    ReadOnly(String),
    NotText,
    TooLarge,
}

pub fn read(path: &Path) -> io::Result<(Opened, Option<SystemTime>)> {
    let meta = fs::metadata(path)?;
    let mtime = meta.modified().ok();
    if meta.len() > MAX_BYTES {
        return Ok((Opened::TooLarge, mtime));
    }
    let bytes = fs::read(path)?;
    // A NUL byte early on is the usual sign of a binary file, the same test `file` and git use.
    if bytes[..bytes.len().min(8000)].contains(&0) {
        return Ok((Opened::NotText, mtime));
    }
    let opened = match String::from_utf8(bytes) {
        Ok(text) => Opened::Text(text),
        Err(e) => Opened::ReadOnly(String::from_utf8_lossy(e.as_bytes()).into_owned()),
    };
    Ok((opened, mtime))
}

#[derive(Debug, PartialEq)]
pub enum SaveError {
    /// Another program changed the file after it was opened here.
    ChangedOnDisk,
    Failed(String),
}

/// Saves `text` to `path`. Unless `force`, refuses when the file's mtime is no longer `opened_at`.
/// Returns the new mtime, to compare against on the next save.
pub fn save(
    path: &Path,
    text: &str,
    opened_at: Option<SystemTime>,
    force: bool,
) -> Result<Option<SystemTime>, SaveError> {
    let failed = |e: io::Error| SaveError::Failed(format!("{}: {e}", path.display()));
    // Write through a symlink to the file it points at, so the link itself survives.
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let meta = fs::metadata(&target).ok();
    if !force && meta.as_ref().and_then(|m| m.modified().ok()) != opened_at {
        return Err(SaveError::ChangedOnDisk);
    }
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = target.with_file_name(format!(".{name}.ferry-save"));
    let written = fs::write(&temp, text)
        .and_then(|_| match &meta {
            Some(meta) => fs::set_permissions(&temp, meta.permissions()),
            None => Ok(()),
        })
        .and_then(|_| fs::rename(&temp, &target));
    if let Err(e) = written {
        let _ = fs::remove_file(&temp);
        return Err(failed(e));
    }
    Ok(fs::metadata(&target).and_then(|m| m.modified()).ok())
}

/// MIME types that are text even though they aren't `text/*`.
const TEXT_MIMES: &[&str] = &[
    "application/json",
    "application/xml",
    "application/toml",
    "application/yaml",
    "application/x-yaml",
    "application/javascript",
    "application/x-shellscript",
    "application/x-desktop",
    "application/x-php",
    "application/sql",
    "application/x-perl",
    "application/x-ruby",
    "application/x-subrip",
    "application/x-zerosize",
];

/// Whether a file of this MIME type (if it has a known one) should open in the editor.
/// Unknown types are tried too: the editor reads them and hands binary ones on.
pub fn wants(mime: Option<&str>) -> bool {
    match mime {
        None => true,
        // SVG is text too, but opening one should show the picture.
        Some(m) if m.starts_with("image/") => false,
        Some(m) => {
            m.starts_with("text/")
                || m.ends_with("+xml")
                || m.ends_with("+json")
                || TEXT_MIMES.contains(&m)
        }
    }
}

pub struct Editor {
    pub path: PathBuf,
    pub text: String,
    saved: String,
    pub mtime: Option<SystemTime>,
    pub read_only: bool,
    pub loading: bool,
    pub saving: bool,
    /// Markdown shown rendered rather than as source.
    pub preview: bool,
    /// The find bar's text, while it is open.
    pub find: Option<String>,
    pub focus_find: bool,
    /// Byte ranges of the find bar's matches, and which one is current.
    pub found: Vec<Range<usize>>,
    pub current: Option<usize>,
    /// A byte range to select and scroll to on the next frame.
    pub select: Option<Range<usize>>,
    /// A line (from 1) to go to once the file has loaded.
    pub line: Option<u64>,
}

impl Editor {
    pub fn loading(path: PathBuf) -> Self {
        Editor {
            path,
            text: String::new(),
            saved: String::new(),
            mtime: None,
            read_only: true,
            loading: true,
            saving: false,
            preview: false,
            find: None,
            focus_find: false,
            found: Vec::new(),
            current: None,
            select: None,
            line: None,
        }
    }

    pub fn loaded(&mut self, text: String, mtime: Option<SystemTime>, read_only: bool) {
        self.saved = text.clone();
        self.text = text;
        self.mtime = mtime;
        self.read_only = read_only;
        self.loading = false;
        // Markdown opens rendered, unless it was opened to go to a line.
        self.preview = crate::markdown::is_markdown(&self.path) && self.line.is_none();
        self.refind();
        if let Some(line) = self.line.take() {
            self.go_to_line(line);
        }
    }

    /// Recomputes the find bar's matches after the text or the search changed.
    pub fn refind(&mut self) {
        self.found = crate::highlight::find_all(&self.text, self.find.as_deref().unwrap_or(""));
        self.current = self.current.filter(|&c| c < self.found.len());
    }

    /// Moves to the next match (or the previous one), wrapping around.
    pub fn step(&mut self, forward: bool) {
        let n = self.found.len();
        if n == 0 {
            return;
        }
        let next = match self.current {
            Some(c) if forward => (c + 1) % n,
            Some(c) => (c + n - 1) % n,
            None if forward => 0,
            None => n - 1,
        };
        self.current = Some(next);
        self.select = Some(self.found[next].clone());
    }

    /// Puts the cursor on `line` (from 1), on its first match when the find bar has one.
    pub fn go_to_line(&mut self, line: u64) {
        let start = if line <= 1 {
            0
        } else {
            self.text
                .match_indices('\n')
                .nth(line as usize - 2)
                .map_or(self.text.len(), |(i, _)| i + 1)
        };
        self.current = self.found.iter().position(|r| r.start >= start);
        self.select = Some(match self.current {
            Some(c) => self.found[c].clone(),
            None => start..start,
        });
    }

    pub fn dirty(&self) -> bool {
        !self.read_only && self.text != self.saved
    }

    /// Records a finished save of `text` (what was in the editor when the save began).
    pub fn saved(&mut self, text: String, mtime: Option<SystemTime>) {
        self.saved = text;
        self.mtime = mtime;
        self.saving = false;
    }

    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Text for the line-number gutter.
    pub fn line_numbers(&self) -> String {
        let lines = self.text.split('\n').count().max(1);
        (1..=lines)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Why a file didn't open in the editor, for the status bar.
pub fn refusal(opened: &Opened) -> Option<String> {
    match opened {
        Opened::TooLarge => Some(tr(
            "Too large to edit here; opening it with the default app",
        )),
        Opened::NotText => Some(tr("Not a text file; opening it with the default app")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ferry-editor-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reads_text_and_refuses_binary_and_huge_files() {
        let dir = temp();
        fs::write(dir.join("a.md"), "# Hi\n").unwrap();
        fs::write(dir.join("b.bin"), [0x89, b'P', 0, 1]).unwrap();
        fs::write(dir.join("c.txt"), [b'o', b'k', 0xff, b'\n']).unwrap();
        let f = fs::File::create(dir.join("big.log")).unwrap();
        f.set_len(MAX_BYTES + 1).unwrap();

        assert!(matches!(read(&dir.join("a.md")).unwrap().0, Opened::Text(t) if t == "# Hi\n"));
        assert!(matches!(
            read(&dir.join("b.bin")).unwrap().0,
            Opened::NotText
        ));
        assert!(
            matches!(read(&dir.join("c.txt")).unwrap().0, Opened::ReadOnly(t) if t == "ok\u{fffd}\n")
        );
        assert!(matches!(
            read(&dir.join("big.log")).unwrap().0,
            Opened::TooLarge
        ));
    }

    #[test]
    fn saves_atomically_keeping_permissions_and_symlinks() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = temp();
        let file = dir.join("run.sh");
        fs::write(&file, "echo a\n").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o750)).unwrap();
        symlink(&file, dir.join("link.sh")).unwrap();
        let (_, mtime) = read(&dir.join("link.sh")).unwrap();

        save(&dir.join("link.sh"), "echo b\n", mtime, false).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "echo b\n");
        assert!(
            fs::symlink_metadata(dir.join("link.sh"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o750
        );
        assert!(fs::read_dir(&dir).unwrap().all(|e| {
            !e.unwrap()
                .file_name()
                .to_string_lossy()
                .contains("ferry-save")
        }));
    }

    #[test]
    fn will_not_overwrite_changes_made_elsewhere_unless_told() {
        let dir = temp();
        let file = dir.join("notes.txt");
        fs::write(&file, "one").unwrap();
        let (_, opened_at) = read(&file).unwrap();
        // Another program saves a moment later.
        let later = SystemTime::now() + std::time::Duration::from_secs(5);
        fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(later)
            .unwrap();

        assert_eq!(
            save(&file, "mine", opened_at, false),
            Err(SaveError::ChangedOnDisk)
        );
        assert_eq!(fs::read_to_string(&file).unwrap(), "one");
        assert!(save(&file, "mine", opened_at, true).is_ok());
        assert_eq!(fs::read_to_string(&file).unwrap(), "mine");
    }

    #[test]
    fn decides_what_opens_here() {
        assert!(wants(Some("text/markdown")) && wants(Some("application/json")) && wants(None));
        assert!(wants(Some("application/atom+xml")));
        assert!(
            !wants(Some("image/svg+xml"))
                && !wants(Some("image/png"))
                && !wants(Some("application/pdf"))
                && !wants(Some("video/mp4"))
        );
    }

    #[test]
    fn tracks_unsaved_changes() {
        let mut e = Editor::loading(PathBuf::from("/x"));
        e.loaded("a".into(), None, false);
        assert!(!e.dirty());
        e.text.push('b');
        assert!(e.dirty());
        e.saved("ab".into(), None);
        assert!(!e.dirty());
        assert_eq!(e.line_numbers(), "1");
        e.text = "1\n2\n".into();
        assert_eq!(e.line_numbers(), "1\n2\n3");
    }

    #[test]
    fn finds_steps_and_goes_to_lines() {
        let mut e = Editor::loading(PathBuf::from("/notes.txt"));
        e.find = Some("milk".into());
        e.line = Some(3);
        e.loaded("milk\nbread\nmore milk\n".into(), None, false);
        assert_eq!(e.found, [0..4, 16..20]);
        // Opened at line 3, on that line's match.
        assert_eq!((e.current, e.select.clone()), (Some(1), Some(16..20)));
        e.step(true);
        assert_eq!(e.select, Some(0..4), "wraps to the first match");
        e.step(false);
        assert_eq!(e.current, Some(1));
        e.text.insert_str(0, "milk ");
        e.refind();
        assert_eq!(e.found.len(), 3);
        assert!(!e.preview);
    }
}
