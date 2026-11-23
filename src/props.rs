// What the Properties dialog shows and changes: a file's facts, a folder's contents counted in
// the background, Unix permissions, and checksums. Reading happens off the UI thread, since the
// file may sit on a network mount that stops answering.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use crate::i18n::tr;

#[derive(Clone, Debug)]
pub struct Info {
    pub path: PathBuf,
    pub is_dir: bool,
    /// Where a symbolic link points, as written in the link.
    pub link: Option<PathBuf>,
    pub size: u64,
    /// Space the file takes on disk (sparse files take less, small ones a whole block).
    pub on_disk: u64,
    pub mode: u32,
    pub owner: String,
    pub group: String,
    /// Whether this user owns it, which is what lets them change its permissions.
    pub mine: bool,
    pub modified: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    pub created: Option<SystemTime>,
    pub free: Option<u64>,
}

/// Reads a file's facts. A link is described by what it points at, and says where that is.
pub fn info(path: &Path) -> Result<Info, String> {
    let lmeta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    let link = lmeta
        .file_type()
        .is_symlink()
        .then(|| fs::read_link(path).ok())
        .flatten();
    let meta = fs::metadata(path).unwrap_or(lmeta);
    let uid = unsafe { libc::geteuid() };
    Ok(Info {
        path: path.to_path_buf(),
        is_dir: meta.is_dir(),
        link,
        size: meta.len(),
        on_disk: meta.blocks() * 512,
        mode: meta.mode() & 0o7777,
        owner: name_of("/etc/passwd", meta.uid()),
        group: name_of("/etc/group", meta.gid()),
        mine: uid == 0 || uid == meta.uid(),
        modified: meta.modified().ok(),
        accessed: meta.accessed().ok(),
        created: meta.created().ok(),
        free: crate::files::free_space(if meta.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        }),
    })
}

/// A user or group name from passwd/group-style lines; the number when it isn't listed there
/// (users from LDAP or another machine's files on a shared drive).
fn name_of(file: &str, id: u32) -> String {
    fs::read_to_string(file)
        .ok()
        .and_then(|text| {
            text.lines().find_map(|l| {
                let mut f = l.split(':');
                let name = f.next()?;
                (f.nth(1)?.parse::<u32>().ok()? == id).then(|| name.to_string())
            })
        })
        .unwrap_or_else(|| id.to_string())
}

/// What's inside folders (and the files themselves): (files, folders, bytes). Doesn't follow
/// links or leave the filesystem it starts on, and stops as soon as `stop` is set.
pub fn usage(paths: &[PathBuf], stop: &AtomicBool) -> (u64, u64, u64) {
    let (mut files, mut dirs, mut bytes) = (0, 0, 0);
    let mut pending: Vec<(PathBuf, u64)> = Vec::new();
    for p in paths {
        if let Ok(m) = fs::symlink_metadata(p) {
            if m.is_dir() {
                pending.push((p.clone(), m.dev()));
            } else {
                files += 1;
                bytes += m.len();
            }
        }
    }
    while let Some((dir, dev)) = pending.pop() {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let Ok(items) = fs::read_dir(&dir) else {
            continue;
        };
        for item in items.flatten() {
            let Ok(m) = item.metadata() else { continue };
            if m.is_dir() {
                dirs += 1;
                if m.dev() == dev {
                    pending.push((item.path(), dev));
                }
            } else {
                files += 1;
                bytes += m.len();
            }
        }
    }
    (files, dirs, bytes)
}

/// Sets the permission bits, keeping the file type.
pub fn set_mode(path: &Path, mode: u32) -> Result<(), String> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o7777)).map_err(|e| {
        match e.kind() {
            std::io::ErrorKind::PermissionDenied => {
                tr("Only the owner can change the permissions of %1").replace(
                    "%1",
                    &path.file_name().unwrap_or_default().to_string_lossy(),
                )
            }
            _ => e.to_string(),
        }
    })
}

/// Allows (or stops) running a file as a program: execute for everyone who can read it, the
/// way `chmod +x` does with the usual umask.
pub fn executable(mode: u32, on: bool) -> u32 {
    if on {
        mode | ((mode & 0o444) >> 2)
    } else {
        mode & !0o111
    }
}

/// "rwxr-xr-x", as ls shows it.
pub fn mode_string(mode: u32) -> String {
    let bit = |m: u32, c: char| if mode & m != 0 { c } else { '-' };
    [
        bit(0o400, 'r'),
        bit(0o200, 'w'),
        bit(0o100, 'x'),
        bit(0o040, 'r'),
        bit(0o020, 'w'),
        bit(0o010, 'x'),
        bit(0o004, 'r'),
        bit(0o002, 'w'),
        bit(0o001, 'x'),
    ]
    .iter()
    .collect()
}

/// A checksum from coreutils (`sha256sum`, `md5sum`), lowercase hex.
pub fn checksum(path: &Path, tool: &str) -> Result<String, String> {
    let out = Command::new(tool)
        .arg("--")
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("{tool}: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .map(|s| s.trim_start_matches('\\').to_lowercase())
        .ok_or_else(|| format!("{tool}: no output"))
}

/// A cancel flag for background work, shared with the dialog that wants the answer.
pub fn stop_flag() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ferry-props-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("box/inner")).unwrap();
        fs::write(dir.join("box/a.txt"), "12345").unwrap();
        fs::write(dir.join("box/inner/b.txt"), "123").unwrap();
        fs::write(dir.join("run.sh"), "echo hi\n").unwrap();
        fs::set_permissions(dir.join("run.sh"), fs::Permissions::from_mode(0o640)).unwrap();
        std::os::unix::fs::symlink("run.sh", dir.join("link")).unwrap();
        dir
    }

    #[test]
    fn reads_facts_and_links() {
        let dir = temp();
        let i = info(&dir.join("link")).unwrap();
        assert_eq!(i.link, Some(PathBuf::from("run.sh")));
        assert_eq!((i.size, i.mode, i.is_dir), (8, 0o640, false));
        assert!(i.mine && !i.owner.is_empty() && i.modified.is_some());
        assert!(info(&dir.join("missing")).is_err());
    }

    #[test]
    fn counts_folder_contents() {
        let dir = temp();
        let (files, dirs, bytes) = usage(
            &[dir.join("box"), dir.join("run.sh")],
            &AtomicBool::new(false),
        );
        assert_eq!((files, dirs, bytes), (3, 1, 16));
        assert_eq!(
            usage(&[dir.join("box")], &AtomicBool::new(true)).0,
            0,
            "stops when asked"
        );
    }

    #[test]
    fn permissions() {
        assert_eq!(executable(0o640, true), 0o750);
        assert_eq!(executable(0o644, true), 0o755);
        assert_eq!(executable(0o755, false), 0o644);
        assert_eq!(mode_string(0o754), "rwxr-xr--");
        let dir = temp();
        set_mode(&dir.join("run.sh"), executable(0o640, true)).unwrap();
        assert_eq!(info(&dir.join("run.sh")).unwrap().mode, 0o750);
        assert!(set_mode(&dir.join("missing"), 0o644).is_err());
    }

    #[test]
    fn checksums() {
        let dir = temp();
        assert_eq!(
            checksum(&dir.join("box/a.txt"), "sha256sum").unwrap(),
            "5994471abb01112afcc18159f6cc74b4f511b99806da59b3caf5a9c173cacfc5"
        );
        assert_eq!(
            checksum(&dir.join("box/a.txt"), "md5sum").unwrap(),
            "827ccb0eea8a706c4c34a16891f84e7b"
        );
        assert!(checksum(&dir.join("missing"), "md5sum").is_err());
    }
}
