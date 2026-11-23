// The window: places on the left, the folder in the middle, a path bar on top and a status bar below.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, SystemTime};

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, Event, FontId, Galley, Key, Layout, Margin,
    Modifiers, Panel, Rect, RichText, ScrollArea, Sense, Stroke, TextEdit, Ui, Vec2,
    text::LayoutJob, vec2,
};

use crate::devices::{self, Device};
use crate::files::{self, Entry, Kind, SortKey};
use crate::i18n::{self, tr, trn};
use crate::search;
use crate::theme::{Palette, Theme};

const ROW: f32 = 28.0;
/// Sidebar rows are a little taller than file rows: fewer, and aimed at more often.
const PLACE_ROW: f32 = 32.0;
/// Icon-view tile width at 100% zoom; the height follows from the icon and two lines of name.
const TILE_WIDTH: f32 = 112.0;

enum Dialog {
    Rename {
        path: PathBuf,
        text: String,
        error: Option<String>,
    },
    NewFolder {
        text: String,
        error: Option<String>,
    },
    NewFile {
        text: String,
        error: Option<String>,
    },
    Delete {
        paths: Vec<PathBuf>,
    },
    Help,
    /// Erasing a drive: which filesystem (an index into `devices::formats()`) and its name.
    Format {
        device: Device,
        fs: usize,
        label: String,
    },
    /// Closing the editor (or quitting, when `quit`) with unsaved changes.
    Unsaved {
        quit: bool,
    },
    /// The file changed on disk after it was opened in the editor.
    ChangedOnDisk,
}

/// What to do once the editor's save finishes.
#[derive(Clone, Copy, PartialEq)]
enum AfterSave {
    Close,
    Quit,
}

enum Action {
    Go(PathBuf),
    Back,
    Forward,
    Up,
    Open,
    Trash,
    AskDelete,
    AskRename,
    AskNewFolder,
    AskNewFile,
    /// Search below the shown folder.
    Search,
    CloseSearch,
    /// Open a search result; `line` opens a contents match in the editor at that line.
    OpenFound {
        path: PathBuf,
        is_dir: bool,
        line: Option<u64>,
    },
    /// Show a file in its folder, selected.
    Reveal(PathBuf),
    /// Open a drive, mounting it first if it isn't.
    OpenDevice(Device),
    Unmount(Device),
    Eject(Device),
    AskFormat(Device),
    Copy,
    Cut,
    Paste,
    PasteText(String),
    CopyPath,
    Terminal,
    Refresh,
    ToggleHidden,
    SelectAll,
    AddPlaces(Vec<PathBuf>),
    RemovePlace(PathBuf),
    /// Open in the built-in editor even if the file's type says otherwise.
    EditText(PathBuf),
    OpenExternal(PathBuf),
    CloseEditor,
    /// Save the editor; `true` overwrites changes made on disk since it was opened.
    SaveEditor(bool),
}

/// What is being dragged out of the folder view; `folders` is known from the listing, so the
/// sidebar never has to touch the disk to tell.
struct Dragged {
    paths: Vec<PathBuf>,
    folders: Vec<PathBuf>,
}

/// A folder read on a background thread.
struct Load {
    generation: u64,
    path: PathBuf,
    select: Option<PathBuf>,
    history: bool,
    fresh: bool,
    result: std::io::Result<Vec<Entry>>,
    mtime: Option<SystemTime>,
    free: Option<u64>,
}

enum Loaded {
    Dir(Load),
    /// The once-a-second look at whether the shown folder changed.
    Check {
        path: PathBuf,
        mtime: Option<SystemTime>,
        free: Option<u64>,
    },
    /// A file read for the editor; `forced` when asked for with "Open in Text Editor".
    Text {
        path: PathBuf,
        forced: bool,
        result: std::io::Result<(crate::editor::Opened, Option<SystemTime>)>,
    },
    /// A fresh look at the drives and shares.
    Devices(Vec<Device>),
    /// A drive finished mounting; open it.
    Mounted(Result<PathBuf, String>),
    /// The editor's save finished; `text` is what was saved.
    Saved {
        path: PathBuf,
        text: String,
        result: Result<Option<SystemTime>, crate::editor::SaveError>,
    },
}

/// A search below a folder, shown in place of the folder.
struct SearchView {
    root: PathBuf,
    text: String,
    mode: search::Mode,
    everything: bool,
    hits: Vec<search::Hit>,
    sorted: bool,
    done: bool,
    /// When the text last changed: the search runs once typing pauses.
    typed: Option<f64>,
    /// Row under the keyboard cursor, in the list of results and their lines.
    cursor: usize,
    scroll: bool,
    focus: bool,
    /// Scroll offset and height of the results, for keeping the cursor in view.
    viewport: (f32, f32),
}

/// A row of search results: a file or folder, or one matching line of a file.
#[derive(Clone, Copy)]
enum Row {
    Hit(usize),
    Line(usize, usize),
}

struct Clip {
    paths: Vec<PathBuf>,
    cut: bool,
    text: String,
}

/// A finished background job; `job` is false for fire-and-forget launches that only report failures.
struct Done {
    job: bool,
    result: Result<String, String>,
}

pub struct App {
    ctx: egui::Context,
    theme: Theme,
    language: String,
    last_poll: f64,

    cwd: PathBuf,
    back: Vec<PathBuf>,
    forward: Vec<PathBuf>,
    entries: Vec<Entry>,
    view: Vec<usize>,
    dir_mtime: Option<SystemTime>,
    free: Option<u64>,
    places: Vec<(String, PathBuf, char)>,
    bookmarks: Vec<(String, PathBuf)>,
    bookmarks_file: PathBuf,
    devices: Vec<Device>,
    scanning_devices: bool,

    show_hidden: bool,
    sort: SortKey,
    ascending: bool,
    grid: bool,
    zoom_list: f32,
    zoom_grid: f32,
    zoom_editor: f32,
    /// Overall size; None follows the desktop's text scaling.
    ui_scale: Option<f32>,
    /// Background opacity; None follows Omarchy's window opacity setting.
    opacity: Option<f32>,
    /// The size slider's value while it's being dragged.
    scale_edit: Option<f32>,

    filter: String,
    focus_filter: bool,
    path_edit: Option<String>,

    selected: HashSet<PathBuf>,
    cursor: Option<usize>,
    anchor: Option<usize>,
    scroll_to_cursor: bool,
    viewport: (f32, f32),
    jump: (String, f64),

    dialog: Option<Dialog>,
    status: Option<(String, bool)>,
    clip: Option<Clip>,
    busy: usize,
    tx: Sender<Done>,
    rx: Receiver<Done>,
    pending: Vec<Action>,
    load_tx: Sender<Loaded>,
    load_rx: Receiver<Loaded>,
    load_gen: u64,
    loading: Option<PathBuf>,
    checking: bool,
    dnd: Option<crate::dnd::Dnd>,
    /// Set when a drag left for another app: egui never sees that button release.
    release_pointer: bool,
    thumbs: crate::thumbs::Thumbs,
    /// Where settings are kept, and what was last written there; tests run without one.
    settings_file: Option<PathBuf>,
    saved_settings: String,
    editor: Option<crate::editor::Editor>,
    focus_editor: bool,
    /// The syntax colors made from the palette they were made from.
    code_theme: Option<(Palette, syntect::highlighting::Theme)>,
    /// The editor's last highlighted text, by a hash of what went into it.
    editor_layout: Option<(u64, egui::text::LayoutJob)>,
    search: Option<SearchView>,
    search_id: u64,
    search_progress: Arc<search::Progress>,
    search_tx: Sender<search::Update>,
    search_rx: Receiver<search::Update>,
    after_save: Option<AfterSave>,
    /// Set once quitting has been confirmed, so the close request goes through.
    allow_close: bool,
}

impl App {
    pub fn new(ctx: &egui::Context, start: PathBuf, select: Option<PathBuf>) -> Self {
        let language = i18n::read_language(&language_file());
        i18n::set(&language);
        let (tx, rx) = channel();
        let (load_tx, load_rx) = channel();
        let (search_tx, search_rx) = channel();
        let mut app = App {
            ctx: ctx.clone(),
            theme: Theme::new(ctx),
            language,
            last_poll: 0.0,
            cwd: start.clone(),
            back: Vec::new(),
            forward: Vec::new(),
            entries: Vec::new(),
            view: Vec::new(),
            dir_mtime: None,
            free: None,
            places: files::places(),
            bookmarks: files::bookmarks(&files::bookmarks_file()),
            bookmarks_file: files::bookmarks_file(),
            devices: Vec::new(),
            scanning_devices: false,
            show_hidden: false,
            sort: SortKey::Name,
            ascending: true,
            grid: false,
            zoom_list: 1.0,
            zoom_grid: 1.0,
            zoom_editor: 1.0,
            ui_scale: None,
            opacity: None,
            scale_edit: None,

            filter: String::new(),
            focus_filter: false,
            path_edit: None,
            selected: HashSet::new(),
            cursor: None,
            anchor: None,
            scroll_to_cursor: false,
            viewport: (0.0, 0.0),
            jump: (String::new(), 0.0),
            dialog: None,
            status: None,
            clip: None,
            busy: 0,
            tx,
            rx,
            pending: Vec::new(),
            load_tx,
            load_rx,
            load_gen: 0,
            loading: None,
            checking: false,
            dnd: None,
            release_pointer: false,
            thumbs: crate::thumbs::Thumbs::new(ctx),
            settings_file: None,
            saved_settings: String::new(),
            editor: None,
            focus_editor: false,
            code_theme: None,
            editor_layout: None,
            search: None,
            search_id: 0,
            search_progress: Arc::default(),
            search_tx,
            search_rx,
            after_save: None,
            allow_close: false,
        };
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        app.go(start, select, false);
        app
    }

