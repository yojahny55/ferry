// Everything ferry does to the disk, kept apart from the window so it can be tested.

use std::cmp::Ordering;
use std::ffi::CString;
use std::fs;
use std::io;
use std::iter::Peekable;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::Chars;
use std::time::SystemTime;

use crate::i18n::tr;

pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_link: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

impl Entry {
    pub fn hidden(&self) -> bool {
        self.name.starts_with('.')
    }
}

pub fn list(dir: &Path) -> io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    for item in fs::read_dir(dir)? {
        // A file removed while the folder is being read is not an error worth showing.
        let Ok(item) = item else { continue };
        let path = item.path();
        let is_link = item.file_type().is_ok_and(|t| t.is_symlink());
        // Links are followed so a link to a folder opens like one; a dangling link shows as itself.
        let meta = fs::metadata(&path).or_else(|_| fs::symlink_metadata(&path));
        let (is_dir, size, modified) = match meta {
            Ok(m) => (m.is_dir(), m.len(), m.modified().ok()),
            Err(_) => (false, 0, None),
        };
        out.push(Entry {
            name: item.file_name().to_string_lossy().into_owned(),
            path,
            is_dir,
            is_link,
            size,
            modified,
        });
    }
    Ok(out)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortKey {
    Name,
    Size,
    Modified,
}

/// Folders first, like Dolphin, then by the chosen column.
pub fn sort(entries: &mut [Entry], key: SortKey, ascending: bool) {
    entries.sort_by(|a, b| {
        let by_name = natural(&a.name, &b.name);
        let order = match key {
            SortKey::Name => by_name,
            SortKey::Size => a.size.cmp(&b.size).then(by_name),
            SortKey::Modified => a.modified.cmp(&b.modified).then(by_name),
        };
        b.is_dir
            .cmp(&a.is_dir)
            .then(if ascending { order } else { order.reverse() })
    });
}

/// Case-insensitive order that reads runs of digits as numbers, so "file2" comes before "file10".
pub fn natural(a: &str, b: &str) -> Ordering {
    fn digits(chars: &mut Peekable<Chars>) -> String {
        let mut run = String::new();
        while let Some(&c) = chars.peek().filter(|c| c.is_ascii_digit()) {
            run.push(c);
            chars.next();
        }
        run
    }
    let (lower_a, lower_b) = (a.to_lowercase(), b.to_lowercase());
    let (mut x, mut y) = (lower_a.chars().peekable(), lower_b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let (n, m) = (digits(&mut x), digits(&mut y));
                let (n0, m0) = (n.trim_start_matches('0'), m.trim_start_matches('0'));
                let order = n0
                    .len()
                    .cmp(&m0.len())
                    .then_with(|| n0.cmp(m0))
                    .then(n.len().cmp(&m.len()));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(c), Some(d)) => {
                if c != d {
                    return c.cmp(&d);
                }
                x.next();
                y.next();
            }
        }
    }
}

/// `dir/name`, or `dir/name (1).ext` and up when that is taken, so nothing is ever overwritten.
pub fn unique_name(dir: &Path, name: &str) -> PathBuf {
    let taken = |p: &Path| p.symlink_metadata().is_ok();
    let first = dir.join(name);
    if !taken(&first) {
        return first;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (1..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !taken(p))
        .unwrap()
}

/// Copies or moves `sources` into `dir` with coreutils, renaming on a clash instead of overwriting.
pub fn paste(sources: &[PathBuf], dir: &Path, cut: bool) -> Result<usize, String> {
    let mut done = 0;
    for src in sources {
        let Some(name) = src.file_name() else {
            continue;
        };
        if dir.starts_with(src) {
            return Err(tr("Can't put %1 inside itself").replace("%1", &name.to_string_lossy()));
        }
        if cut && src.parent() == Some(dir) {
            continue;
        }
        let dest = unique_name(dir, &name.to_string_lossy());
        let (program, flags) = if cut { ("mv", "-T") } else { ("cp", "-aT") };
        let out = Command::new(program)
            .args([flags, "--"])
            .arg(src)
            .arg(&dest)
            .output()
            .map_err(|e| format!("{program}: {e}"))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
        }
        done += 1;
    }
    Ok(done)
}

