// Compressing and extracting, through bsdtar (libarchive), which every Arch system has because
// pacman needs it. It reads zip, 7z, rar, tar in every compression, iso, cpio, deb and more, and
// writes zip, 7z and tar.*; it also refuses entries that would land outside the target folder.
// Single compressed files (notes.txt.gz) go through their own tool, which bsdtar doesn't do.
//
// Both directions work beside the result and rename it into place at the end, so a failed or
// cancelled run never leaves half an archive or half a folder behind.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::i18n::tr;

/// Archive suffixes, longest first so "x.tar.gz" is read as a .tar.gz, not a .gz.
const ARCHIVES: &[&str] = &[
    "tar.gz", "tar.bz2", "tar.xz", "tar.zst", "tar.lz4", "tar.lzma", "tar.lz", "tar.z", "tgz",
    "tbz", "tbz2", "txz", "tzst", "tlz", "zip", "7z", "rar", "tar", "iso", "cpio", "deb", "rpm",
    "jar", "apk", "xpi", "cab", "xar", "lha", "lzh", "cbz", "cbr", "cb7",
];

/// Single compressed files and the tool that unpacks one to stdout.
const COMPRESSED: &[(&str, &str)] = &[
    ("gz", "gzip"),
    ("bz2", "bzip2"),
    ("xz", "xz"),
    ("lzma", "xz"),
    ("zst", "zstd"),
    ("lz4", "lz4"),
];

/// What Compress offers: (suffix, what it's for).
pub fn formats() -> Vec<(&'static str, String)> {
    vec![
        ("zip", tr("zip: opens anywhere")),
        ("tar.zst", tr("tar.zst: fast and small, for Linux")),
        ("tar.gz", tr("tar.gz: the classic for Linux")),
        ("tar.xz", tr("tar.xz: smallest, slower")),
        ("7z", tr("7z: small, for 7-Zip users")),
    ]
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    /// Holds files: bsdtar extracts it.
    Archive,
    /// One compressed file, and the program that decompresses it.
    Compressed(&'static str),
}

/// Splits a file name into its name and its archive suffix, if it has one.
pub fn split(name: &str) -> Option<(&str, &str)> {
    let lower = name.to_lowercase();
    let suffix = ARCHIVES
        .iter()
        .copied()
        .chain(COMPRESSED.iter().map(|(ext, _)| *ext))
        .find(|ext| lower.len() > ext.len() + 1 && lower.ends_with(&format!(".{ext}")))?;
    let at = name.len() - suffix.len() - 1;
    Some((&name[..at], &name[at + 1..]))
}

pub fn kind(name: &str) -> Option<Kind> {
    let (_, ext) = split(name)?;
    let ext = ext.to_lowercase();
    Some(match COMPRESSED.iter().find(|(e, _)| *e == ext) {
        Some((_, tool)) => Kind::Compressed(tool),
        None => Kind::Archive,
    })
}

/// A free name in `dir` for `name`, numbering before the whole archive suffix:
/// "photos (1).tar.gz", not "photos.tar (1).gz".
pub fn unique(dir: &Path, name: &str) -> PathBuf {
    let taken = |p: &Path| p.symlink_metadata().is_ok();
    let first = dir.join(name);
    if !taken(&first) {
        return first;
    }
    let (stem, ext) = match split(name) {
        Some((stem, ext)) => (stem, format!(".{ext}")),
        None => (name, String::new()),
    };
    (1..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !taken(p))
        .unwrap()
}

/// A hidden working name beside the result, unique within this run of Ferry.
fn scratch(dir: &Path, ext: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    dir.join(format!(".ferry-{}-{n}{ext}", std::process::id()))
}

fn run(command: &mut Command) -> Result<(), String> {
    let out = command
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("{:?}: {e}", command.get_program()))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    if err.contains("assphrase") || err.contains("ncrypted") {
        return Err(tr(
            "This archive is password-protected, which Ferry can't open yet.",
        ));
    }
    let line = err
        .lines()
        .rfind(|l| !l.trim().is_empty() && !l.contains("Error exit delayed"));
    Err(line
        .unwrap_or("failed")
        .trim()
        .trim_start_matches("bsdtar: ")
        .to_string())
}