    /// Restores the view, zoom, sorting and hidden-files choice from the last run, and keeps
    /// saving them as they change.
    pub fn with_settings(mut self, file: PathBuf) -> Self {
        let saved = std::fs::read_to_string(&file).unwrap_or_default();
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&saved) {
            let zoom = |key: &str, max: f64| v[key].as_f64().map(|z| z.clamp(0.6, max) as f32);
            self.grid = v["grid"].as_bool().unwrap_or(self.grid);
            self.show_hidden = v["show_hidden"].as_bool().unwrap_or(self.show_hidden);
            self.zoom_list = zoom("zoom_list", 2.5).unwrap_or(self.zoom_list);
            self.zoom_grid = zoom("zoom_grid", 4.0).unwrap_or(self.zoom_grid);
            self.zoom_editor = zoom("zoom_editor", 3.0).unwrap_or(self.zoom_editor);
            self.ui_scale = v["ui_scale"].as_f64().map(|z| z.clamp(0.75, 3.0) as f32);
            self.opacity = v["opacity"].as_f64().map(|o| o.clamp(0.3, 1.0) as f32);
            self.ascending = v["ascending"].as_bool().unwrap_or(self.ascending);
            self.sort = match v["sort"].as_str() {
                Some("size") => SortKey::Size,
                Some("modified") => SortKey::Modified,
                Some("name") => SortKey::Name,
                _ => self.sort,
            };
            // The first folder was asked for with the old sort order; ask again with this one.
            self.go(self.cwd.clone(), None, false);
        }
        self.saved_settings = self.settings_json();
        self.settings_file = Some(file);
        self
    }

    // Saved through f64, so 0.65 is written as 0.65 rather than as f32 noise.
    fn settings_json(&self) -> String {
        let sort = match self.sort {
            SortKey::Name => "name",
            SortKey::Size => "size",
            SortKey::Modified => "modified",
        };
        let settings = serde_json::json!({
            "grid": self.grid,
            "show_hidden": self.show_hidden,
            "zoom_list": (self.zoom_list as f64 * 100.0).round() / 100.0,
            "zoom_grid": (self.zoom_grid as f64 * 100.0).round() / 100.0,
            "zoom_editor": (self.zoom_editor as f64 * 100.0).round() / 100.0,
            "ui_scale": self.ui_scale.map(|z| (z as f64 * 100.0).round() / 100.0),
            "opacity": self.opacity.map(|o| (o as f64 * 100.0).round() / 100.0),
            "sort": sort,
            "ascending": self.ascending,
        });
        serde_json::to_string_pretty(&settings).unwrap_or_default()
    }

    /// Writes settings when they changed, beside the file and renamed over it.
    fn save_settings(&mut self) {
        let Some(file) = &self.settings_file else {
            return;
        };
        let json = self.settings_json();
        if json == self.saved_settings {
            return;
        }
        let temp = file.with_extension("json.tmp");
        let written = file
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|_| std::fs::write(&temp, format!("{json}\n")))
            .and_then(|_| std::fs::rename(&temp, file));
        match written {
            Ok(()) => {
                debug(&format!("settings saved to {}", file.display()));
                self.saved_settings = json;
            }
            Err(e) => {
                debug(&format!("saving settings: {e}"));
                // Don't retry every second; the next change tries again.
                self.saved_settings = json;
            }
        }
    }

    pub fn with_dnd(mut self, dnd: Option<crate::dnd::Dnd>) -> Self {
        debug(&format!(
            "drag to other apps: {}",
            if dnd.is_some() {
                "ready"
            } else {
                "unavailable"
            }
        ));
        self.dnd = dnd;
        self
    }

    /// A drag that leaves the window becomes a desktop drag, so files can be dropped into a
    /// browser, a chat or another file manager. Inside the window it stays Ferry's own.
    fn drag_out(&mut self, ctx: &egui::Context) {
        let Some(dragged) = egui::DragAndDrop::payload::<Dragged>(ctx) else {
            return;
        };
        let window = ctx.content_rect().shrink(2.0);
        if ctx.pointer_latest_pos().is_some_and(|p| window.contains(p)) {
            return;
        }
        let Some(dnd) = &self.dnd else { return };
        match dnd.start(&dragged.paths) {
            Ok(()) => {
                debug(&format!(
                    "drag handed to the desktop: {} items",
                    dragged.paths.len()
                ));
                egui::DragAndDrop::clear_payload(ctx);
                self.release_pointer = true;
            }
            Err(e) => debug(&format!("desktop drag failed: {e}")),
        }
    }

    fn p(&self) -> Palette {
        self.theme.palette
    }

    /// How opaque the window's background is. Glassy Omarchy windows are see-through like its
    /// terminals (0.65, over Hyprland's blur); text and icons always stay solid.
    fn opacity(&self) -> f32 {
        self.opacity
            .unwrap_or(if self.theme.glassy() { 0.65 } else { 1.0 })
    }

    /// A background color at the window's opacity.
    fn glass(&self, c: Color32) -> Color32 {
        c.gamma_multiply(self.opacity())
    }

    /// Overall size: the desktop's text scaling, as GTK apps use, unless set in Appearance.
    fn scale(&self) -> f32 {
        self.ui_scale.unwrap_or(self.theme.text_scale())
    }

    /// The button that opens Appearance. Unlike a menu of commands, it holds settings, so it
    /// stays open while they're changed and closes on a click outside it (or Esc).
    fn appearance_button(&mut self, ui: &mut Ui) {
        let config = egui::containers::menu::MenuConfig::new()
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
        let (button, _) =
            egui::containers::menu::MenuButton::new(RichText::new("\u{f1de}").size(15.0))
                .config(config)
                .ui(ui, |ui| self.appearance(ui));
        button.on_hover_text(tr("Appearance"));
    }

    /// The Appearance menu: size and transparency, following the desktop by default.
    fn appearance(&mut self, ui: &mut Ui) {
        let p = self.p();
        ui.set_min_width(260.0);
        let heading = ui.label(RichText::new(tr("Size")).color(p.dim).size(12.0));
        let mut follow = self.ui_scale.is_none();
        if ui
            .checkbox(&mut follow, tr("Follow the desktop's text size"))
            .changed()
        {
            self.ui_scale = (!follow).then(|| self.theme.text_scale());
        }
        // Resizing while dragging would move the slider out from under the pointer, so the
        // dragged value is held here and lands on release.
        let scale = self.scale_edit.get_or_insert(self.scale() * 100.0);
        let slider = egui::Slider::new(scale, 75.0..=250.0)
            .suffix("%")
            .step_by(5.0);
        let resp = ui.add_enabled(!follow, slider).labelled_by(heading.id);
        // Shown, the slider snaps the desktop's 163.64% to a 5% step. That isn't a choice, so
        // only a change while it's enabled (not following) counts.
        if !follow && (resp.drag_stopped() || (resp.changed() && !resp.dragged())) {
            self.ui_scale = self.scale_edit.map(|s| s / 100.0);
        }
        if !resp.dragged() {
            self.scale_edit = None;
        }
        ui.add_space(8.0);
        let heading = ui.label(RichText::new(tr("Transparency")).color(p.dim).size(12.0));
        let mut follow = self.opacity.is_none();
        if ui
            .checkbox(&mut follow, tr("Follow Omarchy's window opacity"))
            .changed()
        {
            self.opacity = (!follow).then(|| self.opacity());
        }
        let mut opacity = self.opacity() * 100.0;
        if ui
            .add_enabled(
                !follow,
                egui::Slider::new(&mut opacity, 30.0..=100.0)
                    .suffix("%")
                    .step_by(5.0),
            )
            .labelled_by(heading.id)
            .changed()
            && !follow
        {
            self.opacity = Some(opacity / 100.0);
        }
        ui.label(
            RichText::new(if self.theme.glassy() {
                tr("Omarchy's windows are glassy now.")
            } else {
                tr("Make Omarchy's windows glassy to see the desktop through Ferry.")
            })
            .color(p.dim)
            .size(12.0),
        );
    }

    // ----- Folders -------------------------------------------------------------------------

    fn go(&mut self, path: PathBuf, select: Option<PathBuf>, history: bool) {
        self.close_search();
        self.load(path, select, history, true);
    }

    /// Re-reads the folder, keeping the selection and cursor, or moving it to `select`.
    fn refresh(&mut self) {
        self.reload(None);
    }

    fn reload(&mut self, select: Option<PathBuf>) {
        self.load(self.cwd.clone(), select, false, false);
    }

    /// Reads a folder on its own thread. Anything on disk can hang (an unreachable NAS behind
    /// autofs blocks until it answers), and the window must keep drawing meanwhile. Only the
    /// newest request is applied, so clicking on while a slow folder loads just moves on.
    fn load(&mut self, path: PathBuf, select: Option<PathBuf>, history: bool, fresh: bool) {
        self.load_gen += 1;
        let generation = self.load_gen;
        if fresh {
            self.loading = Some(path.clone());
        }
        let (tx, ctx, sort, ascending) = (
            self.load_tx.clone(),
            self.ctx.clone(),
            self.sort,
            self.ascending,
        );
        std::thread::spawn(move || {
            let result = files::list(&path).map(|mut entries| {
                files::sort(&mut entries, sort, ascending);
                entries
            });
            let (mtime, free) = (mtime(&path), files::free_space(&path));
            let _ = tx.send(Loaded::Dir(Load {
                generation,
                path,
                select,
                history,
                fresh,
                result,
                mtime,
                free,
            }));
            ctx.request_repaint();
        });
    }

    fn loaded(&mut self, load: Load) {
        if load.generation != self.load_gen {
            return;
        }
        self.loading = None;
        let entries = match load.result {
            Ok(entries) => entries,
            // Asked to open a file: show its folder with the file selected.
            Err(e) if e.kind() == std::io::ErrorKind::NotADirectory && load.fresh => {
                return match load.path.parent() {
                    Some(parent) => {
                        self.go(parent.to_path_buf(), Some(load.path.clone()), load.history)
                    }
                    None => self.error(format!("{}: {e}", load.path.display())),
                };
            }
            // The folder went away (unmounted, deleted elsewhere): fall back to its parent.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !load.history => {
                return match load.path.parent() {
                    Some(parent) => self.go(parent.to_path_buf(), None, false),
                    None => self.error(format!("{}: {e}", load.path.display())),
                };
            }
            Err(e) => return self.error(format!("{}: {e}", load.path.display())),
        };
        self.dir_mtime = load.mtime;
        self.free = load.free;
        if load.fresh {
            self.thumbs.clear();
        }
        if !load.fresh {
            let cursor = self.cursor_entry().map(|e| e.path.clone());
            self.entries = entries;
            self.rebuild_view();
            self.cursor = cursor
                .and_then(|c| self.index_of(&c))
                .or(self.cursor.filter(|&c| c < self.view.len()));
            if let Some(path) = load.select {
                self.select_path(&path);
            }
            return;
        }
        let path = load.path;
        let select = load.select;
        if load.history && path != self.cwd {
            self.back.push(std::mem::replace(&mut self.cwd, path));
            self.forward.clear();
        } else {
            self.cwd = path;
        }
        self.entries = entries;
        self.selected.clear();
        self.filter.clear();
        self.path_edit = None;
        self.status = None;
        self.rebuild_view();
        self.cursor = None;
        match select {
            Some(p) => self.select_path(&p),
            None => self.cursor = (!self.view.is_empty()).then_some(0),
        }
        self.viewport.0 = 0.0;
        self.scroll_to_cursor = true;
        let name = self
            .cwd
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "/".into());
        self.ctx
            .send_viewport_cmd(egui::ViewportCommand::Title(format!("{name} — Ferry")));
    }

    fn folder_title(&self) {
        let name = self
            .cwd
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "/".into());
        self.ctx
            .send_viewport_cmd(egui::ViewportCommand::Title(format!("{name} — Ferry")));
    }

    fn rebuild_view(&mut self) {
        let needle = self.filter.to_lowercase();
        self.view = (0..self.entries.len())
            .filter(|&i| {
                let e = &self.entries[i];
                (self.show_hidden || !e.hidden())
                    && (needle.is_empty() || e.name.to_lowercase().contains(&needle))
            })
            .collect();
        let visible: HashSet<&PathBuf> = self.view.iter().map(|&i| &self.entries[i].path).collect();
        self.selected.retain(|p| visible.contains(p));
        if self.cursor.is_some_and(|c| c >= self.view.len()) {
            self.cursor = self.view.len().checked_sub(1);
        }
    }

    fn resort(&mut self, key: SortKey) {
        self.ascending = if self.sort == key {
            !self.ascending
        } else {
            true
        };
        self.sort = key;
        self.refresh();
    }

    // ----- Selection -----------------------------------------------------------------------

    fn entry(&self, i: usize) -> &Entry {
        &self.entries[self.view[i]]
    }

    fn cursor_entry(&self) -> Option<&Entry> {
        self.cursor
            .filter(|&c| c < self.view.len())
            .map(|c| self.entry(c))
    }

    fn index_of(&self, path: &Path) -> Option<usize> {
        self.view.iter().position(|&i| self.entries[i].path == path)
    }

    fn select_path(&mut self, path: &Path) {
        if let Some(i) = self.index_of(path) {
            self.click(i, Modifiers::NONE);
            self.scroll_to_cursor = true;
        }
    }

    fn click(&mut self, i: usize, mods: Modifiers) {
        let path = self.entry(i).path.clone();
        if mods.shift {
            let a = self.anchor.unwrap_or(i);
            if !mods.command {
                self.selected.clear();
            }
            for j in a.min(i)..=a.max(i) {
                self.selected.insert(self.entry(j).path.clone());
            }
        } else if mods.command {
            if !self.selected.remove(&path) {
                self.selected.insert(path);
            }
            self.anchor = Some(i);
        } else {
            self.selected = HashSet::from([path]);
            self.anchor = Some(i);
        }
        self.cursor = Some(i);
    }

    /// The selection in view order, or the file under the cursor when nothing is selected.
    fn targets(&self) -> Vec<PathBuf> {
        let chosen: Vec<PathBuf> = self
            .view
            .iter()
            .map(|&i| &self.entries[i].path)
            .filter(|p| self.selected.contains(*p))
            .cloned()
            .collect();
        if chosen.is_empty() {
            self.cursor_entry()
                .map(|e| vec![e.path.clone()])
                .unwrap_or_default()
        } else {
            chosen
        }
    }

    fn move_cursor(&mut self, delta: isize, mods: Modifiers) {
        if self.view.is_empty() {
            return;
        }
        let last = self.view.len() as isize - 1;
        let next = match self.cursor {
            Some(c) => (c as isize + delta).clamp(0, last),
            None => 0,
        } as usize;
        self.click(
            next,
            Modifiers {
                command: false,
                ..mods
            },
        );
        self.scroll_to_cursor = true;
    }

    /// Typing a name jumps to it, as in Dolphin; a pause of a second starts a new name.
    fn jump_to(&mut self, text: &str, now: f64) {
        if now - self.jump.1 > 1.0 {
            self.jump.0.clear();
        }
        self.jump.0.push_str(&text.to_lowercase());
        self.jump.1 = now;
        if let Some(i) = (0..self.view.len())
            .find(|&i| self.entry(i).name.to_lowercase().starts_with(&self.jump.0))
        {
            self.click(i, Modifiers::NONE);
            self.scroll_to_cursor = true;
        }
    }

    // ----- Actions -------------------------------------------------------------------------

    fn error(&mut self, message: String) {
        self.status = Some((message, true));
    }

    fn job(
        &mut self,
        label: String,
        work: impl FnOnce() -> Result<String, String> + Send + 'static,
    ) {
        self.busy += 1;
        self.status = Some((label, false));
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Done {
                job: true,
                result: work(),
            });
            ctx.request_repaint();
        });
    }

    /// Starts a program without blocking, and reports in the window if it fails.
    fn launch(&mut self, mut command: Command, what: String) {
        match command.spawn() {
            Err(e) => self.error(format!("{what}: {e}")),
            Ok(mut child) => {
                let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
                std::thread::spawn(move || {
                    if child.wait().is_ok_and(|s| !s.success()) {
                        let message = tr("Couldn't open %1").replace("%1", &what);
                        let _ = tx.send(Done {
                            job: false,
                            result: Err(message),
                        });
                        ctx.request_repaint();
                    }
                });
            }
        }
    }

    /// Whether a path in the shown folder is a folder, from the listing rather than the disk.
    fn is_folder(&self, path: &Path) -> bool {
        self.entries.iter().any(|e| e.is_dir && e.path == path)
    }

    fn open(&mut self, path: PathBuf) {
        if self.is_folder(&path) {
            self.go(path, None, true);
        } else {
            self.open_file(path);
        }
    }

    /// Text in the built-in editor, everything else in its default app.
    fn open_file(&mut self, path: PathBuf) {
        if crate::editor::wants(self.thumbs.mime_for(&path)) {
            self.open_editor(path, false);
        } else {
            self.open_external(path);
        }
    }

    fn open_external(&mut self, path: PathBuf) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut command = Command::new("xdg-open");
        command.arg(&path);
        self.launch(command, name);
    }

    /// Opens a file in the built-in editor, reading it on a background thread.
    fn open_editor(&mut self, path: PathBuf, forced: bool) {
        if self.editor.as_ref().is_some_and(|e| e.dirty()) {
            return self.error(tr("Save or close the file being edited first"));
        }
        self.editor = Some(crate::editor::Editor::loading(path.clone()));
        let (tx, ctx) = (self.load_tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let result = crate::editor::read(&path);
            let _ = tx.send(Loaded::Text {
                path,
                forced,
                result,
            });
            ctx.request_repaint();
        });
    }

    fn text_loaded(
        &mut self,
        path: PathBuf,
        forced: bool,
        result: std::io::Result<(crate::editor::Opened, Option<SystemTime>)>,
    ) {
        use crate::editor::Opened;
        let Some(editor) = self.editor.as_mut().filter(|e| e.path == path && e.loading) else {
            return;
        };
        match result {
            Ok((Opened::Text(text), mtime)) => {
                editor.loaded(text, mtime, false);
                self.focus_editor = true;
                self.status = None;
            }
            Ok((Opened::ReadOnly(text), mtime)) => {
                editor.loaded(text, mtime, true);
                self.status = Some((tr("Not UTF-8 text, so it opens read-only"), false));
            }
            Ok((opened, _)) => {
                let message = crate::editor::refusal(&opened).unwrap_or_default();
                self.editor = None;
                if forced {
                    self.error(tr("Can't open this file as text"));
                } else {
                    self.status = Some((message, false));
                    self.open_external(path);
                }
                return;
            }
            Err(e) => {
                self.editor = None;
                return self.error(format!("{}: {e}", path.display()));
            }
        }
        let title = format!(
            "{} — Ferry",
            self.editor.as_ref().map(|e| e.name()).unwrap_or_default()
        );
        self.ctx
            .send_viewport_cmd(egui::ViewportCommand::Title(title));
    }

    fn save_editor(&mut self, force: bool) {
        let Some(editor) = self
            .editor
            .as_mut()
            .filter(|e| !e.read_only && !e.loading && !e.saving)
        else {
            return;
        };
        editor.saving = true;
        let (path, text, opened_at) = (editor.path.clone(), editor.text.clone(), editor.mtime);
        let (tx, ctx) = (self.load_tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let result = crate::editor::save(&path, &text, opened_at, force);
            let _ = tx.send(Loaded::Saved { path, text, result });
            ctx.request_repaint();
        });
    }

    fn text_saved(
        &mut self,
        path: PathBuf,
        text: String,
        result: Result<Option<SystemTime>, crate::editor::SaveError>,
    ) {
        let Some(editor) = self.editor.as_mut().filter(|e| e.path == path) else {
            return;
        };
        match result {
            Ok(mtime) => {
                editor.saved(text, mtime);
                self.status = Some((tr("Saved %1").replace("%1", &editor.name()), false));
                match self.after_save.take() {
                    Some(AfterSave::Close) => self.close_editor(),
                    Some(AfterSave::Quit) => self.quit(),
                    None => {}
                }
            }
            Err(crate::editor::SaveError::ChangedOnDisk) => {
                editor.saving = false;
                self.dialog = Some(Dialog::ChangedOnDisk);
            }
            Err(crate::editor::SaveError::Failed(message)) => {
                editor.saving = false;
                self.after_save = None;
                self.error(message);
            }
        }
    }

    /// Back to the folder; asks first when there are unsaved changes.
    fn request_close_editor(&mut self) {
        match &self.editor {
            Some(e) if e.dirty() => self.dialog = Some(Dialog::Unsaved { quit: false }),
            Some(_) => self.close_editor(),
            None => {}
        }
    }

    fn close_editor(&mut self) {
        if let Some(editor) = self.editor.take() {
            self.folder_title();
            // Select the file just edited, with its new size and date.
            self.reload(Some(editor.path));
        }
    }

    fn quit(&mut self) {
        self.allow_close = true;
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// The editor in place of the folder: a header with the file's name and actions, a find
    /// bar, and the text with line numbers, highlighted, or Markdown rendered.
    fn editor_view(&mut self, ui: &mut Ui) {
        let p = self.p();
        let z = self.zoom_editor;
        if self
            .code_theme
            .as_ref()
            .is_none_or(|(made_from, _)| *made_from != p)
        {
            self.code_theme = Some((p, crate::highlight::theme(&p)));
        }
        let radius = self.theme.radius().min(6);
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        let Some((_, code_theme)) = self.code_theme.as_ref() else {
            return;
        };
        let (dirty, read_only, saving, loading) = (
            editor.dirty(),
            editor.read_only,
            editor.saving,
            editor.loading,
        );
        let markdown = crate::markdown::is_markdown(&editor.path);
        let path = editor.path.clone();
        let mut action = None;
        ui.horizontal(|ui| {
            if ui
                .add(egui::Button::new(RichText::new("\u{f053}").size(14.0)).frame(false))
                .on_hover_text(tr("Back to the folder (Esc)"))
                .clicked()
            {
                action = Some(Action::CloseEditor);
            }
            ui.label(RichText::new(editor.name()).size(15.0).color(p.bright));
            if dirty {
                ui.label(RichText::new("\u{25cf}").color(p.accent))
                    .on_hover_text(tr("Unsaved changes"));
            }
            if read_only && !loading {
                ui.label(RichText::new(tr("Read-only")).size(12.0).color(p.yellow));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let fill = if dirty { p.accent } else { p.surface };
                let save =
                    egui::Button::new(RichText::new(tr("Save")).color(crate::theme::on(fill)))
                        .fill(fill);
                if ui
                    .add_enabled(dirty && !saving, save)
                    .on_hover_text("Ctrl+S")
                    .clicked()
                {
                    action = Some(Action::SaveEditor(false));
                }
                if saving {
                    ui.spinner();
                }
                if ui.button(tr("Open With Default App")).clicked() {
                    action = Some(Action::OpenExternal(path.clone()));
                }
                if markdown && !loading {
                    let label = if editor.preview {
                        tr("Edit")
                    } else {
                        tr("Preview")
                    };
                    if ui.button(label).on_hover_text("Ctrl+E").clicked() {
                        editor.preview = !editor.preview;
                    }
                }
                if !editor.preview
                    && !loading
                    && icon_button(ui, '\u{f002}', true, &tr("Find (Ctrl+F)")).clicked()
                {
                    editor.find.get_or_insert_with(String::new);
                    editor.focus_find = true;
                }
            });
        });
        ui.add_space(4.0);
        if loading {
            ui.centered_and_justified(|ui| ui.spinner());
            self.pending.extend(action);
            return;
        }
        if editor.preview {
            egui::Frame::new()
                .fill(p.sidebar)
                .corner_radius(CornerRadius::same(radius))
                .inner_margin(Margin::symmetric(20, 14))
                .show(ui, |ui| {
                    ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                        // A readable line length, like a page.
                        ui.set_max_width(ui.available_width().min(860.0 * z));
                        crate::markdown::show(ui, &editor.text, &p, code_theme, (15.0 * z).round());
                    });
                });
            self.pending.extend(action);
            return;
        }

        // The find bar.
        if editor.find.is_some() {
            let (mut changed, mut step, mut close) = (false, None, false);
            let (count, current) = (editor.found.len(), editor.current);
            let focus_find = std::mem::take(&mut editor.focus_find);
            if let Some(find) = editor.find.as_mut() {
                ui.horizontal(|ui| {
                    let edit = ui.add(
                        TextEdit::singleline(find)
                            .id(egui::Id::unique("ferry-find"))
                            .hint_text(format!("\u{f002}  {}", tr("Find in this file")))
                            .desired_width(280.0)
                            .margin(Margin::symmetric(8, 4)),
                    );
                    if focus_find {
                        edit.request_focus();
                        // Select what's there, so typing starts a new search.
                        let mut state = TextEdit::load_state(ui.ctx(), edit.id).unwrap_or_default();
                        state
                            .cursor
                            .set_char_range(Some(egui::text::CCursorRange::two(
                                egui::text::CCursor::new(0),
                                egui::text::CCursor::new(find.chars().count()),
                            )));
                        state.store(ui.ctx(), edit.id);
                    }
                    changed = edit.changed();
                    if edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        step = Some(!ui.input(|i| i.modifiers.shift));
                        edit.request_focus();
                    }
                    let summary = match (count, current) {
                        (0, _) if find.is_empty() => String::new(),
                        (0, _) => tr("No matches"),
                        (n, Some(c)) => tr("%1 of %2")
                            .replace("%1", &(c + 1).to_string())
                            .replace("%2", &n.to_string()),
                        (n, None) => trn("%n matches", n),
                    };
                    let color = if count == 0 && !find.is_empty() {
                        p.red
                    } else {
                        p.dim
                    };
                    ui.label(RichText::new(summary).color(color));
                    if icon_button(ui, '\u{f077}', count > 0, &tr("Previous (Shift+Enter)"))
                        .clicked()
                    {
                        step = Some(false);
                    }
                    if icon_button(ui, '\u{f078}', count > 0, &tr("Next (Enter)")).clicked() {
                        step = Some(true);
                    }
                    if icon_button(ui, '\u{f00d}', true, &tr("Close (Esc)")).clicked() {
                        close = true;
                    }
                });
            }
            if changed {
                editor.refind();
                editor.current = None;
                editor.step(true);
            }
            if let Some(forward) = step {
                editor.step(forward);
            }
            if close {
                editor.find = None;
                editor.refind();
            }
            ui.add_space(4.0);
        }

        let size = (14.0 * z).round();
        let font = FontId::monospace(size);
        let numbers = editor.line_numbers();
        let focus = std::mem::take(&mut self.focus_editor);
        let (found, current) = (editor.found.clone(), editor.current);
        let select = editor.select.take();
        let id = egui::Id::unique(("ferry-editor", &path));
        // Highlighting runs only when the text, the matches or the look changed.
        let mut cache = self.editor_layout.take();
        let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, _wrap: f32| {
            use std::hash::{Hash, Hasher};
            let text = buf.as_str();
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (text, &path, &found, current, size.to_bits(), p.fg, p.accent).hash(&mut h);
            let key = h.finish();
            let job = match &cache {
                Some((k, job)) if *k == key => job.clone(),
                _ => {
                    let job = crate::highlight::layout(
                        text, &path, code_theme, &p, &font, &found, current,
                    );
                    cache = Some((key, job.clone()));
                    job
                }
            };
            ui.ctx().fonts_mut(|f| f.layout_job(job))
        };
        egui::Frame::new()
            .fill(p.sidebar)
            .corner_radius(CornerRadius::same(radius))
            .inner_margin(Margin::symmetric(6, 6))
            .show(ui, |ui| {
                ScrollArea::both().auto_shrink(false).show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        ui.label(RichText::new(numbers).font(font.clone()).color(p.dim));
                        ui.add_space(6.0);
                        let size = ui.available_size();
                        let output = if read_only {
                            // A &str buffer: selectable and copyable, but not editable.
                            let mut text: &str = &editor.text;
                            TextEdit::multiline(&mut text)
                                .id(id)
                                .frame(egui::Frame::NONE)
                                .margin(Margin::ZERO)
                                .desired_width(f32::INFINITY)
                                .min_size(size)
                                .layouter(&mut layouter)
                                .show(ui)
                        } else {
                            TextEdit::multiline(&mut editor.text)
                                .id(id)
                                .code_editor()
                                .frame(egui::Frame::NONE)
                                .margin(Margin::ZERO)
                                .lock_focus(true)
                                .desired_width(f32::INFINITY)
                                .min_size(size)
                                .layouter(&mut layouter)
                                .show(ui)
                        };
                        if focus {
                            output.response.request_focus();
                        }
                        if output.response.changed() && editor.find.is_some() {
                            editor.refind();
                        }
                        // Select a match (or a line) and bring it into view.
                        if let Some(range) = select {
                            let chars = |b: usize| {
                                egui::text::CCursor::new(
                                    editor.text.get(..b).map_or(0, |t| t.chars().count()),
                                )
                            };
                            let (a, b) = (chars(range.start), chars(range.end));
                            let mut state = output.state;
                            // The cursor goes to the match without selecting it: a selection would paint over the highlight.
                            state
                                .cursor
                                .set_char_range(Some(egui::text::CCursorRange::one(a)));
                            state.store(ui.ctx(), output.response.id);
                            let rect = output
                                .galley
                                .pos_from_cursor(b)
                                .translate(output.galley_pos.to_vec2());
                            ui.scroll_to_rect(rect.expand2(vec2(60.0, 0.0)), Some(Align::Center));
                        }
                    });
                });
            });
        self.editor_layout = cache;
        self.pending.extend(action);
    }

    /// Keys while the editor is open: everything else belongs to the text.
    fn editor_key(&mut self, key: Key, m: Modifiers, ctx: &egui::Context) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        match key {
            Key::S if m.command => self.pending.push(Action::SaveEditor(false)),
            Key::W if m.command => self.pending.push(Action::CloseEditor),
            Key::Escape if editor.find.is_some() => {
                editor.find = None;
                editor.refind();
                self.focus_editor = true;
            }
            Key::Escape => self.pending.push(Action::CloseEditor),
            Key::F if m.command && !editor.preview && !editor.loading => {
                editor.find.get_or_insert_with(String::new);
                editor.focus_find = true;
            }
            Key::F3 => editor.step(!m.shift),
            Key::E if m.command && crate::markdown::is_markdown(&editor.path) => {
                editor.preview = !editor.preview;
            }
            Key::Q if m.command => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Key::Plus | Key::Equals if m.command => self.zoom_by(1.25),
            Key::Minus if m.command => self.zoom_by(0.8),
            Key::Num0 if m.command => self.zoom_by(0.0),
            _ => {}
        }
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::Go(p) => self.go(p, None, true),
            Action::Back => {
                if let Some(p) = self.back.pop() {
                    let from = self.cwd.clone();
                    self.forward.push(from.clone());
                    self.go(p, Some(from), false);
                }
            }
            Action::Forward => {
                if let Some(p) = self.forward.pop() {
                    self.back.push(self.cwd.clone());
                    self.go(p, None, false);
                }
            }
            Action::Up => {
                if let Some(parent) = self.cwd.parent().map(PathBuf::from) {
                    let from = self.cwd.clone();
                    self.go(parent, Some(from), true);
                }
            }
            Action::Open => {
                let targets = self.targets();
                // Several folders can't all be shown at once, so open the first; files all open.
                if let Some(dir) = targets.iter().find(|p| self.is_folder(p)) {
                    self.open(dir.clone());
                } else {
                    for p in targets {
                        self.open(p);
                    }
                }
            }
            Action::Trash => {
                let paths = self.targets();
                if paths.is_empty() {
                    return;
                }
                let n = paths.len();
                self.job(tr("Moving to the trash…"), move || {
                    files::trash(&paths).map(|_| trn("Moved %n items to the trash", n))
                });
            }
            Action::AskDelete => {
                let paths = self.targets();
                if !paths.is_empty() {
                    self.dialog = Some(Dialog::Delete { paths });
                }
            }
            Action::AskRename => {
                if let [path] = self.targets().as_slice() {
                    let path = path.clone();
                    let text = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    self.dialog = Some(Dialog::Rename {
                        path,
                        text,
                        error: None,
                    });
                }
            }
            Action::AskNewFolder => {
                self.dialog = Some(Dialog::NewFolder {
                    text: tr("New Folder"),
                    error: None,
                })
            }
            Action::Copy | Action::Cut => {
                let cut = matches!(action, Action::Cut);
                let paths = self.targets();
                if paths.is_empty() {
                    return;
                }
                // The paths also go on the system clipboard as text, so they paste into a terminal too.
                let text = paths
                    .iter()
                    .map(|p| p.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("\n");
                self.ctx.copy_text(text.clone());
                let n = paths.len();
                self.status = Some((
                    if cut {
                        trn("Cut %n items", n)
                    } else {
                        trn("Copied %n items", n)
                    },
                    false,
                ));
                self.clip = Some(Clip { paths, cut, text });
            }
            Action::Paste => {
                if let Some(clip) = &self.clip {
                    let (paths, cut) = (clip.paths.clone(), clip.cut);
                    self.paste(paths, cut);
                }
            }
            Action::PasteText(text) => {
                // shortcut: file:// URIs are taken literally, without percent-decoding; decode them if
                // pasting from browsers or other file managers turns out to matter.
                let paths: Vec<PathBuf> = text
                    .lines()
                    .map(|l| PathBuf::from(l.trim().strip_prefix("file://").unwrap_or(l.trim())))
                    .filter(|p| p.is_absolute() && p.symlink_metadata().is_ok())
                    .collect();
                if paths.is_empty() {
                    return self.error(tr("The clipboard holds no files"));
                }
                let cut = self.clip.as_ref().is_some_and(|c| c.cut && c.text == text);
                self.paste(paths, cut);
            }
            Action::CopyPath => {
                let text = self
                    .targets()
                    .iter()
                    .map(|p| p.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("\n");
                self.ctx.copy_text(if text.is_empty() {
                    self.cwd.to_string_lossy().into_owned()
                } else {
                    text
                });
            }
            Action::Terminal => {
                let mut command = Command::new("xdg-terminal-exec");
                command.arg(format!("--dir={}", self.cwd.display()));
                self.launch(command, tr("Terminal"));
            }
            Action::Refresh => {
                self.places = files::places();
                self.refresh();
            }
            Action::ToggleHidden => {
                self.show_hidden = !self.show_hidden;
                self.rebuild_view();
            }
            Action::SelectAll => {
                self.selected = self
                    .view
                    .iter()
                    .map(|&i| self.entries[i].path.clone())
                    .collect();
            }
            Action::AddPlaces(paths) => {
                let folders: Vec<PathBuf> =
                    paths.into_iter().filter(|p| !self.in_places(p)).collect();
                if folders.is_empty() {
                    return self.status = Some((tr("Already in Bookmarks"), false));
                }
                match files::update_bookmarks(&self.bookmarks_file, &folders, None) {
                    Ok(n) => self.status = Some((trn("Added %n folders to Bookmarks", n), false)),
                    Err(e) => self.error(format!("{}: {e}", self.bookmarks_file.display())),
                }
                self.bookmarks = files::bookmarks(&self.bookmarks_file);
            }
            Action::EditText(path) => self.open_editor(path, true),
            Action::AskNewFile => {
                self.dialog = Some(Dialog::NewFile {
                    text: tr("untitled.txt"),
                    error: None,
                })
            }
            Action::Search => self.open_search(),
            Action::CloseSearch => self.close_search(),
            Action::OpenFound { path, is_dir, line } => {
                if is_dir {
                    self.go(path, None, true);
                } else if let Some(line) = line {
                    let query = self.search.as_ref().map(|s| s.text.trim().to_string());
                    self.open_editor(path.clone(), true);
                    if let Some(editor) =
                        self.editor.as_mut().filter(|e| e.loading && e.path == path)
                    {
                        editor.line = Some(line);
                        editor.find = query;
                    }
                } else {
                    // The results stay, so closing the file comes back to them.
                    self.open_file(path);
                }
            }
            Action::OpenDevice(d) => match (&d.mount, &d.block) {
                (Some(path), _) => self.go(path.clone(), None, true),
                (None, Some(block)) if !d.fstype.is_empty() => {
                    self.busy += 1;
                    self.status = Some((tr("Mounting %1…").replace("%1", &d.name), false));
                    let (tx, ctx, block) = (self.load_tx.clone(), self.ctx.clone(), block.clone());
                    std::thread::spawn(move || {
                        let _ = tx.send(Loaded::Mounted(devices::mount(&block)));
                        ctx.request_repaint();
                    });
                }
                _ => self.error(
                    tr("%1 has no filesystem yet. Format it to use it.").replace("%1", &d.name),
                ),
            },
            Action::Unmount(d) => {
                self.leave(&d);
                let (name, block) = (d.name.clone(), d.block.clone().unwrap_or_default());
                self.job(tr("Unmounting %1…").replace("%1", &name), move || {
                    devices::unmount(&block).map(|_| tr("Unmounted %1").replace("%1", &name))
                });
            }
            Action::Eject(d) => {
                self.leave(&d);
                let name = d.name.clone();
                self.job(tr("Ejecting %1…").replace("%1", &name), move || {
                    devices::eject(&d).map(|_| tr("You can unplug %1 now").replace("%1", &name))
                });
            }
            Action::AskFormat(d) => {
                if devices::formats().is_empty() {
                    self.error(tr(
                        "No formatting tools are installed (exfatprogs, dosfstools or e2fsprogs).",
                    ));
                } else {
                    let label = if d.name.contains(' ') {
                        String::new()
                    } else {
                        d.name.clone()
                    };
                    self.dialog = Some(Dialog::Format {
                        device: d,
                        fs: 0,
                        label,
                    });
                }
            }
            Action::Reveal(path) => {
                if let Some(parent) = path.parent().map(PathBuf::from) {
                    self.go(parent, Some(path), true);
                }
            }
            Action::OpenExternal(path) => self.open_external(path),
            Action::CloseEditor => self.request_close_editor(),
            Action::SaveEditor(force) => self.save_editor(force),
            Action::RemovePlace(path) => {
                if let Err(e) = files::update_bookmarks(&self.bookmarks_file, &[], Some(&path)) {
                    self.error(format!("{}: {e}", self.bookmarks_file.display()));
                }
                self.bookmarks = files::bookmarks(&self.bookmarks_file);
            }
        }
    }

    /// The selected folders, or the folder being shown when none is selected.
    fn place_targets(&self) -> Vec<PathBuf> {
        // Only an explicit selection counts: the cursor alone means "this folder".
        let folders: Vec<PathBuf> = self
            .view
            .iter()
            .map(|&i| &self.entries[i])
            .filter(|e| e.is_dir && self.selected.contains(&e.path))
            .map(|e| e.path.clone())
            .collect();
        if folders.is_empty() {
            vec![self.cwd.clone()]
        } else {
            folders
        }
    }

    /// A label under the pointer naming what is being dragged.
    fn drag_preview(&self, ctx: &egui::Context, p: &Palette) {
        let Some(dragged) = egui::DragAndDrop::payload::<Dragged>(ctx) else {
            return;
        };
        let Some(pointer) = ctx.pointer_latest_pos() else {
            return;
        };
        let text = match dragged.paths.as_slice() {
            [one] => one
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            many => trn("%n items", many.len()),
        };
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::unique("ferry-drag"),
        ));
        let galley = painter.layout_no_wrap(text, FontId::proportional(13.0), p.on_accent);
        let rect = Rect::from_min_size(pointer + vec2(14.0, 10.0), galley.size() + vec2(16.0, 8.0));
        painter.rect_filled(
            rect,
            CornerRadius::same(self.theme.radius().min(4)),
            p.accent,
        );
        painter.galley(rect.min + vec2(8.0, 4.0), galley, p.on_accent);
    }

    fn in_places(&self, path: &Path) -> bool {
        self.places.iter().any(|(_, p, _)| p == path)
            || self.bookmarks.iter().any(|(_, p)| p == path)
    }

    fn paste(&mut self, paths: Vec<PathBuf>, cut: bool) {
        let dir = self.cwd.clone();
        if cut {
            self.clip = None;
        }
        let label = if cut {
            tr("Moving…")
        } else {
            tr("Copying…")
        };
        self.job(label, move || {
            files::paste(&paths, &dir, cut).map(|n| trn("Pasted %n items", n))
        });
    }

    // ----- Input ---------------------------------------------------------------------------

    fn keys(&mut self, ui: &Ui) {
        let ctx = ui.ctx().clone();
        let (events, now) = ctx.input(|i| (i.events.clone(), i.time));
        let typing = ctx.egui_wants_keyboard_input();
        let zoom = ctx.input(|i| i.zoom_delta());
        if zoom != 1.0 {
            self.zoom_by(zoom);
        }
        for event in events {
            match event {
                Event::Key {
                    key,
                    pressed: true,
                    modifiers: m,
                    ..
                } => {
                    if self.editor.is_some() {
                        self.editor_key(key, m, &ctx);
                        continue;
                    }
                    if self.search.is_some() {
                        self.search_key(key, m, &ctx);
                        continue;
                    }
                    // These work even while a text field has focus.
                    match key {
                        Key::Q if m.command => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                        Key::L if m.command => {
                            self.path_edit = Some(self.cwd.to_string_lossy().into_owned())
                        }
                        Key::F if m.command && m.shift => self.pending.push(Action::Search),
                        Key::F | Key::I if m.command => self.focus_filter = true,
                        _ => {}
                    }
                    if !typing {
                        self.key(key, m, now);
                    }
                }
                _ if self.editor.is_some() || self.search.is_some() => {}
                Event::Text(t) if !typing => {
                    let fresh = self.jump.0.is_empty() || now - self.jump.1 > 1.0;
                    match t.as_str() {
                        "?" if fresh => self.dialog = Some(Dialog::Help),
                        " " if fresh => {} // the Space key itself opens
                        _ => self.jump_to(&t, now),
                    }
                }
                Event::Copy if !typing => self.pending.push(Action::Copy),
                Event::Cut if !typing => self.pending.push(Action::Cut),
                Event::Paste(text) if !typing => self.pending.push(Action::PasteText(text)),
                _ => {}
            }
        }
    }

    fn key(&mut self, key: Key, m: Modifiers, now: f64) {
        let cols = self.columns() as isize;
        let vertical = if self.grid { cols } else { 1 };
        let page = ((self.viewport.1
            / if self.grid {
                self.tile_size().y
            } else {
                self.row_height()
            }) as isize)
            .max(1)
            * vertical;
        let action = match key {
            Key::ArrowLeft if m.alt => Some(Action::Back),
            Key::ArrowRight if m.alt => Some(Action::Forward),
            Key::ArrowUp if m.alt => Some(Action::Up),
            Key::Home if m.alt => Some(Action::Go(files::home())),
            Key::ArrowDown => return self.move_cursor(vertical, m),
            Key::ArrowUp => return self.move_cursor(-vertical, m),
            Key::ArrowRight if self.grid => return self.move_cursor(1, m),
            Key::ArrowLeft if self.grid => return self.move_cursor(-1, m),
            Key::PageDown => return self.move_cursor(page, m),
            Key::PageUp => return self.move_cursor(-page, m),
            Key::Home => return self.move_cursor(isize::MIN / 2, m),
            Key::End => return self.move_cursor(isize::MAX / 2, m),
            Key::Enter => Some(Action::Open),
            // Space opens, unless it is part of a name being typed to jump to it.
            Key::Space if self.jump.0.is_empty() || now - self.jump.1 > 1.0 => Some(Action::Open),
            Key::Backspace => Some(Action::Back),
            Key::F1 => {
                self.dialog = Some(Dialog::Help);
                None
            }
            Key::F2 => Some(Action::AskRename),
            Key::F4 => Some(Action::Terminal),
            Key::F5 => Some(Action::Refresh),
            Key::Delete if m.shift => Some(Action::AskDelete),
            Key::Delete => Some(Action::Trash),
            Key::A if m.command => Some(Action::SelectAll),
            Key::H if m.command => Some(Action::ToggleHidden),
            Key::D if m.command => Some(Action::AddPlaces(self.place_targets())),
            Key::N if m.command && m.alt => Some(Action::AskNewFile),
            Key::N if m.command && m.shift => Some(Action::AskNewFolder),
            Key::C if m.command && m.alt => Some(Action::CopyPath),
            Key::Plus | Key::Equals if m.command => {
                self.zoom_by(1.25);
                None
            }
            Key::Minus if m.command => {
                self.zoom_by(0.8);
                None
            }
            Key::Num0 if m.command => {
                self.zoom_by(0.0);
                None
            }
            Key::Num1 if m.command => {
                self.grid = false;
                None
            }
            Key::Num2 if m.command => {
                self.grid = true;
                None
            }
            Key::Escape => {
                if !self.filter.is_empty() {
                    self.filter.clear();
                    self.rebuild_view();
                } else {
                    self.selected.clear();
                }
                self.jump.0.clear();
                None
            }
            _ => None,
        };
        self.pending.extend(action);
    }

    fn columns(&self) -> usize {
        ((self.viewport_width() / self.tile_size().x) as usize).max(1)
    }

    fn zoom(&self) -> f32 {
        if self.editor.is_some() {
            self.zoom_editor
        } else if self.grid {
            self.zoom_grid
        } else {
            self.zoom_list
        }
    }

    /// Multiplies the current view's zoom, keeping the cursor in sight.
    fn zoom_by(&mut self, factor: f32) {
        let (zoom, max) = if self.editor.is_some() {
            (&mut self.zoom_editor, 3.0)
        } else if self.grid {
            // Icons go to 4x, where they read like Dolphin's largest; details rows stop at 2.5x.
            (&mut self.zoom_grid, 4.0)
        } else {
            (&mut self.zoom_list, 2.5)
        };
        *zoom = if factor == 0.0 {
            1.0
        } else {
            (*zoom * factor).clamp(0.6, max)
        };
        self.scroll_to_cursor = true;
    }

    fn row_height(&self) -> f32 {
        (ROW * self.zoom_list).round()
    }

    /// Icons grow with the zoom; their names grow more slowly, so big icons don't get huge text.
    fn name_size(&self) -> f32 {
        13.0 * (1.0 + (self.zoom_grid - 1.0) * 0.35)
    }

    fn tile_size(&self) -> Vec2 {
        let z = self.zoom_grid;
        vec2(
            (TILE_WIDTH * z).round(),
            (56.0 * z + 2.6 * self.name_size() + 10.0).round(),
        )
    }

    fn viewport_width(&self) -> f32 {
        self.ctx
            .memory(|m| m.data.get_temp::<f32>(egui::Id::unique("ferry-width")))
            .unwrap_or(800.0)
    }

    // ----- Drawing -------------------------------------------------------------------------

    fn toolbar(&mut self, ui: &mut Ui) {
        let p = self.p();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if icon_button(
                ui,
                '\u{f053}',
                !self.back.is_empty(),
                &tr("Back (Alt+Left)"),
            )
            .clicked()
            {
                self.pending.push(Action::Back);
            }
            if icon_button(
                ui,
                '\u{f054}',
                !self.forward.is_empty(),
                &tr("Forward (Alt+Right)"),
            )
            .clicked()
            {
                self.pending.push(Action::Forward);
            }
            if icon_button(
                ui,
                '\u{f062}',
                self.cwd.parent().is_some(),
                &tr("Up (Alt+Up)"),
            )
            .clicked()
            {
                self.pending.push(Action::Up);
            }
            ui.add_space(8.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                self.appearance_button(ui);
                if icon_toggle(ui, '\u{f00a}', self.grid, &tr("Icons (Ctrl+2)")).clicked() {
                    self.grid = true;
                }
                if icon_toggle(ui, '\u{f03a}', !self.grid, &tr("Details (Ctrl+1)")).clicked() {
                    self.grid = false;
                }
                let eye = if self.show_hidden {
                    '\u{f06e}'
                } else {
                    '\u{f070}'
                };
                if icon_toggle(ui, eye, self.show_hidden, &tr("Hidden files (Ctrl+H)")).clicked() {
                    self.pending.push(Action::ToggleHidden);
                }
                ui.add_space(6.0);
                if icon_toggle(
                    ui,
                    '\u{f002}',
                    self.search.is_some(),
                    &tr("Search in subfolders (Ctrl+Shift+F)"),
                )
                .clicked()
                {
                    self.pending.push(if self.search.is_some() {
                        Action::CloseSearch
                    } else {
                        Action::Search
                    });
                }
                let filter = ui.add(
                    TextEdit::singleline(&mut self.filter)
                        .hint_text(format!("\u{f0b0}  {}", tr("Filter")))
                        .desired_width(190.0)
                        .margin(Margin::symmetric(8, 5)),
                );
                if std::mem::take(&mut self.focus_filter) {
                    filter.request_focus();
                }
                if filter.changed() {
                    self.rebuild_view();
                    self.cursor = (!self.view.is_empty()).then_some(0);
                }
                if filter.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
                    self.filter.clear();
                    self.rebuild_view();
                    filter.surrender_focus();
                }
                if filter.lost_focus()
                    && ui.input(|i| i.key_pressed(Key::Enter) || i.key_pressed(Key::ArrowDown))
                {
                    filter.surrender_focus();
                    // Nothing here matches: look in the subfolders instead.
                    if self.view.is_empty() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        self.pending.push(Action::Search);
                    }
                }
                ui.add_space(8.0);
                self.path_bar(ui, &p);
            });
        });
    }

    fn path_bar(&mut self, ui: &mut Ui, p: &Palette) {
        let r = self.theme.radius();
        let frame = egui::Frame::new()
            .fill(self.glass(p.sidebar))
            .corner_radius(CornerRadius::same(r.min(6)))
            .inner_margin(Margin::symmetric(6, 2));
        let width = ui.available_width();
        ui.allocate_ui_with_layout(
            vec2(width, 30.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                frame.show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.set_height(26.0);
                    if let Some(text) = &mut self.path_edit {
                        let edit = ui.add(
                            TextEdit::singleline(text)
                                .frame(egui::Frame::NONE)
                                .desired_width(f32::INFINITY),
                        );
                        if !edit.has_focus() && !edit.lost_focus() {
                            edit.request_focus();
                        }
                        if edit.lost_focus() {
                            let entered = ui.input(|i| i.key_pressed(Key::Enter));
                            let target = PathBuf::from(shellexpand_home(text.trim()));
                            self.path_edit = None;
                            // The loader reports a missing folder, and opens a file's folder with it selected.
                            if entered {
                                self.pending.push(Action::Go(target));
                            }
                        }
                        return;
                    }
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for (i, (label, path)) in crumbs(&self.cwd).into_iter().enumerate() {
                        if i > 0 {
                            ui.label(RichText::new(" \u{f105} ").color(p.dim));
                        }
                        let current = path == self.cwd;
                        let text =
                            RichText::new(label).color(if current { p.bright } else { p.fg });
                        if ui.add(egui::Button::new(text).frame(false)).clicked() && !current {
                            self.pending.push(Action::Go(path));
                        }
                    }
                    // Clicking the empty part of the bar edits the path, like Dolphin.
                    let rest = ui.allocate_response(ui.available_size(), Sense::click());
                    if rest.clicked() {
                        self.path_edit = Some(self.cwd.to_string_lossy().into_owned());
                    }
                });
            },
        );
    }

    fn sidebar(&mut self, ui: &mut Ui) {
        let p = self.p();
        ui.add_space(10.0);
        section(ui, &tr("Places"), &p);
        for (label, path, icon) in self.places.clone() {
            self.place(ui, &label, &path, icon, &p, false);
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            section(ui, &tr("Bookmarks"), &p);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let add = egui::Button::new(RichText::new("\u{f067}").size(11.0).color(p.dim))
                    .frame(false)
                    .min_size(vec2(22.0, 18.0));
                if ui
                    .add(add)
                    .on_hover_text(tr("Bookmark the selected folders, or this one (Ctrl+D)"))
                    .clicked()
                {
                    self.pending.push(Action::AddPlaces(self.place_targets()));
                }
            });
        });
        // Bookmarks that duplicate a built-in place (GTK often lists Downloads) show once.
        let bookmarks: Vec<_> = self
            .bookmarks
            .iter()
            .filter(|(_, b)| !self.places.iter().any(|(_, p, _)| p == b))
            .cloned()
            .collect();
        for (label, path) in &bookmarks {
            self.place(ui, label, path, '\u{f02e}', &p, true);
        }

        // Dragging folders anywhere over the sidebar offers to bookmark them.
        let dragged = egui::DragAndDrop::payload::<Dragged>(ui.ctx()).map(|d| {
            d.folders
                .iter()
                .filter(|p| !self.in_places(p))
                .cloned()
                .collect::<Vec<_>>()
        });
        if let Some(folders) = &dragged {
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
            let text = if folders.is_empty() {
                tr("Only new folders can be added")
            } else {
                tr("Drop to bookmark")
            };
            let r = CornerRadius::same(self.theme.radius().min(4));
            ui.painter().rect_stroke(
                rect.shrink(1.0),
                r,
                Stroke::new(1.0, p.accent.gamma_multiply(0.7)),
                egui::StrokeKind::Inside,
            );
            ui.painter().text(
                rect.left_center() + vec2(14.0, 0.0),
                Align2::LEFT_CENTER,
                format!("\u{f067}  {text}"),
                FontId::proportional(13.0),
                p.accent,
            );
        } else if bookmarks.is_empty() {
            ui.label(
                RichText::new(tr("Drag folders here, or press +"))
                    .size(12.0)
                    .color(p.dim),
            );
        }

        let (network, drives): (Vec<Device>, Vec<Device>) = self
            .devices
            .clone()
            .into_iter()
            .partition(|d| d.kind == devices::Kind::Network);
        for (title, list) in [(tr("Devices"), drives), (tr("Network"), network)] {
            if !list.is_empty() {
                ui.add_space(12.0);
                section(ui, &title, &p);
                for device in list {
                    self.device_row(ui, &device, &p);
                }
            }
        }

        let zone = ui.interact(ui.max_rect(), ui.scope_id().with("drop"), Sense::hover());
        if dragged.is_some() && zone.contains_pointer() {
            ui.painter().rect_stroke(
                ui.max_rect(),
                0,
                Stroke::new(2.0, p.accent),
                egui::StrokeKind::Inside,
            );
        }
        if dragged.is_some() && ui.input(|i| i.pointer.any_released()) {
            debug(&format!(
                "drag released, over sidebar: {}",
                zone.contains_pointer()
            ));
        }
        if let Some(drop) = zone.dnd_release_payload::<Dragged>() {
            self.pending.push(Action::AddPlaces(drop.folders.clone()));
        }
    }

    fn place(
        &mut self,
        ui: &mut Ui,
        label: &str,
        path: &Path,
        icon: char,
        p: &Palette,
        removable: bool,
    ) {
        let (rect, resp) =
            ui.allocate_exact_size(vec2(ui.available_width(), PLACE_ROW), Sense::click());
        let current = self.cwd == path;
        paint_place(ui, rect, current, resp.hovered(), self.theme.radius(), p);
        let color = if current { p.bright } else { p.fg };
        ui.painter().text(
            rect.left_center() + vec2(14.0, 0.0),
            Align2::LEFT_CENTER,
            icon,
            FontId::proportional(15.0),
            if current { p.accent } else { p.dim },
        );
        let galley = text_galley(ui, label, 14.0, color, rect.width() - 48.0, 1, false);
        ui.painter().galley(
            rect.left_center() + vec2(40.0, -galley.size().y / 2.0),
            galley,
            color,
        );
        if resp.clicked() {
            self.pending.push(Action::Go(path.to_path_buf()));
        }
        if removable {
            resp.context_menu(|ui| {
                if ui.button(tr("Remove from Bookmarks")).clicked() {
                    self.pending.push(Action::RemovePlace(path.to_path_buf()));
                }
            });
        }
        resp.on_hover_text(path.display().to_string());
    }

    /// A drive or share: its name, a bar showing how full it is, an eject button for what can
    /// be unplugged, and a menu with the rest.
    fn device_row(&mut self, ui: &mut Ui, d: &Device, p: &Palette) {
        let bar = d.fill();
        let height = if bar.is_some() {
            PLACE_ROW + 6.0
        } else {
            PLACE_ROW
        };
        let (rect, resp) =
            ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
        let current = d.mount.as_ref().is_some_and(|m| self.cwd.starts_with(m));
        paint_place(ui, rect, current, resp.hovered(), self.theme.radius(), p);
        let mounted = d.mount.is_some();
        let color = if current {
            p.bright
        } else if mounted {
            p.fg
        } else {
            p.dim
        };
        let middle = rect.top() + PLACE_ROW / 2.0;
        ui.painter().text(
            egui::pos2(rect.left() + 14.0, middle),
            Align2::LEFT_CENTER,
            d.icon(),
            FontId::proportional(15.0),
            if current { p.accent } else { p.dim },
        );
        let can_eject = d.removable && d.block.is_some();
        let text_w = rect.width() - if can_eject { 72.0 } else { 48.0 };
        let galley = text_galley(ui, &d.name, 14.0, color, text_w, 1, false);
        ui.painter().galley(
            egui::pos2(rect.left() + 40.0, middle - galley.size().y / 2.0),
            galley,
            color,
        );
        if let Some(fill) = bar {
            let track = Rect::from_min_size(
                egui::pos2(rect.left() + 40.0, rect.bottom() - 7.0),
                vec2(rect.width() - 52.0, 3.0),
            );
            ui.painter().rect_filled(track, 2.0, p.muted);
            let used = if fill >= 0.9 { p.red } else { p.accent };
            ui.painter().rect_filled(
                Rect::from_min_size(track.min, vec2(track.width() * fill, track.height())),
                2.0,
                used,
            );
        }
        // The eject button sits over the row, so it gets the click first.
        let mut ejected = false;
        if can_eject {
            let button =
                Rect::from_center_size(egui::pos2(rect.right() - 16.0, middle), vec2(24.0, 22.0));
            let eject = ui.interact(button, resp.id.with("eject"), Sense::click());
            let tint = if eject.hovered() { p.accent } else { p.dim };
            ui.painter().text(
                button.center(),
                Align2::CENTER_CENTER,
                '\u{f052}',
                FontId::proportional(13.0),
                tint,
            );
            if eject.on_hover_text(tr("Eject")).clicked() {
                self.pending.push(Action::Eject(d.clone()));
                ejected = true;
            }
        }
        if resp.clicked() && !ejected {
            self.pending.push(Action::OpenDevice(d.clone()));
        }
        resp.context_menu(|ui| {
            if ui.button(tr("Open")).clicked() {
                self.pending.push(Action::OpenDevice(d.clone()));
            }
            if d.kind == devices::Kind::Network {
                if ui.button(tr("Copy Address")).clicked() {
                    ui.ctx().copy_text(d.detail.clone());
                }
                return;
            }
            if mounted && ui.button(tr("Unmount")).clicked() {
                self.pending.push(Action::Unmount(d.clone()));
            }
            if can_eject && ui.button(tr("Eject")).clicked() {
                self.pending.push(Action::Eject(d.clone()));
            }
            if d.removable {
                ui.separator();
                if ui
                    .button(RichText::new(tr("Format…")).color(p.red))
                    .clicked()
                {
                    self.pending.push(Action::AskFormat(d.clone()));
                }
            }
        });
        resp.on_hover_text(d.summary());
    }

    /// Leaves a drive's folders before it goes away, so nothing of Ferry's keeps it busy.
    fn leave(&mut self, d: &Device) {
        if d.mount.as_ref().is_some_and(|m| self.cwd.starts_with(m)) {
            self.go(files::home(), None, true);
        }
        if self
            .editor
            .as_ref()
            .is_some_and(|e| d.mount.as_ref().is_some_and(|m| e.path.starts_with(m)))
        {
            self.request_close_editor();
        }
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        let p = self.p();
        ui.horizontal(|ui| {
            match &self.status {
                _ if self.loading.is_some() => {
                    let path = self.loading.as_ref().unwrap().display().to_string();
                    ui.spinner();
                    ui.label(RichText::new(tr("Loading %1…").replace("%1", &path)).color(p.fg));
                }
                Some((message, error)) => {
                    if self.busy > 0 {
                        ui.spinner();
                    }
                    ui.label(RichText::new(message).color(if *error { p.red } else { p.fg }));
                }
                None => {
                    let folders = self
                        .view
                        .iter()
                        .filter(|&&i| self.entries[i].is_dir)
                        .count();
                    let files_n = self.view.len() - folders;
                    let mut text = format!(
                        "{}, {}",
                        trn("%n folders", folders),
                        trn("%n files", files_n)
                    );
                    if !self.selected.is_empty() {
                        let size: u64 = self
                            .entries
                            .iter()
                            .filter(|e| !e.is_dir && self.selected.contains(&e.path))
                            .map(|e| e.size)
                            .sum();
                        text = format!(
                            "{} ({})",
                            trn("%n selected", self.selected.len()),
                            files::human_size(size)
                        );
                    }
                    ui.label(RichText::new(text).color(p.dim));
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new("?").color(p.dim))
                    .on_hover_text(tr("Keyboard shortcuts"));
                let zoom = self.zoom();
                if (zoom - 1.0).abs() > 0.01 {
                    let label = egui::Button::new(
                        RichText::new(format!("{:.0}%", zoom * 100.0)).color(p.dim),
                    )
                    .frame(false);
                    if ui
                        .add(label)
                        .on_hover_text(tr("Reset zoom (Ctrl+0)"))
                        .clicked()
                    {
                        self.zoom_by(0.0);
                    }
                }
                if let Some(free) = self.free {
                    ui.label(
                        RichText::new(tr("%1 free").replace("%1", &files::human_size(free)))
                            .color(p.dim),
                    );
                }
            });
        });
    }

    fn folder(&mut self, ui: &mut Ui) {
        let p = self.p();
        ui.ctx()
            .data_mut(|d| d.insert_temp(egui::Id::unique("ferry-width"), ui.available_width()));
        // The background sits under the files: clicking it clears the selection, right-clicking
        // it offers what can be done in the folder itself.
        let background = ui.interact(
            ui.available_rect_before_wrap(),
            ui.scope_id().with("background"),
            Sense::click(),
        );
        if background.clicked() {
            self.selected.clear();
        }
        background.context_menu(|ui| self.folder_menu(ui));

        if self.view.is_empty() {
            let message = match (&self.status, self.filter.is_empty()) {
                (_, false) => tr("Nothing here matches. Press Enter to search the subfolders."),
                _ if self.loading.is_some() => tr("Loading…"),
                _ => tr("This folder is empty"),
            };
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new(format!("\u{f07c}\n\n{message}"))
                        .color(p.dim)
                        .size(15.0),
                )
            });
            return;
        }
        if self.grid {
            self.grid_view(ui, &p)
        } else {
            self.details_view(ui, &p)
        }
    }

    /// Scroll offset that brings the cursor into view, when the keyboard moved it.
    fn scroll_target(&mut self, row_height: f32, per_row: usize) -> Option<f32> {
        let cursor = self
            .cursor
            .filter(|_| std::mem::take(&mut self.scroll_to_cursor))?;
        let top = (cursor / per_row) as f32 * row_height;
        let (offset, height) = self.viewport;
        if top < offset {
            Some(top)
        } else if top + row_height > offset + height {
            Some(top + row_height - height)
        } else {
            None
        }
    }

    fn details_view(&mut self, ui: &mut Ui, p: &Palette) {
        let width = ui.available_width();
        let z = self.zoom_list;
        let (size_w, date_w) = (100.0 * z, 150.0 * z);
        let name_w = (width - size_w - date_w - 48.0 * z).max(80.0);
        // Column headers sort, and a second click reverses.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for (key, label, w, align) in [
                (SortKey::Name, tr("Name"), name_w + 40.0 * z, Align::Min),
                (SortKey::Size, tr("Size"), size_w, Align::Max),
                (SortKey::Modified, tr("Modified"), date_w, Align::Max),
            ] {
                let arrow = if self.sort == key {
                    if self.ascending {
                        " \u{f0de}"
                    } else {
                        " \u{f0dd}"
                    }
                } else {
                    ""
                };
                let (rect, resp) = ui.allocate_exact_size(vec2(w, 24.0), Sense::click());
                let color = if self.sort == key {
                    p.accent
                } else if resp.hovered() {
                    p.fg
                } else {
                    p.dim
                };
                let (pos, anchor) = if align == Align::Min {
                    (rect.left_center() + vec2(12.0, 0.0), Align2::LEFT_CENTER)
                } else {
                    (rect.right_center() - vec2(4.0, 0.0), Align2::RIGHT_CENTER)
                };
                ui.painter().text(
                    pos,
                    anchor,
                    format!("{label}{arrow}"),
                    FontId::proportional(12.5),
                    color,
                );
                if resp.clicked() {
                    self.resort(key);
                }
            }
        });
        ui.painter().hline(
            ui.max_rect().x_range(),
            ui.cursor().top(),
            Stroke::new(1.0, p.selection),
        );

        let mut area = ScrollArea::vertical().auto_shrink(false);
        let row = self.row_height();
        if let Some(offset) = self.scroll_target(row, 1) {
            area = area.vertical_scroll_offset(offset);
        }
        ui.spacing_mut().item_spacing.y = 0.0;
        let out = area.show_rows(ui, row, self.view.len(), |ui, rows| {
            for i in rows {
                self.detail_row(ui, i, p, name_w, size_w, date_w);
            }
        });
        self.viewport = (out.state.clamped_offset().y, out.inner_rect.height());
    }

    fn detail_row(
        &mut self,
        ui: &mut Ui,
        i: usize,
        p: &Palette,
        name_w: f32,
        size_w: f32,
        date_w: f32,
    ) {
        let z = self.zoom_list;
        let (rect, resp) = ui.allocate_exact_size(
            vec2(ui.available_width(), self.row_height()),
            Sense::click_and_drag(),
        );
        let (selected, faded) = self.paint_row_state(ui, i, rect, &resp, p);
        // Below 150% a preview would be too small to tell apart from the icon.
        let slot = Rect::from_center_size(
            rect.left_center() + vec2(22.0 * z, 0.0),
            Vec2::splat(rect.height() - 4.0),
        );
        let preview = if z >= 1.5 {
            self.preview(ui, i, slot)
        } else {
            None
        };
        let e = self.entry(i);
        let painter = ui.painter();
        let text_color = (if selected { p.bright } else { p.fg }).gamma_multiply(faded);
        match preview {
            Some((texture, at)) => paint_preview(painter, texture, at, faded),
            None => {
                painter.text(
                    rect.left_center() + vec2(14.0 * z, 0.0),
                    Align2::LEFT_CENTER,
                    files::glyph(e),
                    FontId::proportional(16.0 * z),
                    kind_color(e, p).gamma_multiply(faded),
                );
            }
        }
        let name = text_galley(ui, &e.name, 14.0 * z, text_color, name_w, 1, false);
        painter.galley(
            rect.left_center() + vec2(40.0 * z, -name.size().y / 2.0),
            name,
            text_color,
        );
        let size = if e.is_dir {
            String::new()
        } else {
            files::human_size(e.size)
        };
        let x = rect.left() + 40.0 * z + name_w;
        painter.text(
            egui::pos2(x + size_w, rect.center().y),
            Align2::RIGHT_CENTER,
            size,
            FontId::proportional(13.0 * z),
            p.dim,
        );
        let date = e.modified.map(files::format_time).unwrap_or_default();
        painter.text(
            egui::pos2(x + size_w + date_w, rect.center().y),
            Align2::RIGHT_CENTER,
            date,
            FontId::proportional(13.0 * z),
            p.dim,
        );
        self.row_input(i, resp);
    }

    fn grid_view(&mut self, ui: &mut Ui, p: &Palette) {
        let cols = self.columns();
        let rows = self.view.len().div_ceil(cols);
        let mut area = ScrollArea::vertical().auto_shrink(false);
        let tile = self.tile_size();
        if let Some(offset) = self.scroll_target(tile.y, cols) {
            area = area.vertical_scroll_offset(offset);
        }
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        let out = area.show_rows(ui, tile.y, rows, |ui, range| {
            for row in range {
                ui.horizontal(|ui| {
                    for i in row * cols..((row + 1) * cols).min(self.view.len()) {
                        self.tile(ui, i, p);
                    }
                });
            }
        });
        self.viewport = (out.state.clamped_offset().y, out.inner_rect.height());
    }

    fn tile(&mut self, ui: &mut Ui, i: usize, p: &Palette) {
        let (z, tile, name_size) = (self.zoom_grid, self.tile_size(), self.name_size());
        let (rect, resp) = ui.allocate_exact_size(tile, Sense::click_and_drag());
        let (selected, faded) = self.paint_row_state(ui, i, rect.shrink(3.0), &resp, p);
        let slot = Rect::from_center_size(
            rect.center_top() + vec2(0.0, 30.0 * z),
            vec2(tile.x - 16.0, 50.0 * z),
        );
        let preview = self.preview(ui, i, slot);
        let e = self.entry(i);
        let painter = ui.painter();
        match preview {
            Some((texture, at)) => paint_preview(painter, texture, at, faded),
            None => {
                painter.text(
                    rect.center_top() + vec2(0.0, 30.0 * z),
                    Align2::CENTER_CENTER,
                    files::glyph(e),
                    FontId::proportional(38.0 * z),
                    kind_color(e, p).gamma_multiply(faded),
                );
            }
        }
        let color = (if selected { p.bright } else { p.fg }).gamma_multiply(faded);
        let name = text_galley(ui, &e.name, name_size, color, tile.x - 12.0, 2, true);
        painter.galley(rect.center_top() + vec2(0.0, 56.0 * z), name, color);
        self.row_input(i, resp);
    }

    /// The preview of a file, fitted inside `slot` keeping its proportions, if one is ready.
    fn preview(&mut self, ui: &Ui, i: usize, slot: Rect) -> Option<(egui::TextureId, Rect)> {
        let e = self.entry(i);
        if e.is_dir {
            return None;
        }
        let (path, modified) = (e.path.clone(), e.modified);
        let pixels = slot.width().max(slot.height()) * ui.ctx().pixels_per_point();
        let texture = self.thumbs.get(&path, modified, pixels)?;
        let size = texture.size_vec2();
        let scale = (slot.width() / size.x).min(slot.height() / size.y);
        Some((
            texture.id(),
            Rect::from_center_size(slot.center(), size * scale),
        ))
    }

    /// Paints selection, hover and cursor behind a file; returns (selected, opacity).
    fn paint_row_state(
        &self,
        ui: &Ui,
        i: usize,
        rect: Rect,
        resp: &egui::Response,
        p: &Palette,
    ) -> (bool, f32) {
        let e = self.entry(i);
        let selected = self.selected.contains(&e.path);
        let r = CornerRadius::same(self.theme.radius().min(4));
        if selected {
            ui.painter()
                .rect_filled(rect, r, p.accent.gamma_multiply(0.28));
        } else if resp.hovered() {
            ui.painter().rect_filled(rect, r, p.surface);
        }
        if self.cursor == Some(i) && !self.ctx.egui_wants_keyboard_input() {
            ui.painter().rect_stroke(
                rect.shrink(0.5),
                r,
                Stroke::new(1.0, p.accent.gamma_multiply(0.45)),
                egui::StrokeKind::Inside,
            );
        }
        // Cut files fade until they are pasted, as in Dolphin.
        let cut = self
            .clip
            .as_ref()
            .is_some_and(|c| c.cut && c.paths.contains(&e.path));
        let hidden = e.hidden();
        (
            selected,
            if cut {
                0.45
            } else if hidden {
                0.7
            } else {
                1.0
            },
        )
    }

    fn row_input(&mut self, i: usize, resp: egui::Response) {
        if resp.clicked() {
            let mods = resp.ctx.input(|i| i.modifiers);
            self.click(i, mods);
        }
        if resp.double_clicked() {
            self.pending.push(Action::Open);
        }
        if resp.secondary_clicked() && !self.selected.contains(&self.entry(i).path) {
            self.click(i, Modifiers::NONE);
        }
        // Dragging a file that isn't selected drags just it, as in Dolphin; a selected one drags the selection.
        if resp.drag_started() {
            if !self.selected.contains(&self.entry(i).path) {
                self.click(i, Modifiers::NONE);
            }
            let paths = self.targets();
            let folders: Vec<PathBuf> = self
                .entries
                .iter()
                .filter(|e| e.is_dir && paths.contains(&e.path))
                .map(|e| e.path.clone())
                .collect();
            debug(&format!(
                "drag started: {} items, {} folders",
                paths.len(),
                folders.len()
            ));
            egui::DragAndDrop::set_payload(&resp.ctx, Dragged { paths, folders });
        }
        resp.context_menu(|ui| self.item_menu(ui));
    }

    fn item_menu(&mut self, ui: &mut Ui) {
        let single = self.targets().len() == 1;
        let is_dir = single && self.is_folder(&self.targets()[0]);
        for (label, keys, action, enabled) in [
            (tr("Open"), "Enter", Action::Open, true),
            (
                tr("Open in Text Editor"),
                "",
                Action::EditText(self.targets().first().cloned().unwrap_or_default()),
                single && !is_dir,
            ),
            (
                tr("Open With Default App"),
                "",
                Action::OpenExternal(self.targets().first().cloned().unwrap_or_default()),
                single && !is_dir,
            ),
            (tr("Cut"), "Ctrl+X", Action::Cut, true),
            (tr("Copy"), "Ctrl+C", Action::Copy, true),
            (tr("Copy Path"), "Ctrl+Alt+C", Action::CopyPath, true),
            (tr("Rename…"), "F2", Action::AskRename, single),
            (tr("Move to Trash"), "Del", Action::Trash, true),
            (tr("Delete…"), "Shift+Del", Action::AskDelete, true),
        ] {
            if ui
                .add_enabled(enabled, egui::Button::new(label).shortcut_text(keys))
                .clicked()
            {
                self.pending.push(action);
            }
        }
        let folders: Vec<PathBuf> = self
            .place_targets()
            .into_iter()
            .filter(|p| !self.in_places(p))
            .collect();
        if self.selected.iter().any(|p| self.is_folder(p)) && !folders.is_empty() {
            ui.separator();
            if ui
                .add(egui::Button::new(tr("Add to Bookmarks")).shortcut_text("Ctrl+D"))
                .clicked()
            {
                self.pending.push(Action::AddPlaces(folders));
            }
        }
        if is_dir {
            ui.separator();
            if ui.button(tr("Open Terminal Here")).clicked() {
                let dir = self.targets()[0].clone();
                let mut command = Command::new("xdg-terminal-exec");
                command.arg(format!("--dir={}", dir.display()));
                self.launch(command, tr("Terminal"));
            }
        }
    }

    fn folder_menu(&mut self, ui: &mut Ui) {
        let can_paste = self.clip.is_some();
        for (label, keys, action, enabled) in [
            (
                tr("New Folder…"),
                "Ctrl+Shift+N",
                Action::AskNewFolder,
                true,
            ),
            (tr("New Text File…"), "Ctrl+Alt+N", Action::AskNewFile, true),
            (tr("Paste"), "Ctrl+V", Action::Paste, can_paste),
            (tr("Select All"), "Ctrl+A", Action::SelectAll, true),
            (tr("Open Terminal Here"), "F4", Action::Terminal, true),
            (
                tr("Add to Bookmarks"),
                "Ctrl+D",
                Action::AddPlaces(vec![self.cwd.clone()]),
                !self.in_places(&self.cwd),
            ),
            (
                if self.show_hidden {
                    tr("Hide Hidden Files")
                } else {
                    tr("Show Hidden Files")
                },
                "Ctrl+H",
                Action::ToggleHidden,
                true,
            ),
            (tr("Refresh"), "F5", Action::Refresh, true),
        ] {
            if ui
                .add_enabled(enabled, egui::Button::new(label).shortcut_text(keys))
                .clicked()
            {
                self.pending.push(action);
            }
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.dialog else {
            return;
        };
        let p = self.theme.palette;
        let (enter, escape) =
            ctx.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
        let mut close = escape;
        let mut confirm = false;
        // Unsaved-changes answer: Some(Some(then)) saves first, Some(None) discards.
        let mut unsaved: Option<Option<AfterSave>> = None;
        let mut unsaved_quit = false;
        let frame = egui::Frame::popup(&ctx.global_style())
            .inner_margin(Margin::same(18))
            .fill(p.surface);
        // Each kind of dialog keeps its own id, or the next one opens at the last one's size.
        let kind = std::mem::discriminant(&*dialog);
        egui::Modal::new(egui::Id::unique(("ferry-dialog", kind)))
            .frame(frame)
            .show(ctx, |ui| {
                ui.set_width(380.0);
                let title = match dialog {
                    Dialog::Rename { .. } => Some(tr("Rename")),
                    Dialog::NewFolder { .. } => Some(tr("New Folder")),
                    Dialog::NewFile { .. } => Some(tr("New Text File")),
                    _ => None,
                };
                if let Some(title) = title {
                    ui.label(RichText::new(title).size(16.0).color(p.bright));
                    ui.add_space(6.0);
                }
                match dialog {
                    Dialog::Rename { text, error, .. }
                    | Dialog::NewFolder { text, error }
                    | Dialog::NewFile { text, error } => {
                        let edit = ui.add(
                            TextEdit::singleline(text)
                                .desired_width(f32::INFINITY)
                                .margin(Margin::symmetric(8, 6)),
                        );
                        edit.request_focus();
                        if let Some(e) = error {
                            ui.label(RichText::new(e.as_str()).color(p.red));
                        }
                        confirm = enter;
                    }
                    Dialog::Format { device, fs, label } => {
                        ui.label(
                            RichText::new(tr("Format %1?").replace("%1", &device.name))
                                .size(16.0)
                                .color(p.bright),
                        );
                        ui.add_space(2.0);
                        ui.label(RichText::new(device.summary()).color(p.dim).size(12.0));
                        ui.add_space(8.0);
                        for (i, (_, what)) in devices::formats().iter().enumerate() {
                            ui.radio_value(fs, i, what.as_str());
                        }
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.label(tr("Name"));
                            ui.add(
                                TextEdit::singleline(label)
                                    .desired_width(f32::INFINITY)
                                    .margin(Margin::symmetric(8, 5)),
                            );
                        });
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(tr("Everything on it will be erased. This can't be undone."))
                                .color(p.red),
                        );
                    }
                    Dialog::Delete { paths } => {
                        ui.label(
                            RichText::new(trn("Permanently delete %n items?", paths.len()))
                                .size(16.0)
                                .color(p.bright),
                        );
                        ui.add_space(4.0);
                        for path in paths.iter().take(5) {
                            ui.label(
                                RichText::new(
                                    path.file_name().unwrap_or_default().to_string_lossy(),
                                )
                                .color(p.fg),
                            );
                        }
                        if paths.len() > 5 {
                            ui.label(RichText::new("…").color(p.dim));
                        }
                        ui.add_space(4.0);
                        ui.label(RichText::new(tr("This can't be undone.")).color(p.red));
                        confirm = enter;
                    }
                    Dialog::Help => {
                        help(ui, &p);
                        close |= enter;
                    }
                    Dialog::Unsaved { quit } => {
                        let name = self.editor.as_ref().map(|e| e.name()).unwrap_or_default();
                        ui.label(
                            RichText::new(tr("Save changes to %1?").replace("%1", &name))
                                .size(16.0)
                                .color(p.bright),
                        );
                        ui.add_space(4.0);
                        ui.label(RichText::new(tr("Your changes will be lost if you don't save them.")).color(p.fg));
                        ui.add_space(10.0);
                        let then = if *quit { AfterSave::Quit } else { AfterSave::Close };
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let save = egui::Button::new(RichText::new(tr("Save")).color(crate::theme::on(p.accent))).fill(p.accent);
                            if ui.add(save).clicked() || enter {
                                unsaved = Some(Some(then));
                            }
                            if ui.button(tr("Cancel")).clicked() {
                                close = true;
                            }
                            if ui.button(RichText::new(tr("Don't Save")).color(p.red)).clicked() {
                                unsaved = Some(None);
                            }
                        });
                        unsaved_quit = *quit;
                    }
                    Dialog::ChangedOnDisk => {
                        let name = self.editor.as_ref().map(|e| e.name()).unwrap_or_default();
                        ui.label(
                            RichText::new(tr("%1 changed on disk").replace("%1", &name))
                                .size(16.0)
                                .color(p.bright),
                        );
                        ui.add_space(4.0);
                        ui.label(RichText::new(tr("Another program saved it after you opened it here. Overwrite its changes with yours?")).color(p.fg));
                    }
                }
                if !matches!(dialog, Dialog::Help | Dialog::Unsaved { .. }) {
                    ui.add_space(10.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let label = match dialog {
                            Dialog::Rename { .. } => tr("Rename"),
                            Dialog::NewFolder { .. } | Dialog::NewFile { .. } => tr("Create"),
                            Dialog::ChangedOnDisk => tr("Overwrite"),
                            Dialog::Format { .. } => tr("Format"),
                            _ => tr("Delete"),
                        };
                        let fill = if matches!(
                            dialog,
                            Dialog::Delete { .. } | Dialog::ChangedOnDisk | Dialog::Format { .. }
                        ) {
                            p.red
                        } else {
                            p.accent
                        };
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(label).color(crate::theme::on(fill)),
                                )
                                .fill(fill),
                            )
                            .clicked()
                        {
                            confirm = true;
                        }
                        if ui.button(tr("Cancel")).clicked() {
                            close = true;
                        }
                    });
                }
            });
        if close {
            if matches!(self.dialog, Some(Dialog::ChangedOnDisk)) {
                self.after_save = None;
            }
            self.dialog = None;
        } else if let Some(answer) = unsaved {
            self.dialog = None;
            match answer {
                Some(then) => {
                    self.after_save = Some(then);
                    self.save_editor(false);
                }
                None if unsaved_quit => self.quit(),
                None => {
                    // Discard: forget the edits, then leave.
                    if let Some(editor) = &mut self.editor {
                        editor.read_only = true;
                    }
                    self.close_editor();
                }
            }
        } else if confirm {
            self.confirm();
        }
    }

    fn confirm(&mut self) {
        match self.dialog.take() {
            Some(Dialog::Rename { path, text, .. }) => {
                let dir = path.parent().map(PathBuf::from).unwrap_or_default();
                let result = files::check_name(&dir, &text, Some(&path)).and_then(|target| {
                    std::fs::rename(&path, &target)
                        .map(|_| target)
                        .map_err(|e| e.to_string())
                });
                match result {
                    Ok(target) => {
                        self.reload(Some(target));
                    }
                    Err(error) => {
                        self.dialog = Some(Dialog::Rename {
                            path,
                            text,
                            error: Some(error),
                        })
                    }
                }
            }
            Some(Dialog::NewFile { text, .. }) => {
                let result = files::check_name(&self.cwd, &text, None).and_then(|target| {
                    std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&target)
                        .map(|_| target)
                        .map_err(|e| e.to_string())
                });
                match result {
                    Ok(target) => {
                        self.reload(Some(target.clone()));
                        self.open_editor(target, true);
                    }
                    Err(error) => {
                        self.dialog = Some(Dialog::NewFile {
                            text,
                            error: Some(error),
                        })
                    }
                }
            }
            Some(Dialog::NewFolder { text, .. }) => {
                let result = files::check_name(&self.cwd, &text, None).and_then(|target| {
                    std::fs::create_dir(&target)
                        .map(|_| target)
                        .map_err(|e| e.to_string())
                });
                match result {
                    Ok(target) => {
                        self.reload(Some(target));
                    }
                    Err(error) => {
                        self.dialog = Some(Dialog::NewFolder {
                            text,
                            error: Some(error),
                        })
                    }
                }
            }
            Some(Dialog::Delete { paths }) => {
                let n = paths.len();
                self.job(tr("Deleting…"), move || {
                    files::delete(&paths).map(|_| trn("Deleted %n items", n))
                });
            }
            Some(Dialog::ChangedOnDisk) => self.save_editor(true),
            Some(Dialog::Format { device, fs, label }) => {
                let Some((fstype, _)) = devices::formats().get(fs).cloned() else {
                    return;
                };
                self.leave(&device);
                let (name, block) = (
                    device.name.clone(),
                    device.block.clone().unwrap_or_default(),
                );
                self.job(tr("Formatting %1…").replace("%1", &name), move || {
                    if device.mount.is_some() {
                        devices::unmount(&block)?;
                    }
                    devices::format(&block, fstype, &label)
                        .map(|_| tr("Formatted %1").replace("%1", &name))
                });
            }
            Some(Dialog::Help) | Some(Dialog::Unsaved { .. }) | None => {}
        }
    }

    /// Theme, language and folder changes are polled once a second; all three are tiny reads.
    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(done) = self.rx.try_recv() {
            if done.job {
                self.busy = self.busy.saturating_sub(1);
                self.refresh();
            }
            self.status = Some(match done.result {
                Ok(message) => (message, false),
                Err(message) => (message, true),
            });
        }
        while let Ok(loaded) = self.load_rx.try_recv() {
            match loaded {
                Loaded::Dir(load) => self.loaded(load),
                Loaded::Devices(list) => {
                    self.scanning_devices = false;
                    self.devices = list;
                }
                Loaded::Mounted(Ok(path)) => {
                    self.busy = self.busy.saturating_sub(1);
                    self.status = None;
                    self.go(path, None, true);
                }
                Loaded::Mounted(Err(message)) => {
                    self.busy = self.busy.saturating_sub(1);
                    self.error(message);
                }
                Loaded::Text {
                    path,
                    forced,
                    result,
                } => self.text_loaded(path, forced, result),
                Loaded::Saved { path, text, result } => self.text_saved(path, text, result),
                Loaded::Check { path, mtime, free } => {
                    self.checking = false;
                    if path == self.cwd && self.loading.is_none() {
                        self.free = free;
                        if mtime != self.dir_mtime {
                            self.refresh();
                        }
                    }
                }
            }
        }
        let now = ctx.input(|i| i.time);
        let opacity = self.opacity();
        self.theme.set_opacity(ctx, opacity);
        let scale = self.scale();
        if (ctx.zoom_factor() - scale).abs() > 0.001 {
            ctx.set_zoom_factor(scale);
        }
        self.poll_search(ctx, now);
        if now - self.last_poll >= 1.0 {
            self.last_poll = now;
            self.theme.poll(ctx);
            let language = i18n::read_language(&language_file());
            if language != self.language {
                i18n::set(&language);
                self.places = files::places();
                self.language = language;
            }
            // Off the UI thread too: the shown folder may sit on a mount that stops answering.
            if !self.checking && self.loading.is_none() {
                self.checking = true;
                let (tx, ctx, path) = (self.load_tx.clone(), ctx.clone(), self.cwd.clone());
                std::thread::spawn(move || {
                    let (mtime, free) = (mtime(&path), files::free_space(&path));
                    let _ = tx.send(Loaded::Check { path, mtime, free });
                    ctx.request_repaint();
                });
            }
            // lsblk is quick, but it's a program: off the UI thread like everything else.
            if !self.scanning_devices {
                self.scanning_devices = true;
                let (tx, ctx) = (self.load_tx.clone(), ctx.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(Loaded::Devices(devices::scan()));
                    ctx.request_repaint();
                });
            }
            // Other apps edit the same bookmarks file, so it is re-read like the folder.
            self.bookmarks = files::bookmarks(&self.bookmarks_file);
            self.save_settings();
        }
        self.thumbs.collect(ctx);
        ctx.request_repaint_after(Duration::from_secs(1));
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll(ctx);
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // The window, and the Wayland connection the drag code shares, still exist here.
        if let Some(dnd) = &mut self.dnd {
            dnd.shutdown();
        }
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        // The compositor owns the pointer during a desktop drag and keeps the release, so tell
        // egui the button came up, or it would think a drag is still going when we're back.
        if std::mem::take(&mut self.release_pointer) {
            raw_input.events.push(Event::PointerButton {
                pos: egui::Pos2::ZERO,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            });
            raw_input.events.push(Event::PointerGone);
        }
    }
}