/// Moves files to the freedesktop trash, where the desktop's other tools can restore them.
pub fn trash(paths: &[PathBuf]) -> Result<(), String> {
    let out = Command::new("gio")
        .args(["trash", "--"])
        .args(paths)
        .output()
        .map_err(|e| format!("gio: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
    }
}

pub fn delete(paths: &[PathBuf]) -> Result<(), String> {
    for p in paths {
        let is_dir = p.symlink_metadata().is_ok_and(|m| m.is_dir());
        let result = if is_dir {
            fs::remove_dir_all(p)
        } else {
            fs::remove_file(p)
        };
        result.map_err(|e| format!("{}: {e}", p.display()))?;
    }
    Ok(())
}

/// Checks a new name for a file in `dir`, returning where it would go.
pub fn check_name(dir: &Path, name: &str, current: Option<&Path>) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
        return Err(tr("That name can't be used"));
    }
    let target = dir.join(name);
    if Some(target.as_path()) != current && target.symlink_metadata().is_ok() {
        return Err(tr("%1 already exists").replace("%1", name));
    }
    Ok(target)
}

pub fn free_space(path: &Path) -> Option<u64> {
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statvfs(c.as_ptr(), &mut s) } == 0)
        .then(|| s.f_bavail as u64 * s.f_frsize as u64)
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit]).replace('.', &crate::i18n::decimal().to_string())
    }
}

// shortcut: ISO-style dates in every language; switch to the language's date format if users ask.
pub fn format_time(time: SystemTime) -> String {
    let secs = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as libc::time_t)
        .unwrap_or(0);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min
    )
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// The sidebar's places: home, the XDG user folders that exist, the trash, and the root.
pub fn places() -> Vec<(String, PathBuf, char)> {
    let home = home();
    let user_dirs = fs::read_to_string(home.join(".config/user-dirs.dirs")).unwrap_or_default();
    let xdg = |key: &str, fallback: &str| {
        user_dirs
            .lines()
            .find_map(|l| l.strip_prefix(&format!("XDG_{key}_DIR=")))
            .map(|v| {
                PathBuf::from(
                    v.trim_matches('"')
                        .replace("$HOME", &home.to_string_lossy()),
                )
            })
            .unwrap_or_else(|| home.join(fallback))
    };
    let mut places = vec![(tr("Home"), home.clone(), '\u{f015}')];
    for (key, label, icon) in [
        ("DESKTOP", "Desktop", '\u{f108}'),
        ("DOCUMENTS", "Documents", '\u{f15c}'),
        ("DOWNLOAD", "Downloads", '\u{f019}'),
        ("MUSIC", "Music", '\u{f001}'),
        ("PICTURES", "Pictures", '\u{f03e}'),
        ("VIDEOS", "Videos", '\u{f03d}'),
    ] {
        let path = xdg(key, label);
        if path != home && path.is_dir() {
            places.push((tr(label), path, icon));
        }
    }
    let trash = home.join(".local/share/Trash/files");
    if trash.is_dir() {
        places.push((tr("Trash"), trash, '\u{f1f8}'));
    }
    places.push((tr("Root"), PathBuf::from("/"), '\u{f0a0}'));
    places
}

/// Folder shortcuts live in GTK's bookmarks file, so they are shared with the desktop's
/// file pickers and other file managers.
pub fn bookmarks_file() -> PathBuf {
    home().join(".config/gtk-3.0/bookmarks")
}

/// Each line is `file:///percent/encoded/path [optional label]`; other URI schemes are left alone.
fn parse_bookmark(line: &str) -> Option<(PathBuf, Option<String>)> {
    let (uri, label) = match line.trim().split_once(' ') {
        Some((uri, label)) => (uri, Some(label.trim().to_owned()).filter(|l| !l.is_empty())),
        None => (line.trim(), None),
    };
    let encoded = uri.strip_prefix("file://")?;
    let mut bytes = Vec::with_capacity(encoded.len());
    let mut rest = encoded.as_bytes();
    while let Some((&b, tail)) = rest.split_first() {
        let hex = tail
            .get(..2)
            .and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok());
        match (b, hex) {
            (b'%', Some(v)) => {
                bytes.push(v);
                rest = &tail[2..];
            }
            _ => {
                bytes.push(b);
                rest = tail;
            }
        }
    }
    Some((PathBuf::from(std::ffi::OsStr::from_bytes(&bytes)), label))
}