/// The names inside an archive, for a look before extracting it.
pub fn list(path: &Path) -> Result<Vec<String>, String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match kind(&name) {
        Some(Kind::Compressed(_)) => Ok(vec![
            split(&name).map_or(name.clone(), |(s, _)| s.to_string()),
        ]),
        _ => {
            let out = Command::new("bsdtar")
                .arg("-tf")
                .arg(path)
                .stdin(Stdio::null())
                .output()
                .map_err(|e| format!("bsdtar: {e}"))?;
            if !out.status.success() {
                return run(Command::new("bsdtar").arg("-tf").arg(path)).map(|_| Vec::new());
            }
            Ok(String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|l| l.trim_end_matches('/').to_string())
                .filter(|l| !l.is_empty() && l != ".")
                .collect())
        }
    }
}

/// Extracts `path` into `dir` and returns what it made. An archive holding a single folder
/// (or file) becomes just that; anything else goes into a new folder named after the archive,
/// so extracting never scatters files. `into_folder` always makes the folder.
pub fn extract(path: &Path, dir: &Path, into_folder: bool) -> Result<PathBuf, String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = split(&name).map_or(name.as_str(), |(s, _)| s).to_string();
    match kind(&name) {
        None => Err(tr("%1 isn't an archive Ferry knows").replace("%1", &name)),
        Some(Kind::Compressed(tool)) => {
            let temp = scratch(dir, "");
            let file = fs::File::create(&temp).map_err(|e| e.to_string())?;
            let result = run(Command::new(tool).arg("-dc").arg(path).stdout(file));
            let target = unique(dir, &stem);
            result
                .and_then(|_| fs::rename(&temp, &target).map_err(|e| e.to_string()))
                .map(|_| target)
                .inspect_err(|_| {
                    let _ = fs::remove_file(&temp);
                })
        }
        Some(Kind::Archive) => {
            let temp = scratch(dir, "");
            fs::create_dir(&temp).map_err(|e| e.to_string())?;
            let done = run(Command::new("bsdtar")
                .arg("-xf")
                .arg(path)
                .arg("-C")
                .arg(&temp))
            .and_then(|_| {
                let inside: Vec<PathBuf> = fs::read_dir(&temp)
                    .map_err(|e| e.to_string())?
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .collect();
                if let [only] = &inside[..]
                    && !into_folder
                {
                    let target =
                        unique(dir, &only.file_name().unwrap_or_default().to_string_lossy());
                    fs::rename(only, &target).map_err(|e| e.to_string())?;
                    fs::remove_dir(&temp).map_err(|e| e.to_string())?;
                    Ok(target)
                } else {
                    let target = unique(dir, &stem);
                    fs::rename(&temp, &target).map_err(|e| e.to_string())?;
                    Ok(target)
                }
            });
            if done.is_err() {
                let _ = fs::remove_dir_all(&temp);
            }
            done
        }
    }
}

/// Packs `paths` (all in `dir`) into `dir/<name>.<ext>`, the format chosen by the suffix.
pub fn compress(paths: &[PathBuf], dir: &Path, name: &str, ext: &str) -> Result<PathBuf, String> {
    if paths.iter().any(|p| p.parent() != Some(dir)) {
        return Err(tr("Compress files from one folder at a time"));
    }
    let temp = scratch(dir, &format!(".{ext}"));
    let mut command = Command::new("bsdtar");
    command.arg("-a").arg("-cf").arg(&temp).arg("-C").arg(dir);
    for p in paths {
        let file = p
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        // A name starting with "-" would read as an option.
        command.arg(if file.starts_with('-') {
            format!("./{file}")
        } else {
            file
        });
    }
    let target = unique(dir, &format!("{name}.{ext}"));
    run(&mut command)
        .and_then(|_| fs::rename(&temp, &target).map_err(|e| e.to_string()))
        .map(|_| target)
        .inspect_err(|_| {
            let _ = fs::remove_file(&temp);
        })
}