impl App {
    fn draw(&mut self, ui: &mut Ui) {
        let p = self.p();
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.viewport().close_requested())
            && !self.allow_close
            && self.editor.as_ref().is_some_and(|e| e.dirty())
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.dialog = Some(Dialog::Unsaved { quit: true });
        }
        if self.dialog.is_none() {
            self.keys(ui);
        }

        Panel::top("toolbar")
            .frame(
                egui::Frame::new()
                    .fill(self.glass(p.bg))
                    .inner_margin(Margin::symmetric(10, 8)),
            )
            .show_separator_line(false)
            .show(ui, |ui| self.toolbar(ui));
        Panel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(self.glass(p.sidebar))
                    .inner_margin(Margin::symmetric(12, 5)),
            )
            .show_separator_line(false)
            .show(ui, |ui| self.status_bar(ui));
        Panel::left("places")
            .frame(
                egui::Frame::new()
                    .fill(self.glass(p.sidebar))
                    .inner_margin(Margin::symmetric(10, 6)),
            )
            .resizable(true)
            .default_size(210.0)
            .size_range(150.0..=360.0)
            .show_separator_line(false)
            .show(ui, |ui| self.sidebar(ui));
        egui::CentralPanel::no_frame()
            .frame(
                egui::Frame::new()
                    .fill(self.glass(p.bg))
                    .inner_margin(Margin {
                        left: 6,
                        right: 6,
                        top: 2,
                        bottom: 0,
                    }),
            )
            .show(ui, |ui| {
                if self.editor.is_some() {
                    self.editor_view(ui);
                } else if self.search.is_some() {
                    self.search_view(ui);
                } else {
                    self.folder(ui);
                }
            });

        self.drag_out(&ctx);
        self.drag_preview(&ctx, &p);
        self.dialogs(&ctx);
        for action in std::mem::take(&mut self.pending) {
            self.apply(action);
        }
    }
}