/// A file:// URI with everything but unreserved characters percent-encoded.
pub fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}

/// (label, path) for every local folder bookmark. They are not checked on disk: a bookmark
/// can point at a network mount, and asking an unreachable one blocks until it times out.
pub fn bookmarks(file: &Path) -> Vec<(String, PathBuf)> {
    fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .filter_map(parse_bookmark)
        .map(|(path, label)| {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "/".into());
            (label.unwrap_or(name), path)
        })
        .collect()
}

/// Adds folders that aren't bookmarked yet and removes `remove`, keeping every other line as it was.
/// Returns how many were added.
pub fn update_bookmarks(file: &Path, add: &[PathBuf], remove: Option<&Path>) -> io::Result<usize> {
    let text = fs::read_to_string(file).or_else(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            Ok(String::new())
        } else {
            Err(e)
        }
    })?;
    let mut lines: Vec<String> = text
        .lines()
        .filter(|l| remove.is_none_or(|r| parse_bookmark(l).is_none_or(|(p, _)| p != r)))
        .map(String::from)
        .collect();
    let mut added = 0;
    for path in add {
        if !lines
            .iter()
            .any(|l| parse_bookmark(l).is_some_and(|(p, _)| &p == path))
        {
            lines.push(file_uri(path));
            added += 1;
        }
    }
    // Write beside the file and rename over it, so a crash never leaves it half written.
    // A symlinked file (dotfiles) is followed, so the link itself survives.
    let target = fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    if let Some(dir) = target.parent() {
        fs::create_dir_all(dir)?;
    }
    let temp = target.with_extension("ferry-tmp");
    fs::write(
        &temp,
        lines.iter().map(|l| format!("{l}\n")).collect::<String>(),
    )?;
    fs::rename(&temp, &target)?;
    Ok(added)
}

/// Removable and mounted drives, from the kernel's mount table.
pub fn devices() -> Vec<(String, PathBuf)> {
    let mounts = fs::read_to_string("/proc/mounts").unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    mounts
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1))
        .map(|m| m.replace("\\040", " "))
        .filter(|m| {
            ["/run/media/", "/media/", "/mnt/"]
                .iter()
                .any(|p| m.starts_with(p))
        })
        // A path mounted over twice is still one place.
        .filter(|m| seen.insert(m.clone()))
        .map(|m| {
            let path = PathBuf::from(m);
            (
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                path,
            )
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Folder,
    Image,
    Audio,
    Video,
    Archive,
    Code,
    Document,
    Other,
}

pub fn kind(entry: &Entry) -> Kind {
    if entry.is_dir {
        return Kind::Folder;
    }
    let ext = entry
        .name
        .rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "avif" | "heic" | "tiff"
        | "ico" => Kind::Image,
        "mp3" | "flac" | "ogg" | "opus" | "wav" | "m4a" | "aac" => Kind::Audio,
        "mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v" => Kind::Video,
        "zip" | "tar" | "gz" | "xz" | "zst" | "bz2" | "7z" | "rar" | "tgz" | "deb" | "rpm" => {
            Kind::Archive
        }
        "rs" | "c" | "h" | "cpp" | "hpp" | "py" | "js" | "ts" | "tsx" | "jsx" | "go" | "rb"
        | "php" | "java" | "sh" | "lua" | "json" | "toml" | "yaml" | "yml" | "html" | "css"
        | "qml" | "sql" | "conf" => Kind::Code,
        "pdf" | "txt" | "md" | "doc" | "docx" | "odt" | "rtf" | "csv" | "xls" | "xlsx" | "ods"
        | "ppt" | "pptx" | "epub" => Kind::Document,
        _ => Kind::Other,
    }
}

