// Previews of images, videos, PDFs and office files, following the freedesktop thumbnail
// spec: thumbnails live in ~/.cache/thumbnails/<size>/<md5 of the file URI>.png, tagged with
// the file's URI and mtime, so Ferry reads what other file managers made and they read
// Ferry's. Missing ones are made by the thumbnailers the system already has installed
// (/usr/share/thumbnailers), on background threads, newest request first.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use md5::{Digest, Md5};

use crate::files::{self, file_uri};

/// The spec's size directories and their longest edge in pixels.
const SIZES: [(&str, u32); 4] = [
    ("normal", 128),
    ("large", 256),
    ("x-large", 512),
    ("xx-large", 1024),
];
const WORKERS: usize = 3;
/// A thumbnailer that takes longer than this on one file is stopped.
const TIMEOUT: Duration = Duration::from_secs(15);

/// Which thumbnailer handles each MIME type, and which MIME type each extension is.
#[derive(Default)]
pub struct Registry {
    by_mime: HashMap<String, String>,
    by_ext: HashMap<String, String>,
}

impl Registry {
    pub fn load() -> Self {
        let mut registry = Registry::default();
        let home = files::home();
        // User thumbnailers come last so they override the system's.
        for dir in [
            PathBuf::from("/usr/share/thumbnailers"),
            home.join(".local/share/thumbnailers"),
        ] {
            let Ok(entries) = fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                registry.add_thumbnailer(&fs::read_to_string(entry.path()).unwrap_or_default());
            }
        }
        for globs in [
            "/usr/share/mime/globs2".into(),
            home.join(".local/share/mime/globs2"),
        ] {
            registry.add_globs(&fs::read_to_string(globs).unwrap_or_default());
        }
        registry
    }

    fn add_thumbnailer(&mut self, text: &str) {
        let field = |key: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(key))
                .map(str::trim)
        };
        let (Some(exec), Some(mimes)) = (field("Exec="), field("MimeType=")) else {
            return;
        };
        for mime in mimes.split(';').filter(|m| !m.is_empty()) {
            self.by_mime.insert(mime.to_owned(), exec.to_owned());
        }
    }

    /// globs2 lines are `weight:mime:pattern`, heaviest first; only plain `*.ext` patterns matter here.
    fn add_globs(&mut self, text: &str) {
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let mut parts = line.splitn(3, ':');
            let (Some(_), Some(mime), Some(pattern)) = (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let Some(ext) = pattern
                .strip_prefix("*.")
                .filter(|e| !e.contains(['*', '?', '[']))
            else {
                continue;
            };
            self.by_ext
                .entry(ext.to_lowercase())
                .or_insert_with(|| mime.to_owned());
        }
    }

    /// The thumbnailer command for a file, if anything can preview it.
    pub fn exec_for(&self, path: &Path) -> Option<&str> {
        let name = path.file_name()?.to_string_lossy().to_lowercase();
        // Longest extension first, so "x.tar.gz" finds "tar.gz" before "gz".
        let mime = name
            .match_indices('.')
            .find_map(|(i, _)| self.by_ext.get(&name[i + 1..]))?;
        self.by_mime.get(mime).map(String::as_str)
    }
}

pub fn cache_root() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| files::home().join(".cache"))
        .join("thumbnails")
}