impl App {
    /// Starts a search below the shown folder, taking over what the filter had typed.
    fn open_search(&mut self) {
        let text = std::mem::take(&mut self.filter);
        self.rebuild_view();
        let typed = (!text.trim().is_empty()).then_some(0.0);
        self.search = Some(SearchView {
            root: self.cwd.clone(),
            text,
            mode: search::Mode::Names,
            everything: self.show_hidden,
            hits: Vec::new(),
            sorted: true,
            done: true,
            typed,
            cursor: 0,
            scroll: false,
            focus: true,
            viewport: (0.0, 0.0),
        });
    }

    fn close_search(&mut self) {
        if self.search.take().is_some() {
            // Any search still walking stops at its next file.
            self.search_id += 1;
            self.search_progress
                .current
                .store(self.search_id, std::sync::atomic::Ordering::SeqCst);
        }
    }

    fn run_search(&mut self) {
        let Some(s) = self.search.as_mut() else {
            return;
        };
        self.search_id += 1;
        s.hits.clear();
        s.cursor = 0;
        s.sorted = true;
        let query = search::Query {
            text: s.text.clone(),
            mode: s.mode,
            root: s.root.clone(),
            everything: s.everything,
        };
        if query.text.trim().is_empty() {
            s.done = true;
            self.search_progress
                .current
                .store(self.search_id, std::sync::atomic::Ordering::SeqCst);
            return;
        }
        s.done = false;
        let ctx = self.ctx.clone();
        search::start(
            query,
            self.search_id,
            self.search_progress.clone(),
            self.search_tx.clone(),
            move || ctx.request_repaint(),
        );
    }