/// The name Compress suggests: the item's own for one, the folder's for several.
pub fn suggested_name(paths: &[PathBuf], dir: &Path) -> String {
    let base = match paths {
        [one] => one.file_name(),
        _ => dir.file_name(),
    };
    let base = base
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| tr("Archive"));
    match paths {
        // "notes.txt" packs as "notes.zip", but a folder keeps its whole name.
        [one] if !one.is_dir() => match base.rfind('.') {
            Some(i) if i > 0 => base[..i].to_string(),
            _ => base,
        },
        _ => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ferry-archive-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("trip/day1")).unwrap();
        fs::write(dir.join("trip/day1/notes.txt"), "belem").unwrap();
        fs::write(dir.join("trip/plan.md"), "# plan").unwrap();
        fs::write(dir.join("loose.txt"), "loose").unwrap();
        dir
    }

    #[test]
    fn knows_archives_by_their_whole_suffix() {
        assert_eq!(split("photos.tar.gz"), Some(("photos", "tar.gz")));
        assert_eq!(split("Report.ZIP"), Some(("Report", "ZIP")));
        assert_eq!(kind("notes.txt.gz"), Some(Kind::Compressed("gzip")));
        assert_eq!(kind("photos.tar.zst"), Some(Kind::Archive));
        assert_eq!(kind("notes.txt"), None);
        assert_eq!(
            kind(".zip"),
            None,
            "a hidden file named zip isn't an archive"
        );
    }

    #[test]
    fn compresses_and_extracts_every_offered_format() {
        let dir = temp();
        for (ext, _) in formats() {
            let made = compress(
                &[dir.join("trip"), dir.join("loose.txt")],
                &dir,
                "bundle",
                ext,
            )
            .unwrap();
            assert_eq!(made, dir.join(format!("bundle.{ext}")));
            let mut names = list(&made).unwrap();
            names.sort();
            assert!(
                names.contains(&"trip/day1/notes.txt".to_string()),
                "{ext}: {names:?}"
            );

            // Two things inside: they land in a folder named after the archive.
            let out = extract(&made, &dir, false).unwrap();
            assert_eq!(out, dir.join("bundle"), "{ext}");
            assert_eq!(
                fs::read_to_string(out.join("trip/day1/notes.txt")).unwrap(),
                "belem"
            );
            fs::remove_dir_all(&out).unwrap();
            fs::remove_file(&made).unwrap();
        }
        assert!(fs::read_dir(&dir).unwrap().all(|e| {
            !e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".ferry-")
        }));
    }

    #[test]
    fn a_single_folder_comes_out_as_itself_without_overwriting() {
        let dir = temp();
        let made = compress(
            &[dir.join("trip")],
            &dir,
            &suggested_name(&[dir.join("trip")], &dir),
            "tar.gz",
        )
        .unwrap();
        assert_eq!(made, dir.join("trip.tar.gz"));
        // "trip" is still there, so the copy is numbered rather than merged into it.
        assert_eq!(extract(&made, &dir, false).unwrap(), dir.join("trip (1)"));
        assert_eq!(extract(&made, &dir, true).unwrap(), dir.join("trip (2)"));
        assert!(
            dir.join("trip (2)/trip/plan.md").exists(),
            "into_folder wraps it"
        );
        // Numbering goes before the whole suffix.
        assert_eq!(
            compress(&[dir.join("trip")], &dir, "trip", "tar.gz").unwrap(),
            dir.join("trip (1).tar.gz")
        );
    }

    #[test]
    fn single_compressed_files_and_broken_archives() {
        let dir = temp();
        Command::new("gzip")
            .arg("-k")
            .arg(dir.join("loose.txt"))
            .status()
            .unwrap();
        fs::remove_file(dir.join("loose.txt")).unwrap();
        assert_eq!(
            extract(&dir.join("loose.txt.gz"), &dir, false).unwrap(),
            dir.join("loose.txt")
        );
        assert_eq!(fs::read_to_string(dir.join("loose.txt")).unwrap(), "loose");

        fs::write(dir.join("broken.zip"), "not a zip").unwrap();
        assert!(extract(&dir.join("broken.zip"), &dir, false).is_err());
        assert!(list(&dir.join("broken.zip")).is_err());
        assert!(
            fs::read_dir(&dir).unwrap().all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".ferry-")),
            "nothing left behind"
        );
    }

    #[test]
    fn suggests_names() {
        let dir = temp();
        assert_eq!(suggested_name(&[dir.join("loose.txt")], &dir), "loose");
        assert_eq!(suggested_name(&[dir.join("trip")], &dir), "trip");
        let both = [dir.join("trip"), dir.join("loose.txt")];
        assert_eq!(
            suggested_name(&both, &dir),
            dir.file_name().unwrap().to_string_lossy()
        );
    }
}