/// Nerd Font glyphs, which Omarchy's default fonts carry.
pub fn glyph(entry: &Entry) -> char {
    let ext = entry
        .name
        .rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default();
    match kind(entry) {
        Kind::Folder if entry.is_link => '\u{f0c1}',
        Kind::Folder => '\u{f07b}',
        Kind::Image => '\u{f1c5}',
        Kind::Audio => '\u{f1c7}',
        Kind::Video => '\u{f1c8}',
        Kind::Archive => '\u{f1c6}',
        Kind::Code => '\u{f1c9}',
        Kind::Document if ext == "pdf" => '\u{f1c1}',
        Kind::Document => '\u{f15c}',
        Kind::Other => '\u{f15b}',
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ferry-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn natural_order_reads_numbers() {
        let mut names = vec!["file10", "File2", "file1", "a", "file02"];
        names.sort_by(|a, b| natural(a, b));
        assert_eq!(names, ["a", "file1", "File2", "file02", "file10"]);
    }

    #[test]
    fn folders_sort_first() {
        let dir = temp();
        fs::write(dir.join("a.txt"), "x").unwrap();
        fs::create_dir(dir.join("zeta")).unwrap();
        let mut entries = list(&dir).unwrap();
        sort(&mut entries, SortKey::Name, true);
        assert_eq!(entries[0].name, "zeta");
        sort(&mut entries, SortKey::Name, false);
        assert_eq!(entries[0].name, "zeta");
    }

    #[test]
    fn unique_name_never_overwrites() {
        let dir = temp();
        assert_eq!(unique_name(&dir, "a.txt"), dir.join("a.txt"));
        fs::write(dir.join("a.txt"), "x").unwrap();
        assert_eq!(unique_name(&dir, "a.txt"), dir.join("a (1).txt"));
        assert_eq!(unique_name(&dir, ".bashrc"), dir.join(".bashrc"));
        fs::write(dir.join(".bashrc"), "x").unwrap();
        assert_eq!(unique_name(&dir, ".bashrc"), dir.join(".bashrc (1)"));
    }

    #[test]
    fn paste_copies_moves_and_refuses_recursion() {
        let dir = temp();
        let sub = dir.join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(dir.join("a.txt"), "x").unwrap();
        assert_eq!(paste(&[dir.join("a.txt")], &dir, false), Ok(1));
        assert!(dir.join("a (1).txt").exists());
        assert_eq!(paste(&[dir.join("a.txt")], &sub, true), Ok(1));
        assert!(sub.join("a.txt").exists() && !dir.join("a.txt").exists());
        assert!(paste(std::slice::from_ref(&dir), &sub, false).is_err());
    }

    #[test]
    fn names_are_checked() {
        let dir = temp();
        fs::write(dir.join("a"), "x").unwrap();
        assert!(check_name(&dir, "a/b", None).is_err());
        assert!(check_name(&dir, "a", None).is_err());
        assert_eq!(
            check_name(&dir, "a", Some(&dir.join("a"))),
            Ok(dir.join("a"))
        );
        assert_eq!(check_name(&dir, " b ", None), Ok(dir.join("b")));
    }

    #[test]
    fn bookmarks_round_trip_and_keep_other_lines() {
        let dir = temp();
        let spaced = dir.join("Tax 2026 ñ");
        fs::create_dir(&spaced).unwrap();
        let file = dir.join("gtk/bookmarks");
        fs::create_dir_all(dir.join("gtk")).unwrap();
        fs::write(
            &file,
            "sftp://server/home Server\nfile:///nowhere/at/all Gone\n",
        )
        .unwrap();

        assert_eq!(
            update_bookmarks(&file, std::slice::from_ref(&spaced), None).unwrap(),
            1
        );
        assert_eq!(
            update_bookmarks(&file, std::slice::from_ref(&spaced), None).unwrap(),
            0
        );
        let text = fs::read_to_string(&file).unwrap();
        assert!(text.contains("Tax%202026%20%C3%B1"), "{text}");
        let gone = ("Gone".to_owned(), PathBuf::from("/nowhere/at/all"));
        assert_eq!(
            bookmarks(&file),
            vec![gone.clone(), ("Tax 2026 ñ".to_owned(), spaced.clone())]
        );

        update_bookmarks(&file, &[], Some(&spaced)).unwrap();
        assert_eq!(bookmarks(&file), vec![gone]);
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "sftp://server/home Server\nfile:///nowhere/at/all Gone\n"
        );
    }

    #[test]
    fn bookmark_labels_are_read() {
        assert_eq!(
            parse_bookmark("file:///home/a%20b My stuff"),
            Some((PathBuf::from("/home/a b"), Some("My stuff".into())))
        );
        assert_eq!(parse_bookmark("sftp://x/y"), None);
    }

    #[test]
    fn sizes_are_human() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KiB");
    }
}