    fn poll_search(&mut self, ctx: &egui::Context, now: f64) {
        while let Ok(update) = self.search_rx.try_recv() {
            let current = self.search_id;
            let Some(s) = self.search.as_mut() else {
                continue;
            };
            match update {
                search::Update::Hit(id, hit)
                    if id == current && s.hits.len() < search::MAX_HITS =>
                {
                    s.hits.push(hit);
                    s.sorted = false;
                }
                search::Update::Done(id) if id == current => s.done = true,
                _ => {}
            }
        }
        let due = self
            .search
            .as_ref()
            .and_then(|s| s.typed)
            .map(|t| now - t >= 0.15);
        match due {
            Some(true) => {
                if let Some(s) = self.search.as_mut() {
                    s.typed = None;
                }
                self.run_search();
            }
            Some(false) => ctx.request_repaint_after(Duration::from_millis(150)),
            None => {}
        }
    }

    /// The results as rows: each hit, followed by its matching lines in a contents search.
    fn search_rows(s: &SearchView) -> Vec<Row> {
        let mut rows = Vec::with_capacity(s.hits.len());
        for (i, hit) in s.hits.iter().enumerate() {
            rows.push(Row::Hit(i));
            rows.extend((0..hit.lines.len()).map(|l| Row::Line(i, l)));
        }
        rows
    }