pub fn cache_name(uri: &str) -> String {
    let hex: String = Md5::digest(uri.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    hex + ".png"
}

/// The smallest spec size whose thumbnails are at least `pixels` across.
pub fn size_for(pixels: f32) -> usize {
    SIZES
        .iter()
        .position(|(_, s)| *s as f32 >= pixels)
        .unwrap_or(SIZES.len() - 1)
}

struct Decoded {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
    text: Vec<(String, String)>,
}

fn decode(path: &Path) -> Option<Decoded> {
    let mut decoder = png::Decoder::new(BufReader::new(File::open(path).ok()?));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let frame = reader.next_frame(&mut buf).ok()?;
    let _ = reader.finish();
    let pixels = &buf[..frame.buffer_size()];
    let rgba = match frame.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::Rgb => pixels
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => pixels
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => pixels.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    let text = reader
        .info()
        .uncompressed_latin1_text
        .iter()
        .map(|t| (t.keyword.clone(), t.text.clone()))
        .collect();
    Some(Decoded {
        width: frame.width as usize,
        height: frame.height as usize,
        rgba,
        text,
    })
}

/// A cached thumbnail is valid when it is tagged with the file's current mtime.
fn read_cached(path: &Path, mtime: u64) -> Option<Decoded> {
    let decoded = decode(path)?;
    let tagged = decoded
        .text
        .iter()
        .any(|(k, v)| k == "Thumb::MTime" && v.parse::<u64>().ok() == Some(mtime));
    tagged.then_some(decoded)
}

/// Writes a thumbnail with the spec's URI and mtime tags, beside its final name and then renamed,
/// so other programs reading the cache never see half a file.
fn write_cached(path: &Path, image: &Decoded, uri: &str, mtime: u64) -> Option<()> {
    fs::create_dir_all(path.parent()?).ok()?;
    let temp = path.with_extension(format!("ferry-{}.tmp", std::process::id()));
    let mut encoder = png::Encoder::new(
        BufWriter::new(File::create(&temp).ok()?),
        image.width as u32,
        image.height as u32,
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .add_text_chunk("Thumb::URI".into(), uri.into())
        .ok()?;
    encoder
        .add_text_chunk("Thumb::MTime".into(), mtime.to_string())
        .ok()?;
    let mut writer = encoder.write_header().ok()?;
    let written = writer
        .write_image_data(&image.rgba)
        .and_then(|_| writer.finish());
    if written.is_err() || fs::rename(&temp, path).is_err() {
        let _ = fs::remove_file(&temp);
        return None;
    }
    Some(())
}

/// Fills in a thumbnailer's Exec line (%i path, %u URI, %o output, %s size).
pub fn command_for(
    exec: &str,
    input: &Path,
    uri: &str,
    output: &Path,
    size: u32,
) -> Option<Command> {
    let args: Vec<String> = exec
        .split_whitespace()
        .map(|arg| {
            arg.replace("%i", &input.to_string_lossy())
                .replace("%u", uri)
                .replace("%o", &output.to_string_lossy())
                .replace("%s", &size.to_string())
                .replace("%%", "%")
        })
        .collect();
    let (program, rest) = args.split_first()?;
    let mut command = Command::new(program);
    command
        .args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    Some(command)
}

fn run(mut command: Command) -> bool {
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if start.elapsed() < TIMEOUT => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

struct Job {
    path: PathBuf,
    mtime: u64,
    size: usize,
    exec: String,
}

/// Finds or makes one thumbnail. A cached one at least as large as asked for is used as is.
fn produce(job: &Job, root: &Path) -> Option<Decoded> {
    let uri = file_uri(&job.path);
    let name = cache_name(&uri);
    for (dir, _) in &SIZES[job.size..] {
        if let Some(found) = read_cached(&root.join(dir).join(&name), job.mtime) {
            return Some(found);
        }
    }
    let (dir, pixels) = SIZES[job.size];
    let target = root.join(dir).join(&name);
    let raw = std::env::temp_dir().join(format!("ferry-thumb-{}-{name}", std::process::id()));
    let made = command_for(&job.exec, &job.path, &uri, &raw, pixels).is_some_and(run);
    let image = made.then(|| decode(&raw)).flatten();
    let _ = fs::remove_file(&raw);
    let image = image?;
    write_cached(&target, &image, &uri, job.mtime);
    Some(image)
}

/// A file as it was when its preview was made: an edited file gets a new preview.
type Key = (PathBuf, u64);

enum Slot {
    Waiting(usize),
    Ready(usize, TextureHandle),
    Failed,
}

pub struct Thumbs {
    registry: Arc<Registry>,
    queue: Arc<(Mutex<Vec<Job>>, Condvar)>,
    results: Receiver<(Key, usize, Option<ColorImage>)>,
    slots: HashMap<Key, Slot>,
}

impl Thumbs {
    pub fn new(ctx: &egui::Context) -> Self {
        Self::with_cache(ctx, cache_root())
    }

    pub fn with_cache(ctx: &egui::Context, root: PathBuf) -> Self {
        let registry = Arc::new(Registry::load());
        let queue: Arc<(Mutex<Vec<Job>>, Condvar)> = Arc::default();
        let (tx, results): (Sender<_>, _) = channel();
        for _ in 0..WORKERS {
            let (queue, tx, ctx, root) = (queue.clone(), tx.clone(), ctx.clone(), root.clone());
            std::thread::spawn(move || {
                loop {
                    // Newest first: what was asked for last is what is on screen now.
                    let job = {
                        let (lock, ready) = &*queue;
                        let mut jobs = lock.lock().unwrap();
                        loop {
                            match jobs.pop() {
                                Some(job) => break job,
                                None => jobs = ready.wait(jobs).unwrap(),
                            }
                        }
                    };
                    let started = Instant::now();
                    let made = produce(&job, &root);
                    crate::app::debug(&format!(
                        "thumbnail {}: {} in {:?}",
                        job.path.display(),
                        if made.is_some() { "ok" } else { "failed" },
                        started.elapsed()
                    ));
                    let image = made
                        .map(|d| ColorImage::from_rgba_unmultiplied([d.width, d.height], &d.rgba));
                    if tx.send(((job.path, job.mtime), job.size, image)).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            });
        }
        Thumbs {
            registry,
            queue,
            results,
            slots: HashMap::new(),
        }
    }

    /// Drops every preview and pending request, as when another folder opens.
    pub fn clear(&mut self) {
        self.slots.clear();
        self.queue.0.lock().unwrap().clear();
    }

    /// Takes finished thumbnails onto the GPU; call once a frame.
    pub fn collect(&mut self, ctx: &egui::Context) {
        while let Ok((key, size, image)) = self.results.try_recv() {
            // Ignore results for a folder that is no longer shown.
            let Some(slot) = self.slots.get_mut(&key) else {
                continue;
            };
            match image {
                Some(image) => {
                    *slot = Slot::Ready(
                        size,
                        ctx.load_texture(key.0.to_string_lossy(), image, TextureOptions::LINEAR),
                    );
                }
                // A sharper preview that failed leaves the smaller one in place.
                None if matches!(slot, Slot::Ready(..)) => {}
                None => *slot = Slot::Failed,
            }
        }
    }

    /// The preview for `path` to show `pixels` across, asking for it (or a sharper one) if needed.
    pub fn get(
        &mut self,
        path: &Path,
        mtime: Option<std::time::SystemTime>,
        pixels: f32,
    ) -> Option<&TextureHandle> {
        let size = size_for(pixels);
        let mtime = mtime
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());
        let key = (path.to_path_buf(), mtime);
        let have = match self.slots.get(&key) {
            Some(Slot::Failed) => return None,
            Some(Slot::Waiting(s)) => Some(*s),
            Some(Slot::Ready(s, _)) => Some(*s),
            None => None,
        };
        if have.is_none_or(|s| s < size) {
            let Some(exec) = self.registry.exec_for(path).map(str::to_owned) else {
                crate::app::debug(&format!("no thumbnailer for {}", path.display()));
                self.slots.insert(key, Slot::Failed);
                return None;
            };
            let (lock, ready) = &*self.queue;
            lock.lock().unwrap().push(Job {
                path: path.to_path_buf(),
                mtime,
                size,
                exec,
            });
            ready.notify_one();
            // Keep showing a smaller preview while the sharper one is made.
            match self.slots.get_mut(&key) {
                Some(Slot::Ready(s, _)) => *s = size,
                _ => {
                    self.slots.insert(key.clone(), Slot::Waiting(size));
                }
            }
        }
        match self.slots.get(&key) {
            Some(Slot::Ready(_, texture)) => Some(texture),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_names_follow_the_spec() {
        // The spec's own example: md5 of "file:///home/jens/photos/me.png".
        assert_eq!(
            cache_name("file:///home/jens/photos/me.png"),
            "c6ee772d9e49320e97ec29a7eb5b1697.png"
        );
    }

    #[test]
    fn sizes_pick_the_smallest_that_fits() {
        assert_eq!(SIZES[size_for(64.0)].0, "normal");
        assert_eq!(SIZES[size_for(129.0)].0, "large");
        assert_eq!(SIZES[size_for(5000.0)].0, "xx-large");
    }

    #[test]
    fn thumbnailers_match_by_extension() {
        let mut r = Registry::default();
        r.add_thumbnailer("[Thumbnailer Entry]\nExec=thumb -s %s %u %o\nMimeType=image/png;application/x-compressed-tar;\n");
        r.add_globs("50:image/png:*.png\n50:application/x-compressed-tar:*.tar.gz\n50:application/gzip:*.gz\n");
        assert_eq!(r.exec_for(Path::new("/a/B.PNG")), Some("thumb -s %s %u %o"));
        assert_eq!(
            r.exec_for(Path::new("/a/x.tar.gz")),
            Some("thumb -s %s %u %o")
        );
        assert_eq!(r.exec_for(Path::new("/a/notes.txt")), None);
    }

    #[test]
    fn commands_fill_in_the_placeholders() {
        let c = command_for(
            "t -s %s -i %i %u %o",
            Path::new("/a b.png"),
            "file:///a%20b.png",
            Path::new("/o.png"),
            256,
        )
        .unwrap();
        let args: Vec<_> = c
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            ["-s", "256", "-i", "/a b.png", "file:///a%20b.png", "/o.png"]
        );
    }

    #[test]
    fn cache_round_trips_and_goes_stale_with_mtime() {
        let dir = std::env::temp_dir().join(format!("ferry-thumbs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let image = Decoded {
            width: 2,
            height: 1,
            rgba: vec![255, 0, 0, 255, 0, 0, 255, 128],
            text: vec![],
        };
        let path = dir.join("normal/x.png");
        write_cached(&path, &image, "file:///x", 42).unwrap();
        let back = read_cached(&path, 42).unwrap();
        assert_eq!((back.width, back.height, back.rgba), (2, 1, image.rgba));
        assert!(read_cached(&path, 43).is_none());
    }
}
