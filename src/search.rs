// Searching below a folder, for names or for file contents, without an index: the same
// engine as ripgrep and fd. The `ignore` crate walks folders on all cores; names are ranked
// with nucleo (Helix's fuzzy matcher, fzf's scoring); contents are searched with ripgrep's
// own searcher, which skips binary files. Results stream back while the walk goes on, and a
// newer search cancels the older one at once.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;

use grep_matcher::Matcher as _;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{BinaryDetection, SearcherBuilder, sinks};
use ignore::{WalkBuilder, WalkState};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Names,
    Contents,
}

#[derive(Clone, Debug)]
pub struct Query {
    pub text: String,
    pub mode: Mode,
    pub root: PathBuf,
    /// Also look in hidden files and in what .gitignore / .ignore files exclude.
    pub everything: bool,
}

/// One line of a file that matched a contents search.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub number: u64,
    pub text: String,
    /// Byte ranges of the matches within `text`.
    pub matches: Vec<(usize, usize)>,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub path: PathBuf,
    pub is_dir: bool,
    /// Fuzzy score for name searches; higher is better.
    pub score: u32,
    /// For name searches, the matched characters of the path relative to the root.
    pub indices: Vec<u32>,
    /// For contents searches, the first few matching lines.
    pub lines: Vec<Line>,
}

pub enum Update {
    Hit(u64, Hit),
    Done(u64),
}

/// Files bigger than this aren't searched for contents (logs, disk images, databases).
const MAX_CONTENT_BYTES: u64 = 50 * 1024 * 1024;
/// Matching lines kept per file, and their length on screen.
pub const LINES_PER_FILE: usize = 3;
const MAX_LINE_CHARS: usize = 240;
/// A search stops after this many hits; the results say it was cut short.
pub const MAX_HITS: usize = 5000;

/// Shared between the window and a running search.
#[derive(Default)]
pub struct Progress {
    /// The search that may run; anything older stops at its next file.
    pub current: AtomicU64,
    pub scanned: AtomicUsize,
    pub hits: AtomicUsize,
}

/// Runs `query` as search `id` on background threads, sending hits to `tx`.
pub fn start(
    query: Query,
    id: u64,
    progress: Arc<Progress>,
    tx: Sender<Update>,
    wake: impl Fn() + Send + Sync + 'static,
) {
    progress.current.store(id, Ordering::SeqCst);
    progress.scanned.store(0, Ordering::Relaxed);
    progress.hits.store(0, Ordering::Relaxed);
    let wake = Arc::new(wake);
    std::thread::spawn(move || {
        run(&query, id, &progress, &tx, &*wake);
        let _ = tx.send(Update::Done(id));
        wake();
    });
}

fn run(
    query: &Query,
    id: u64,
    progress: &Progress,
    tx: &Sender<Update>,
    wake: &(dyn Fn() + Send + Sync),
) {
    let text = query.text.trim();
    if text.is_empty() {
        return;
    }
    let mut walk = WalkBuilder::new(&query.root);
    walk.hidden(!query.everything)
        .git_ignore(!query.everything)
        .git_global(!query.everything)
        .git_exclude(!query.everything)
        .ignore(!query.everything)
        // Only look at .gitignore files inside a repository, not above the search root.
        .parents(false)
        .require_git(true)
        // Don't wander into other drives (a NAS mounted inside home, /proc under /).
        .same_file_system(true)
        .follow_links(false);

    let names = Pattern::parse(text, CaseMatching::Smart, Normalization::Smart);
    let contents = (query.mode == Mode::Contents)
        .then(|| {
            RegexMatcherBuilder::new()
                .case_smart(true)
                .fixed_strings(true)
                .build(text)
                .ok()
        })
        .flatten();
    if query.mode == Mode::Contents && contents.is_none() {
        return;
    }

    walk.build_parallel().run(|| {
        let names = names.clone();
        let contents = contents.clone();
        let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
        let mut buf = Vec::new();
        let mut searcher = SearcherBuilder::new()
            .binary_detection(BinaryDetection::quit(0))
            .line_number(true)
            .build();
        Box::new(move |entry| {
            if progress.current.load(Ordering::Relaxed) != id
                || progress.hits.load(Ordering::Relaxed) >= MAX_HITS
            {
                return WalkState::Quit;
            }
            let Ok(entry) = entry else {
                return WalkState::Continue;
            };
            if entry.depth() == 0 {
                return WalkState::Continue;
            }
            progress.scanned.fetch_add(1, Ordering::Relaxed);
            let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
            let hit = match &contents {
                None => {
                    let relative = entry
                        .path()
                        .strip_prefix(&query.root)
                        .unwrap_or(entry.path());
                    let haystack = relative.to_string_lossy();
                    let utf32 = Utf32Str::new(&haystack, &mut buf);
                    let mut indices = Vec::new();
                    names
                        .indices(utf32, &mut matcher, &mut indices)
                        .map(|score| Hit {
                            path: entry.path().to_path_buf(),
                            is_dir,
                            score,
                            indices,
                            lines: Vec::new(),
                        })
                }
                Some(regex) => {
                    let small = entry.metadata().is_ok_and(|m| m.len() <= MAX_CONTENT_BYTES);
                    if is_dir || !small || !entry.file_type().is_some_and(|t| t.is_file()) {
                        return WalkState::Continue;
                    }
                    let lines = search_file(&mut searcher, regex, entry.path());
                    (!lines.is_empty()).then(|| Hit {
                        path: entry.path().to_path_buf(),
                        is_dir: false,
                        score: 0,
                        indices: Vec::new(),
                        lines,
                    })
                }
            };
            if let Some(hit) = hit {
                progress.hits.fetch_add(1, Ordering::Relaxed);
                if tx.send(Update::Hit(id, hit)).is_err() {
                    return WalkState::Quit;
                }
                wake();
            }
            WalkState::Continue
        })
    });
}