    fn search_action(s: &SearchView, row: Row) -> Action {
        match row {
            Row::Hit(i) => Action::OpenFound {
                path: s.hits[i].path.clone(),
                is_dir: s.hits[i].is_dir,
                line: None,
            },
            Row::Line(i, l) => Action::OpenFound {
                path: s.hits[i].path.clone(),
                is_dir: false,
                line: Some(s.hits[i].lines[l].number),
            },
        }
    }

    fn search_key(&mut self, key: Key, m: Modifiers, ctx: &egui::Context) {
        let Some(s) = self.search.as_mut() else {
            return;
        };
        let rows = Self::search_rows(s);
        let last = rows.len().saturating_sub(1);
        let page = ((s.viewport.1 / (26.0 * self.zoom_list)) as usize).max(1);
        let mut moved = true;
        match key {
            Key::Escape => self.pending.push(Action::CloseSearch),
            Key::ArrowDown => s.cursor = (s.cursor + 1).min(last),
            Key::ArrowUp => s.cursor = s.cursor.saturating_sub(1),
            Key::PageDown => s.cursor = (s.cursor + page).min(last),
            Key::PageUp => s.cursor = s.cursor.saturating_sub(page),
            Key::Enter if !rows.is_empty() => {
                let row = rows[s.cursor.min(last)];
                self.pending.push(match row {
                    // Alt+Enter shows a result in its folder instead of opening it.
                    Row::Hit(i) if m.alt => Action::Reveal(s.hits[i].path.clone()),
                    _ => Self::search_action(s, row),
                });
                moved = false;
            }
            Key::F if m.command => s.focus = true,
            Key::Tab => {
                s.mode = match s.mode {
                    search::Mode::Names => search::Mode::Contents,
                    search::Mode::Contents => search::Mode::Names,
                };
                s.typed = Some(0.0);
                s.focus = true;
                moved = false;
            }
            Key::Q if m.command => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Key::L if m.command => {
                self.pending.push(Action::CloseSearch);
                self.path_edit = Some(self.cwd.to_string_lossy().into_owned());
                moved = false;
            }
            _ => moved = false,
        }
        if moved && let Some(s) = self.search.as_mut() {
            s.scroll = true;
        }
    }

    fn search_view(&mut self, ui: &mut Ui) {
        let p = self.p();
        let z = self.zoom_list;
        let now = ui.input(|i| i.time);
        let scanned = self
            .search_progress
            .scanned
            .load(std::sync::atomic::Ordering::Relaxed);
        let Some(s) = self.search.as_mut() else {
            return;
        };
        let mut rerun = false;
        let mut action = None;

        // The query, what to search, and how it's going.
        ui.horizontal(|ui| {
            if ui
                .add(egui::Button::new(RichText::new("\u{f053}").size(14.0)).frame(false))
                .on_hover_text(tr("Back to the folder (Esc)"))
                .clicked()
            {
                action = Some(Action::CloseSearch);
            }
            let edit = ui.add(
                TextEdit::singleline(&mut s.text)
                    .hint_text(format!(
                        "\u{f002}  {}",
                        tr("Search in %1").replace("%1", &folder_name(&s.root))
                    ))
                    .desired_width(300.0)
                    .margin(Margin::symmetric(8, 5)),
            );
            // The query keeps the keyboard, the way a picker does; arrows move through results.
            if std::mem::take(&mut s.focus) || edit.lost_focus() {
                edit.request_focus();
            }
            if edit.changed() {
                s.typed = Some(now);
            }
            ui.add_space(6.0);
            for (mode, label, tip) in [
                (
                    search::Mode::Names,
                    tr("Names"),
                    tr("Match file and folder names, fuzzily (Tab)"),
                ),
                (
                    search::Mode::Contents,
                    tr("Contents"),
                    tr("Look for the text inside files (Tab)"),
                ),
            ] {
                if ui
                    .selectable_label(s.mode == mode, label)
                    .on_hover_text(tip)
                    .clicked()
                    && s.mode != mode
                {
                    s.mode = mode;
                    rerun = true;
                }
            }
            let eye = if s.everything { '\u{f06e}' } else { '\u{f070}' };
            if icon_toggle(
                ui,
                eye,
                s.everything,
                &tr("Include hidden files and what .gitignore skips"),
            )
            .clicked()
            {
                s.everything = !s.everything;
                rerun = true;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let n = s.hits.len();
                let status = if !s.done {
                    tr("Searching… %1 items looked at").replace("%1", &scanned.to_string())
                } else if s.text.trim().is_empty() {
                    String::new()
                } else if n >= search::MAX_HITS {
                    tr("Showing the first %1 results").replace("%1", &n.to_string())
                } else {
                    trn("%n results", n)
                };
                ui.label(RichText::new(status).color(p.dim));
                if !s.done {
                    ui.spinner();
                }
            });
        });
        ui.add_space(4.0);
        if rerun {
            s.typed = Some(0.0);
        }

        if !s.sorted {
            match s.mode {
                // Best match first; between equals, the shorter (closer) path.
                search::Mode::Names => s.hits.sort_by(|a, b| {
                    b.score
                        .cmp(&a.score)
                        .then(a.path.as_os_str().len().cmp(&b.path.as_os_str().len()))
                        .then(a.path.cmp(&b.path))
                }),
                search::Mode::Contents => s.hits.sort_by(|a, b| a.path.cmp(&b.path)),
            }
            s.sorted = true;
        }
        let rows = Self::search_rows(s);
        if rows.is_empty() {
            let message = if s.text.trim().is_empty() {
                tr("Type to search in %1 and its subfolders").replace("%1", &folder_name(&s.root))
            } else if !s.done || s.typed.is_some() {
                tr("Searching…")
            } else {
                tr("Nothing found")
            };
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new(message).color(p.dim).size(15.0))
            });
            self.pending.extend(action);
            return;
        }
        s.cursor = s.cursor.min(rows.len() - 1);

        let row_h = (26.0 * z).round();
        let mut area = ScrollArea::vertical().auto_shrink(false);
        if std::mem::take(&mut s.scroll) {
            let (offset, height) = s.viewport;
            let top = s.cursor as f32 * row_h;
            if top < offset {
                area = area.vertical_scroll_offset(top);
            } else if top + row_h > offset + height {
                area = area.vertical_scroll_offset(top + row_h - height);
            }
        }
        let root = s.root.clone();
        let output = area.show_rows(ui, row_h, rows.len(), |ui, range| {
            for r in range {
                let row = rows[r];
                let width = ui.available_width();
                let (rect, resp) = ui.allocate_exact_size(vec2(width, row_h), Sense::click());
                if r == s.cursor {
                    ui.painter().rect_filled(rect, 4.0, p.selection);
                } else if resp.hovered() {
                    ui.painter().rect_filled(rect, 4.0, p.surface);
                }
                let font = FontId::proportional((14.0 * z).round());
                let mono = FontId::monospace((13.0 * z).round());
                let job = match row {
                    Row::Hit(i) => {
                        let hit = &s.hits[i];
                        let relative = hit
                            .path
                            .strip_prefix(&root)
                            .unwrap_or(&hit.path)
                            .to_string_lossy()
                            .into_owned();
                        let entry = Entry {
                            name: hit
                                .path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                            path: hit.path.clone(),
                            is_dir: hit.is_dir,
                            is_link: false,
                            size: 0,
                            modified: None,
                        };
                        let icon = egui::text::LayoutJob::simple_singleline(
                            files::glyph(&entry).to_string(),
                            FontId::proportional((16.0 * z).round()),
                            kind_color(&entry, &p),
                        );
                        let galley = ui.painter().layout_job(icon);
                        ui.painter().galley(
                            rect.left_center() + vec2(8.0 * z, -galley.size().y / 2.0),
                            galley,
                            p.fg,
                        );
                        path_job(&relative, &hit.indices, &font, &p)
                    }
                    Row::Line(i, l) => line_job(&s.hits[i].lines[l], &mono, &p),
                };
                let indent = if matches!(row, Row::Line(..)) {
                    40.0
                } else {
                    34.0
                } * z;
                let mut job = job;
                job.wrap = egui::text::TextWrapping::truncate_at_width(width - indent - 8.0);
                let galley = ui.painter().layout_job(job);
                ui.painter().galley(
                    rect.left_center() + vec2(indent, -galley.size().y / 2.0),
                    galley,
                    p.fg,
                );

                if resp.clicked() {
                    s.cursor = r;
                }
                if resp.double_clicked() {
                    action = Some(Self::search_action(s, row));
                }
                let path = match row {
                    Row::Hit(i) | Row::Line(i, _) => s.hits[i].path.clone(),
                };
                resp.on_hover_text(path.display().to_string())
                    .context_menu(|ui| {
                        if ui.button(tr("Open")).clicked() {
                            action = Some(Self::search_action(s, row));
                        }
                        if ui
                            .add(egui::Button::new(tr("Show in Folder")).shortcut_text("Alt+Enter"))
                            .clicked()
                        {
                            action = Some(Action::Reveal(path.clone()));
                        }
                        if ui.button(tr("Copy Path")).clicked() {
                            ui.ctx().copy_text(path.display().to_string());
                        }
                    });
            }
        });
        s.viewport = (output.state.clamped_offset().y, output.inner_rect.height());
        self.pending.extend(action);
    }
}

fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// A result's path, folders dimmed, with the characters the search matched in the accent.
fn path_job(relative: &str, indices: &[u32], font: &FontId, p: &Palette) -> egui::text::LayoutJob {
    let name_start = relative.rfind('/').map_or(0, |i| i + 1);
    let mut job = egui::text::LayoutJob::default();
    for (n, (b, c)) in relative.char_indices().enumerate() {
        let color = if indices.contains(&(n as u32)) {
            p.accent
        } else if b < name_start {
            p.dim
        } else {
            p.fg
        };
        let format = egui::TextFormat {
            font_id: font.clone(),
            color,
            ..Default::default()
        };
        job.append(c.encode_utf8(&mut [0; 4]), 0.0, format);
    }
    job
}

/// A matching line: its number, then the text with the matches marked.
fn line_job(line: &search::Line, font: &FontId, p: &Palette) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let plain = |color| egui::TextFormat {
        font_id: font.clone(),
        color,
        ..Default::default()
    };
    job.append(&format!("{:>5}  ", line.number), 0.0, plain(p.dim));
    // Leading indentation only pushes the match out of sight.
    let trimmed = line.text.len() - line.text.trim_start().len();
    let mut at = trimmed;
    for &(start, end) in &line.matches {
        if start < at || end > line.text.len() {
            continue;
        }
        job.append(&line.text[at..start], 0.0, plain(p.fg));
        let mut format = plain(crate::theme::on(p.accent));
        format.background = p.accent;
        job.append(&line.text[start..end], 0.0, format);
        at = end;
    }
    job.append(&line.text[at..], 0.0, plain(p.fg));
    job
}