/// The first few matching lines of one file, with where in each line the matches are.
fn search_file(
    searcher: &mut grep_searcher::Searcher,
    regex: &grep_regex::RegexMatcher,
    path: &Path,
) -> Vec<Line> {
    let mut lines = Vec::new();
    let _ = searcher.search_path(
        regex,
        path,
        sinks::UTF8(|number, line| {
            let text: String = line
                .trim_end_matches(['\n', '\r'])
                .chars()
                .take(MAX_LINE_CHARS)
                .collect();
            let mut matches = Vec::new();
            let _ = regex.find_iter(text.as_bytes(), |m| {
                matches.push((m.start(), m.end()));
                true
            });
            lines.push(Line {
                number,
                text,
                matches,
            });
            Ok(lines.len() < LINES_PER_FILE)
        }),
    );
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::mpsc::channel;

    fn tree() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ferry-search-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Projects/ferry/src")).unwrap();
        fs::create_dir_all(dir.join("Projects/ferry/target")).unwrap();
        fs::create_dir_all(dir.join(".secret")).unwrap();
        fs::write(
            dir.join("Projects/notes.md"),
            "Buy milk\nCall Ana about the Lisbon trip\n",
        )
        .unwrap();
        fs::write(
            dir.join("Projects/ferry/src/main.rs"),
            "fn main() {\n    println!(\"lisbon\");\n}\n",
        )
        .unwrap();
        fs::write(
            dir.join("Projects/ferry/target/build.log"),
            "lisbon in a build artifact\n",
        )
        .unwrap();
        fs::write(dir.join("Projects/ferry/.gitignore"), "target/\n").unwrap();
        fs::create_dir_all(dir.join("Projects/ferry/.git")).unwrap();
        fs::write(dir.join(".secret/lisbon.txt"), "lisbon\n").unwrap();
        fs::write(
            dir.join("photo.jpg"),
            [0xff, 0xd8, 0, 0, b'l', b'i', b's', b'b', b'o', b'n'],
        )
        .unwrap();
        dir
    }

    fn search(root: &Path, text: &str, mode: Mode, everything: bool) -> Vec<Hit> {
        let (tx, rx) = channel();
        let query = Query {
            text: text.into(),
            mode,
            root: root.to_path_buf(),
            everything,
        };
        start(query, 1, Arc::default(), tx, || {});
        let mut hits = Vec::new();
        for update in rx {
            match update {
                Update::Hit(_, hit) => hits.push(hit),
                Update::Done(_) => break,
            }
        }
        hits.sort_by(|a, b| b.score.cmp(&a.score).then(a.path.cmp(&b.path)));
        hits
    }

    fn names(root: &Path, hits: &[Hit]) -> Vec<String> {
        hits.iter()
            .map(|h| {
                h.path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    #[test]
    fn names_match_fuzzily_across_the_path() {
        let root = tree();
        let hits = search(&root, "proj notes", Mode::Names, false);
        assert_eq!(names(&root, &hits)[0], "Projects/notes.md");
        // "mainrs" finds main.rs without the dot, and ranks it first.
        assert_eq!(
            names(&root, &search(&root, "mainrs", Mode::Names, false))[0],
            "Projects/ferry/src/main.rs"
        );
    }

    #[test]
    fn contents_skip_binary_hidden_and_ignored_files_unless_asked() {
        let root = tree();
        let hits = search(&root, "Lisbon", Mode::Contents, false);
        // Smart case: "Lisbon" has a capital, so only the exact-case line matches.
        assert_eq!(names(&root, &hits), ["Projects/notes.md"]);
        assert_eq!(
            hits[0].lines,
            vec![Line {
                number: 2,
                text: "Call Ana about the Lisbon trip".into(),
                matches: vec![(19, 25)]
            }]
        );

        let lower = names(&root, &search(&root, "lisbon", Mode::Contents, false));
        assert_eq!(lower, ["Projects/ferry/src/main.rs", "Projects/notes.md"]);

        let all = names(&root, &search(&root, "lisbon", Mode::Contents, true));
        assert!(all.contains(&".secret/lisbon.txt".to_string()));
        assert!(all.contains(&"Projects/ferry/target/build.log".to_string()));
        assert!(
            !all.contains(&"photo.jpg".to_string()),
            "binary files are skipped"
        );
    }

    #[test]
    fn a_newer_search_cancels_the_older_one() {
        let root = tree();
        let progress: Arc<Progress> = Arc::default();
        progress.current.store(2, Ordering::SeqCst);
        let (tx, rx) = channel();
        let query = Query {
            text: "lisbon".into(),
            mode: Mode::Contents,
            root: root.clone(),
            everything: true,
        };
        // Run search 1 directly while search 2 is current: it stops before sending anything.
        run(&query, 1, &progress, &tx, &|| {});
        drop(tx);
        assert!(rx.iter().next().is_none());
    }

    /// `FERRY_BENCH=<dir> cargo test --release bench -- --ignored --nocapture` times real searches.
    #[test]
    #[ignore]
    fn bench() {
        let Some(root) = std::env::var_os("FERRY_BENCH") else {
            return;
        };
        let root = PathBuf::from(root);
        for (text, mode) in [("main", Mode::Names), ("fn main", Mode::Contents)] {
            let started = std::time::Instant::now();
            let hits = search(&root, text, mode, false).len();
            println!("{mode:?} {text:?}: {hits} hits in {:?}", started.elapsed());
        }
    }
}