/// FERRY_DEBUG=1 prints what the app is doing to stderr, for chasing bugs on real hardware.
pub(crate) fn debug(message: &str) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("FERRY_DEBUG").is_some()) {
        eprintln!("ferry: {message}");
    }
}

fn language_file() -> PathBuf {
    files::home().join(".config/omarchy/language")
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn shellexpand_home(text: &str) -> String {
    match text.strip_prefix('~') {
        Some(rest) => format!("{}{rest}", files::home().display()),
        None => text.to_owned(),
    }
}

/// The path bar's segments: home and below reads "Home › Work", anything else starts at the root.
fn crumbs(path: &Path) -> Vec<(String, PathBuf)> {
    let home = files::home();
    let (mut acc, mut out, rest) = match path.strip_prefix(&home) {
        Ok(rest) => (
            home.clone(),
            vec![(format!("\u{f015}  {}", tr("Home")), home.clone())],
            rest.to_path_buf(),
        ),
        Err(_) => (
            PathBuf::from("/"),
            vec![("\u{f0a0} ".to_owned(), PathBuf::from("/"))],
            path.strip_prefix("/").unwrap_or(path).to_path_buf(),
        ),
    };
    for part in rest.components() {
        acc.push(part);
        out.push((part.as_os_str().to_string_lossy().into_owned(), acc.clone()));
    }
    out
}

fn paint_preview(painter: &egui::Painter, texture: egui::TextureId, at: Rect, opacity: f32) {
    let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    painter.image(texture, at, uv, Color32::WHITE.gamma_multiply(opacity));
}

/// Where settings live: ~/.config/ferry/settings.json.
pub fn settings_file() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| files::home().join(".config"))
        .join("ferry/settings.json")
}

fn kind_color(e: &Entry, p: &Palette) -> Color32 {
    match files::kind(e) {
        Kind::Folder => p.accent,
        Kind::Image => p.magenta,
        Kind::Audio => p.cyan,
        Kind::Video => p.red,
        Kind::Archive => p.yellow,
        Kind::Code => p.green,
        Kind::Document => p.fg,
        Kind::Other => p.dim,
    }
}

/// Text cut to `rows` lines with an ellipsis, so long names never push the layout around.
fn text_galley(
    ui: &Ui,
    text: &str,
    size: f32,
    color: Color32,
    width: f32,
    rows: usize,
    center: bool,
) -> Arc<Galley> {
    let mut job = LayoutJob::simple(
        text.to_owned(),
        FontId::proportional(size),
        color,
        width.max(10.0),
    );
    job.wrap.max_rows = rows;
    // One line cuts anywhere so it can end in an ellipsis; two lines wrap at words, like Dolphin.
    job.wrap.break_anywhere = rows == 1;
    job.wrap.overflow_character = Some('…');
    if center {
        job.halign = Align::Center;
    }
    ui.painter().layout_job(job)
}

fn section(ui: &mut Ui, title: &str, p: &Palette) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        ui.label(RichText::new(title.to_uppercase()).size(11.0).color(p.dim));
    });
    ui.add_space(2.0);
}

/// Sidebar rows: hover lightens; the current place is a soft accent pill, as in libadwaita's
/// navigation sidebars, rather than a hard bar.
fn paint_place(ui: &Ui, rect: Rect, current: bool, hovered: bool, radius: u8, p: &Palette) {
    let r = CornerRadius::same(radius.min(6));
    if current {
        ui.painter()
            .rect_filled(rect, r, p.accent.gamma_multiply(0.18));
    } else if hovered {
        ui.painter()
            .rect_filled(rect, r, p.surface.gamma_multiply(0.8));
    }
}

fn icon_button(ui: &mut Ui, icon: char, enabled: bool, tip: &str) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(icon.to_string()).size(15.0))
            .frame(false)
            .min_size(vec2(30.0, 28.0)),
    )
    .on_hover_text(tip)
}

fn icon_toggle(ui: &mut Ui, icon: char, on: bool, tip: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(icon.to_string()).size(15.0))
            .selected(on)
            .frame_when_inactive(on)
            .min_size(vec2(30.0, 28.0)),
    )
    .on_hover_text(tip)
}

fn help(ui: &mut Ui, p: &Palette) {
    ui.label(
        RichText::new(tr("Keyboard shortcuts"))
            .size(16.0)
            .color(p.bright),
    );
    ui.add_space(6.0);
    egui::Grid::new("help")
        .num_columns(2)
        .spacing(vec2(24.0, 5.0))
        .show(ui, |ui| {
            for (keys, what) in [
                ("Enter / Space", "Open"),
                ("Backspace / Alt+Left", "Back"),
                ("Alt+Right", "Forward"),
                ("Alt+Up", "Parent folder"),
                ("Alt+Home", "Home folder"),
                ("Ctrl+L", "Edit the path"),
                ("Ctrl+F", "Filter"),
                ("Ctrl+Shift+F", "Search in subfolders"),
                ("a–z", "Jump to a name"),
                ("Ctrl+C / Ctrl+X / Ctrl+V", "Copy, cut, paste"),
                ("Ctrl+Alt+C", "Copy Path"),
                ("F2", "Rename"),
                ("Del", "Move to Trash"),
                ("Shift+Del", "Delete permanently"),
                ("Ctrl+Shift+N", "New Folder"),
                ("Ctrl+Alt+N", "New Text File"),
                ("Ctrl+D", "Add to Bookmarks"),
                ("Ctrl+A", "Select All"),
                ("Ctrl+H", "Hidden files"),
                ("Ctrl+1 / Ctrl+2", "Details or icons"),
                ("Ctrl+Wheel / Ctrl+± / Ctrl+0", "Zoom"),
                ("F4", "Open Terminal Here"),
                ("F5", "Refresh"),
                ("Ctrl+S", "Save (in the text editor)"),
                ("Esc / Ctrl+W", "Close the text editor"),
                ("Ctrl+F / F3", "Find in the text editor"),
                ("Ctrl+E", "Markdown preview"),
                ("?", "Keyboard shortcuts"),
                ("Ctrl+Q", "Quit"),
            ] {
                ui.label(RichText::new(keys).color(p.accent));
                ui.label(RichText::new(tr(what)).color(p.fg));
                ui.end_row();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{PointerButton, Pos2, RawInput};

    fn frame(ctx: &egui::Context, app: &mut App, events: Vec<Event>) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 600.0))),
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            app.poll(ui.ctx());
            app.draw(ui);
        });
        // There is no GPU here to hand font textures to.
        out.textures_delta.clear();
    }

    fn pointer(pos: Pos2, pressed: Option<bool>) -> Vec<Event> {
        let mut events = vec![Event::PointerMoved(pos)];
        if let Some(pressed) = pressed {
            events.push(Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
        }
        events
    }

    /// Runs frames long enough for background folder loads (and any fallback they trigger) to land.
    fn settle(ctx: &egui::Context, app: &mut App) {
        for _ in 0..30 {
            std::thread::sleep(Duration::from_millis(10));
            frame(ctx, app, vec![]);
        }
    }

    #[test]
    fn a_folder_that_vanishes_falls_back_to_its_parent() {
        let dir = std::env::temp_dir().join(format!("ferry-vanish-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::write(dir.join("a/b/file"), "x").unwrap();
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, dir.join("a/b"), None);
        settle(&ctx, &mut app);
        assert_eq!(app.cwd, dir.join("a/b"));

        std::fs::remove_dir_all(dir.join("a/b")).unwrap();
        app.refresh();
        settle(&ctx, &mut app);
        assert_eq!(app.cwd, dir.join("a"));
        assert_eq!(app.entries.len(), 0);
    }

    #[test]
    fn dragging_a_folder_onto_the_sidebar_adds_it_to_places() {
        let dir = std::env::temp_dir().join(format!("ferry-dnd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("folder/Invoices")).unwrap();
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, dir.join("folder"), None);
        app.bookmarks_file = dir.join("bookmarks");
        app.bookmarks.clear();
        frame(&ctx, &mut app, vec![]);

        // Press on the only row, drag it over the sidebar, and let go there.
        let row = (0..600)
            .step_by(4)
            .map(|y| Pos2::new(600.0, y as f32))
            .find(|&pos| {
                frame(&ctx, &mut app, pointer(pos, Some(true)));
                frame(&ctx, &mut app, pointer(pos, Some(false)));
                !app.selected.is_empty()
            });
        let row = row.expect("found the row");
        frame(&ctx, &mut app, pointer(row, Some(true)));
        for x in (100..600).rev().step_by(50) {
            frame(&ctx, &mut app, pointer(Pos2::new(x as f32, row.y), None));
        }
        assert!(
            egui::DragAndDrop::payload::<Dragged>(&ctx).is_some(),
            "drag started"
        );
        frame(&ctx, &mut app, pointer(Pos2::new(80.0, 500.0), None));
        frame(&ctx, &mut app, pointer(Pos2::new(80.0, 500.0), Some(false)));
        frame(&ctx, &mut app, vec![]);

        assert_eq!(
            app.bookmarks,
            vec![("Invoices".to_owned(), dir.join("folder/Invoices"))]
        );
        assert!(
            std::fs::read_to_string(dir.join("bookmarks"))
                .unwrap()
                .ends_with("/folder/Invoices\n")
        );
    }
}

#[cfg(test)]
mod zoom_tests {
    use super::*;

    #[test]
    fn zoom_is_per_view_clamped_and_resets() {
        let ctx = egui::Context::default();
        let dir = std::env::temp_dir();
        let mut app = App::new(&ctx, dir, None);
        app.zoom_by(2.0);
        assert_eq!(app.zoom_list, 2.0);
        assert_eq!(app.zoom_grid, 1.0);
        app.zoom_by(10.0);
        assert_eq!(app.zoom_list, 2.5);
        app.grid = true;
        let small = app.tile_size();
        app.zoom_by(10.0);
        assert_eq!(app.zoom_grid, 4.0);
        assert!(app.tile_size().x > small.x * 3.0 && app.tile_size().y > small.y * 2.0);
        app.zoom_by(0.0);
        assert_eq!(app.zoom_grid, 1.0);
        assert_eq!(app.tile_size(), small);
    }
}

#[cfg(test)]
mod settings_tests {
    use super::*;

    #[test]
    fn settings_survive_a_restart() {
        let dir = std::env::temp_dir().join(format!("ferry-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("ferry/settings.json");
        let ctx = egui::Context::default();

        let mut app = App::new(&ctx, std::env::temp_dir(), None).with_settings(file.clone());
        assert!(!file.exists(), "nothing written until something changes");
        app.grid = true;
        app.zoom_grid = 2.5;
        app.show_hidden = true;
        app.sort = SortKey::Modified;
        app.ascending = false;
        app.save_settings();

        let again = App::new(&ctx, std::env::temp_dir(), None).with_settings(file.clone());
        assert!(again.grid && again.show_hidden && !again.ascending);
        assert_eq!(
            (again.zoom_grid, again.zoom_list, again.sort),
            (2.5, 1.0, SortKey::Modified)
        );

        // A broken or hand-edited file falls back to defaults rather than failing.
        std::fs::write(&file, "{\"zoom_grid\": 99, \"sort\": \"bogus\"").unwrap();
        let broken = App::new(&ctx, std::env::temp_dir(), None).with_settings(file);
        assert_eq!((broken.zoom_grid, broken.sort), (1.0, SortKey::Name));
    }
}

#[cfg(test)]
mod editor_tests {
    use super::*;

    fn settle(ctx: &egui::Context, app: &mut App) {
        for _ in 0..30 {
            std::thread::sleep(Duration::from_millis(10));
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                app.poll(ui.ctx());
                app.draw(ui);
            });
            out.textures_delta.clear();
        }
    }

    #[test]
    fn text_files_open_edit_and_save_in_ferry() {
        let dir = std::env::temp_dir().join(format!("ferry-edit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.md"), "# Notes\n").unwrap();
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, dir.clone(), None);
        settle(&ctx, &mut app);

        app.open(dir.join("notes.md"));
        settle(&ctx, &mut app);
        let editor = app.editor.as_mut().expect("opened in the editor");
        assert_eq!(editor.text, "# Notes\n");
        editor.text.push_str("- milk\n");

        // Leaving with unsaved changes asks instead of closing.
        app.request_close_editor();
        assert!(matches!(app.dialog, Some(Dialog::Unsaved { quit: false })));
        app.dialog = None;

        app.after_save = Some(AfterSave::Close);
        app.save_editor(false);
        settle(&ctx, &mut app);
        assert_eq!(
            std::fs::read_to_string(dir.join("notes.md")).unwrap(),
            "# Notes\n- milk\n"
        );
        assert!(app.editor.is_none(), "closed after saving");
    }
}

#[cfg(test)]
mod search_tests {
    use super::*;

    fn settle(ctx: &egui::Context, app: &mut App) {
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(10));
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                app.poll(ui.ctx());
                app.draw(ui);
            });
            out.textures_delta.clear();
        }
    }

    #[test]
    fn search_finds_names_and_contents_and_opens_a_line() {
        let dir = std::env::temp_dir().join(format!("ferry-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("trips/lisbon")).unwrap();
        std::fs::write(
            dir.join("trips/lisbon/plan.md"),
            "# Plan\n\nVisit Belem\nEat pasteis\n",
        )
        .unwrap();
        std::fs::write(dir.join("todo.txt"), "call mum\n").unwrap();
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, dir.clone(), None);
        settle(&ctx, &mut app);

        // The filter's text carries over into the search.
        app.filter = "plan".into();
        app.apply(Action::Search);
        settle(&ctx, &mut app);
        let s = app.search.as_ref().unwrap();
        assert_eq!(
            s.hits.first().map(|h| h.path.clone()),
            Some(dir.join("trips/lisbon/plan.md"))
        );

        let s = app.search.as_mut().unwrap();
        s.text = "pasteis".into();
        s.mode = search::Mode::Contents;
        s.typed = Some(0.0);
        settle(&ctx, &mut app);
        let s = app.search.as_ref().unwrap();
        assert_eq!(s.hits.len(), 1);
        assert_eq!(s.hits[0].lines[0].number, 4);

        // Opening the line shows the file at that line, with the word found.
        let row = App::search_rows(s)[1];
        let action = App::search_action(s, row);
        app.apply(action);
        settle(&ctx, &mut app);
        let editor = app.editor.as_ref().expect("opened in the editor");
        assert_eq!(editor.found.first(), Some(&(24..31)));
        assert_eq!(editor.current, Some(0));
        assert!(!editor.preview, "opened at a line, so it shows the source");

        // Closing the file returns to the results; navigating away ends the search.
        app.apply(Action::CloseEditor);
        assert!(app.search.is_some());
        app.apply(Action::Up);
        assert!(app.search.is_none());
    }
}

#[cfg(test)]
mod appearance_tests {
    use super::*;
    use egui::accesskit::Toggled;
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};

    #[test]
    fn appearance_stays_open_while_its_checkboxes_change() {
        let ctx = egui::Context::default();
        let dir = std::env::temp_dir();
        let mut app = App::new(&ctx, dir, None);
        // A desktop text scale that isn't on the slider's 5% steps, as on Omarchy's 4K setup.
        app.theme.set_text_scale(1.6364);
        // Icons come from the desktop's Nerd Font, which a test machine may lack.
        let mut harness = Harness::builder()
            .allow_missing_glyphs()
            .build_ui(move |ui| {
                app.appearance_button(ui);
                ui.label(format!("scale set: {}", app.ui_scale.is_some()));
            });
        harness.get_by_label("scale set: false");
        harness.get_by_label("\u{f1de}").click();
        harness.run();
        harness.get_by_label("scale set: false");
        let follow = harness.get_by_label("Follow the desktop\x27s text size");
        assert_eq!(follow.accesskit_node().toggled(), Some(Toggled::True));
        follow.click();
        harness.run();
        // The menu is still open, the box is unticked, and the size is now set by hand.
        let follow = harness.get_by_label("Follow the desktop\x27s text size");
        assert_eq!(follow.accesskit_node().toggled(), Some(Toggled::False));
        harness.get_by_label("scale set: true");
        follow.click();
        harness.run();
        harness.get_by_label("scale set: false");
    }
}
