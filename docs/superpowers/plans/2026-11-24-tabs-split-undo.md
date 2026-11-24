# Tabs, Split View and Undo Implementation Plan

> **For agentic workers:** Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task by task. Steps use checkbox
> syntax (`- [ ]`) for tracking.

**Goal:** Ferry gets tabs, an optional split view and undo/redo — the three things the
README's "Known limitations" still lists as missing.

**Architecture:** The ~20 per-view fields on `App` move into a new `Pane` struct; a tab is a
`Pane`, and the split's second pane is another `Pane` that dies with the split. Undo/redo is a
window-level stack of `Op` values captured at the five places where Ferry changes the disk,
reversed by redoing the same operations in the opposite order with pre-checks.

**Tech Stack:** Rust, eframe/egui 0.37 (no new dependencies), existing `files.rs` helpers,
`tr()`/`trn()` i18n, app-flow tests with the existing `settle` pattern.

## Global Constraints

- **No new crates.** `egui_frames` pins egui ^0.36 and Ferry is on 0.37; the tab strip and
  split layout are hand-rolled in `app.rs`.
- **Every visible string** goes through `tr("English text")` or `trn("%n items", n)` with
  `%1`/`%n` placeholders, and each new string is added to `src/i18n/es.json` (counted strings
  as `{"one": …, "other": …}`, also in `en.json` when counted). A test fails if a catalog
  doesn't parse.
- **Never lose data.** Undo reverses with pre-checks: the item must still exist and the
  destination must be free; otherwise refuse with a status message. Never overwrite.
- **Colors** come from `self.theme.palette` (`p.fg`, `p.dim`, `p.accent`, `p.red`…), never
  hard-coded. Corners from the theme's rounding.
- **The UI thread never blocks on the disk.** Undo reversals that touch the disk go through
  `self.job(label, || -> Result<String, String>)`; synchronous pre-checks (`exists()`) are
  fine because they're single stat calls.
- **Settings** persist only what they do today (grid, show_hidden, zooms, sort). Tabs and
  split state are NOT persisted — the user chose no tab persistence.
- **Commits:** short imperative subject, no AI attribution lines. Before every commit run
  `cargo fmt`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`
  and fix everything. Update `CHANGELOG.md` under *Unreleased* for user-visible changes.

## Review Focus

The Pane extraction (Task 1) is a ~190-reference mechanical refactor of a 5k-line file; the
risk is a missed reference or a borrow that no longer works, not a design error. Verify by
compiling and running the full existing test suite — every existing app-flow test must pass
unchanged (they read `app.cwd`, `app.selected` etc.; those become `app.active().cwd` via
Deref, so tests keep compiling).

---

### Task 1: Extract `Pane` from `App` (mechanical refactor)

**Files:**
- Modify: `src/app.rs` (struct `App` ~line 276, `App::new` ~358, every method that touches the moved fields)

**Interfaces:**
- Consumes: nothing new.
- Produces: `pub struct Pane { … }` with a `Pane::new(start: PathBuf) -> Self` constructor;
  `App.tabs: Vec<Pane>`, `App.active_tab: usize`; `impl Deref/DerefMut for App` targeting the
  active pane so existing code and tests keep compiling.

The fields that move into `Pane` (exactly these, in this order):
`cwd`, `back`, `forward`, `entries`, `view`, `dir_mtime`, `free`, `show_hidden`, `sort`,
`ascending`, `grid`, `zoom_list`, `zoom_grid`, `filter`, `focus_filter`, `path_edit`,
`selected`, `cursor`, `anchor`, `scroll_to_cursor`, `viewport`, `jump`.

Window-level fields that STAY on `App`: theme, language, settings (file/json/saved), ui_scale,
opacity, scale_edit, compress_format, places, bookmarks, devices, scanning_devices, dialog,
status, clip, busy, tx/rx, pending, load_tx/load_rx, load_gen, loading, checking, dnd,
editor, search + search channels/progress/id, thumbs, allow_close, last_poll.

- [ ] **Step 1: Write the failing test** (pins the shape of the refactor — tests must keep
      compiling and passing with zero changes to their bodies).

There is no new behavior here; the guard is that the existing suite compiles unchanged. Add
one small new test at the end of `app.rs` in a new module so the extraction has its own check:

```rust
#[cfg(test)]
mod pane_tests {
    use super::*;

    #[test]
    fn app_derefs_to_the_active_pane() {
        let dir = std::env::temp_dir().join(format!("ferry-pane-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, dir.clone(), None);
        // Deref: reading a pane field through the App works.
        assert_eq!(app.cwd, dir);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.active_tab, 0);
    }
}
```

- [ ] **Step 2: Run to verify it fails** — `bin/test pane_tests` → compile error
      (`Pane` doesn't exist yet).

- [ ] **Step 3: Implement.** In `src/app.rs`:

1. Define the struct above `impl App`, after the `Loaded` enum:

```rust
/// One view of a folder: its own place, history, selection and scroll position. A tab is a
/// Pane; the split's second pane is another one that dies with the split.
struct Pane {
    cwd: PathBuf,
    back: Vec<PathBuf>,
    forward: Vec<PathBuf>,
    entries: Vec<Entry>,
    view: Vec<usize>,
    dir_mtime: Option<SystemTime>,
    free: Option<u64>,
    show_hidden: bool,
    sort: SortKey,
    ascending: bool,
    grid: bool,
    zoom_list: f32,
    zoom_grid: f32,
    filter: String,
    focus_filter: bool,
    path_edit: Option<String>,
    selected: HashSet<PathBuf>,
    cursor: Option<usize>,
    anchor: Option<usize>,
    scroll_to_cursor: bool,
    viewport: (f32, f32),
    jump: (String, f64),
}

impl Pane {
    fn new(start: PathBuf) -> Self {
        Pane {
            cwd: start,
            back: Vec::new(),
            forward: Vec::new(),
            entries: Vec::new(),
            view: Vec::new(),
            dir_mtime: None,
            free: None,
            show_hidden: false,
            sort: SortKey::Name,
            ascending: true,
            grid: false,
            zoom_list: 1.0,
            zoom_grid: 1.0,
            filter: String::new(),
            focus_filter: false,
            path_edit: None,
            selected: HashSet::new(),
            cursor: None,
            anchor: None,
            scroll_to_cursor: false,
            viewport: (0.0, 0.0),
            jump: (String::new(), 0.0),
        }
    }
}
```

2. In `App`, delete the 22 moved fields and add:

```rust
    tabs: Vec<Pane>,
    active_tab: usize,
```

3. In `App::new`, replace the 22 field initializers with
   `tabs: vec![Pane::new(start.clone())], active_tab: 0,`. The settings-loading code that
   overwrites `grid`/`show_hidden`/`zoom_list`/`zoom_grid`/`sort`/`ascending` after the struct
   literal (it reads `settings.get("grid")` etc.) must now write into
   `app.tabs[0]` — e.g. `if let Some(v) = settings.get("grid").and_then(|v| v.as_bool()) { app.tabs[0].grid = v; }`.

4. Add Deref so every existing `self.cwd`, `self.selected`, … reference keeps working:

```rust
impl std::ops::Deref for App {
    type Target = Pane;
    fn deref(&self) -> &Pane {
        &self.tabs[self.active_tab]
    }
}
impl std::ops::DerefMut for App {
    fn deref_mut(&mut self) -> &mut Pane {
        &mut self.tabs[self.active_tab]
    }
}
```

   Note: methods that need `&mut self` on the App-level fields while also writing a pane field
   are fine — Deref only kicks in when the field isn't found on `App`. If the compiler
   complains about ambiguous `self.x` where `x` exists on both, qualify it as
   `self.tabs[self.active_tab].x`.

5. Fix every place that constructed or reset pane state wholesale: search for
   `cwd:` initializers and `self.cwd =` resets outside `Pane::new`; the `leave()`/`go()`
   history pushes already work through DerefMut.

- [ ] **Step 4: Run the full suite** — `bin/test`. All existing tests pass unchanged, plus
      `pane_tests::app_derefs_to_the_active_pane`. If a test body needed editing to add
      `.tabs[0]`, that's allowed only where it reads `app.tabs`/`app.active_tab` (new fields);
      reading `app.cwd` must NOT need changing.

- [ ] **Step 5: Lint and commit**

```sh
cargo fmt && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked
git add src/app.rs
git commit -m "Extract the per-view state into a Pane"
```


---

### Task 2: Tabs — open, close, switch, tab strip

**Files:**
- Modify: `src/app.rs` (`Action` enum ~81, `apply` ~1758, `keys`/`key` ~2136/2191, `draw` ~3939, `help()` ~4604)
- Modify: `src/i18n/es.json`, `src/i18n/en.json`

**Interfaces:**
- Consumes: `Pane` (Task 1).
- Produces: `Action::NewTab`, `Action::CloseTab`, `Action::NextTab`, `Action::PrevTab`;
  `fn tab_strip(&mut self, ui: &mut Ui)`; keys Ctrl+T / Ctrl+W / Ctrl+Tab / Ctrl+Shift+Tab.

New strings (add to es.json; en.json only if counted — none are):
- `"New Tab"` → `"Pestaña nueva"`
- `"Close Tab"` → `"Cerrar pestaña"`
- `"Next Tab"` → `"Pestaña siguiente"`
- `"Previous Tab"` → `"Pestaña anterior"`

- [ ] **Step 1: Write the failing tests** — new module at the end of `app.rs` (copy the
      `settle` helper from an existing module):

```rust
#[cfg(test)]
mod tabs_tests {
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
    fn tabs_open_close_and_switch() {
        let dir = std::env::temp_dir().join(format!("ferry-tabs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::create_dir_all(dir.join("b")).unwrap();
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, dir.clone(), None);
        settle(&ctx, &mut app);

        // A new tab opens the same folder and becomes active.
        app.apply(Action::NewTab);
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active_tab, 1);
        assert_eq!(app.cwd, dir);

        // Each tab keeps its own place: move the first, the second stays put.
        app.apply(Action::PrevTab);
        app.apply(Action::Go(dir.join("a")));
        settle(&ctx, &mut app);
        assert_eq!(app.tabs[0].cwd, dir.join("a"));
        app.apply(Action::NextTab);
        assert_eq!(app.cwd, dir, "the second tab never moved");

        // Closing a non-last tab keeps the window; closing the last one quits.
        app.apply(Action::CloseTab);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.active_tab, 0);
        assert_eq!(app.cwd, dir.join("a"), "the surviving tab is the first");

        // Closing the only tab closes the window (allow_close is set).
        app.apply(Action::CloseTab);
        assert!(app.allow_close, "last tab closed the window");
    }
}
```

- [ ] **Step 2: Run to verify it fails** — `bin/test tabs_tests` → compile error
      (`Action::NewTab` doesn't exist).

- [ ] **Step 3: Implement.** In `src/app.rs`:

1. Add the four variants to `Action` (after `CloseEditor`, before the closing brace):

```rust
    NewTab,
    CloseTab,
    NextTab,
    PrevTab,
```

2. Handle them in `apply` (a new arm near the top of the match, before folder actions):

```rust
            Action::NewTab => {
                self.tabs.insert(self.active_tab + 1, Pane::new(self.cwd.clone()));
                self.active_tab += 1;
                self.load();
            }
            Action::CloseTab => {
                if self.tabs.len() == 1 {
                    return self.quit();
                }
                let was = self.active_tab;
                self.tabs.remove(was);
                if self.active_tab >= self.tabs.len() {
                    self.active_tab -= 1;
                } else if was < self.active_tab {
                    // removed a tab before the active one: it shifted down
                    self.active_tab -= 1;
                }
                self.load();
            }
            Action::NextTab => {
                self.active_tab = (self.active_tab + 1) % self.tabs.len();
                self.load();
            }
            Action::PrevTab => {
                self.active_tab = (self.active_tab + self.tabs.len() - 1) % self.tabs.len();
                self.load();
            }
```

   `load()` must be made pane-aware in Task 3; for now it reads through Deref and works.

3. Keys: in `key()` add to the match (these work even while a text field has focus, so put
   them with the Ctrl+Q/L/F/I set in `keys()`, not in `key()`):

```rust
            Key::T if m.ctrl => return self.pending.push(Action::NewTab),
            Key::W if m.ctrl => return self.pending.push(Action::CloseTab),
            Key::Tab if m.ctrl && !m.shift => return self.pending.push(Action::NextTab),
            Key::Tab if m.ctrl && m.shift => return self.pending.push(Action::PrevTab),
```

   (Match the exact style of the existing always-active set — read `keys()` first.)

4. The tab strip, drawn in `draw()` between the toolbar panel and the central area, only when
   there is more than one tab OR a split is on (Task 4 adds the split condition):

```rust
    /// The row of tabs under the toolbar; each shows its folder's name and closes with ×.
    fn tab_strip(&mut self, ui: &mut Ui) {
        let p = self.p();
        ui.horizontal(|ui| {
            for (i, tab) in self.tabs.iter().enumerate() {
                let active = i == self.active_tab;
                let name = tab.cwd.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| tab.cwd.display().to_string());
                let mut btn = egui::Button::new(RichText::new(name).weak(!active))
                    .min_size(egui::vec2(0.0, 24.0));
                if ui.add(btn).clicked() {
                    self.active_tab = i;
                    self.load();
                }
                // The close button is its own widget so it doesn't steal the tab's click.
                let close = RichText::new("\u{2715}").size(10.0).color(p.dim);
                if ui.add(egui::Button::new(close).frame(false)).clicked() {
                    self.pending.push(Action::CloseTab);
                    break;
                }
            }
        });
    }
```

   In `draw()`, after the toolbar `Panel::top` block:

```rust
        if self.tabs.len() > 1 {
            Panel::top("tabs")
                .frame(egui::Frame::new().fill(self.glass(p.bg)).inner_margin(Margin::symmetric(8, 2)))
                .show_separator_line(false)
                .show(ui, |ui| self.tab_strip(ui));
        }
```

5. `help()`: add the four shortcuts to the list (Ctrl+T New Tab, Ctrl+W Close Tab,
   Ctrl+Tab Next Tab, Ctrl+Shift+Tab Previous Tab), matching the existing row format.

6. i18n: add the four strings to `src/i18n/es.json`.

- [ ] **Step 4: Run tests** — `bin/test tabs_tests` passes; then full `bin/test`.

- [ ] **Step 5: Lint and commit**

```sh
cargo fmt && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked
git add src/app.rs src/i18n/es.json
git commit -m "Add tabs"
```


---

### Task 3: Make loading, checks and refresh pane-aware

**Files:**
- Modify: `src/app.rs` (`load()`, `loaded()` ~700s, `poll()` ~3743, the mtime check spawn ~3850, `refresh()`, `reload()`)

**Interfaces:**
- Consumes: `Pane` (Task 1).
- Produces: per-pane `loading: Option<PathBuf>` and `load_gen: u64` on `Pane`; `Loaded::Dir(Load)`
  and `Loaded::Check { path, … }` routed to the pane whose `cwd` matches.

Two panes load independently, so the single App-level generation counter can't stay global:
a stale result for tab 1 must not clobber tab 2's listing. The fix is per-pane state plus
routing by path (which every `Loaded` variant already carries).

- [ ] **Step 1: Write the failing test** — add to `tabs_tests`:

```rust
    #[test]
    fn panes_load_independently() {
        let dir = std::env::temp_dir().join(format!("ferry-tabload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::create_dir_all(dir.join("b")).unwrap();
        std::fs::write(dir.join("a/x.txt"), "x").unwrap();
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, dir.clone(), None);
        settle(&ctx, &mut app);

        app.apply(Action::NewTab);
        // Tab 1 goes to b while tab 0 stays in the root.
        app.apply(Action::Go(dir.join("b")));
        settle(&ctx, &mut app);
        assert_eq!(app.tabs[1].cwd, dir.join("b"));
        assert!(app.tabs[1].entries.iter().any(|e| e.name == "x.txt") == false);
        assert_eq!(app.tabs[0].cwd, dir);

        // Both panes show their own listing at the same time.
        app.apply(Action::PrevTab);
        let names: Vec<String> = app.view.iter().map(|&i| app.entries[i].name.clone()).collect();
        assert!(names.contains(&"a".to_string()) && names.contains(&"b".to_string()));
    }
```

- [ ] **Step 2: Run to verify it fails** — `bin/test tabs_tests::panes_load_independently`
      (fails because a second pane's load overwrites the first's entries, or the single
      `loading` slot blocks one of them).

- [ ] **Step 3: Implement.** In `src/app.rs`:

1. Move `load_gen: u64` and `loading: Option<PathBuf>` from `App` into `Pane` (add to the
   struct + `Pane::new`: `load_gen: 0, loading: None`).

2. `load()` (the fn that spawns the listing thread): it already captures `self.cwd.clone()`
   and bumps `self.load_gen` — through DerefMut these now act on the active pane, which is
   exactly right. Verify it compares against `self.loading` (now per-pane) so two panes can
   load at once.

3. `loaded()` / the `Loaded::Dir(Load)` arm in `poll()`: route by path — find the pane whose
   `cwd == load.path` and whose `load_gen` matches, apply to it; drop stale results silently:

```rust
                Loaded::Dir(load) => {
                    let Some(i) = self.tabs.iter().position(|t| t.cwd == load.path) else {
                        return; // the tab moved on or closed
                    };
                    if self.tabs[i].load_gen != load.gen {
                        return; // a newer load for that pane superseded this one
                    }
                    let t = &mut self.tabs[i];
                    t.loading = None;
                    t.entries = load.entries;
                    t.view.clear();
                    t.selected.retain(|p| t.entries.iter().any(|e| e.path == *p));
                    t.cursor = None;
                    t.dir_mtime = load.mtime;
                    t.free = load.free;
                    // (match the field names of the real Load struct — read it first)
                }
```

4. `Loaded::Check { path, mtime, free }` (the once-a-second watcher result): same routing —
   apply to the pane whose `cwd == path`; clear that pane's `checking`. The spawn site
   (~3850, the per-second check in `poll`) must run for EVERY tab (and the split pane), not
   just the active one: loop over panes and skip ones already checking.

5. `refresh()`/`reload()`: they read through DerefMut and stay as-is; confirm they bump the
   right pane's `load_gen`.

- [ ] **Step 4: Run tests** — `bin/test tabs_tests` passes, full `bin/test` green.

- [ ] **Step 5: Lint and commit**

```sh
cargo fmt && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked
git add src/app.rs
git commit -m "Load each pane independently"
```

---

### Task 4: Split view (F3 toggle, F6 switch, shared sidebar)

**Files:**
- Modify: `src/app.rs` (`Action`, `apply`, `keys`/`key`, `draw`, `folder()`, `status_bar()`, `help()`)
- Modify: `src/i18n/es.json`

**Interfaces:**
- Consumes: `Pane` (Task 1), pane-aware loading (Task 3).
- Produces: `App.split: Option<Pane>`, `App.active_pane: bool` (`false` = left/tab, `true` =
  right/split); `Action::ToggleSplit`, `Action::SwitchPane`; keys F3, F6.

Model: when the split is on, the RIGHT pane is a separate `Pane` that starts as a copy of the
active tab's cwd (fresh history/selection). The LEFT side is always the active tab. F3 off
drops the right pane entirely. One sidebar; its bookmarks act on the active pane (DerefMut
already targets it once `active_pane` is honored — see step 3).

New strings: `"Toggle Split View"` → `"Alternar vista dividida"`,
`"Switch Pane"` → `"Cambiar de panel"`.

- [ ] **Step 1: Write the failing tests** — new module (copy `settle`):

```rust
#[cfg(test)]
mod split_tests {
    use super::*;

    fn settle(ctx: &egui::Context, app: &mut App) { /* same as tabs_tests */ }

    #[test]
    fn split_toggles_and_switches() {
        let dir = std::env::temp_dir().join(format!("ferry-split-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::create_dir_all(dir.join("b")).unwrap();
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, dir.clone(), None);
        settle(&ctx, &mut app);

        // F3 opens the split; the right pane starts on the same folder.
        app.apply(Action::ToggleSplit);
        assert!(app.split.is_some());
        assert_eq!(app.split.as_ref().unwrap().cwd, dir);
        assert!(!app.active_pane, "the left (tab) pane stays active");

        // F6 moves activity to the right pane; navigating it leaves the tab alone.
        app.apply(Action::SwitchPane);
        assert!(app.active_pane);
        app.apply(Action::Go(dir.join("b")));
        settle(&ctx, &mut app);
        assert_eq!(app.split.as_ref().unwrap().cwd, dir.join("b"));
        assert_eq!(app.tabs[app.active_tab].cwd, dir, "the tab pane never moved");

        // F3 again closes the split and drops the right pane.
        app.apply(Action::SwitchPane);
        app.apply(Action::ToggleSplit);
        assert!(app.split.is_none());
        assert!(!app.active_pane);
        assert_eq!(app.cwd, dir);
    }
}
```

- [ ] **Step 2: Run to verify it fails** — `bin/test split_tests` → compile error.

- [ ] **Step 3: Implement.** In `src/app.rs`:

1. On `App`: `split: Option<Pane>`, `active_pane: bool`. Init in `App::new`:
   `split: None, active_pane: false`.

2. **The core of the task — make Deref honor the split.** Replace Task 1's Deref with a
   helper and point Deref at it:

```rust
    /// The pane that receives keys, clicks and sidebar actions: the split's right pane when
    /// it's active, otherwise the active tab.
    fn active_pane_index(&self) -> Option<usize> { None } // placeholder, replaced below

impl std::ops::Deref for App {
    type Target = Pane;
    fn deref(&self) -> &Pane {
        if self.active_pane && self.split.is_some() {
            self.split.as_ref().unwrap()
        } else {
            &self.tabs[self.active_tab]
        }
    }
}
impl std::ops::DerefMut for App {
    fn deref_mut(&mut self) -> &mut Pane {
        if self.active_pane && self.split.is_some() {
            self.split.as_mut().unwrap()
        } else {
            &mut self.tabs[self.active_tab]
        }
    }
}
```

   (Drop the placeholder helper — it's only here to show intent; the two Deref impls ARE the
   change.)

3. `Action::ToggleSplit` / `Action::SwitchPane` in `apply`:

```rust
            Action::ToggleSplit => {
                if let Some(mut pane) = self.split.take() {
                    // Closing: the right pane's place is gone with it.
                    let _ = &mut pane;
                    self.active_pane = false;
                } else {
                    self.split = Some(Pane::new(self.cwd.clone()));
                    self.split.as_mut().unwrap().load_gen = 0;
                    self.load(); // loads the new right pane (it's not active yet)
                    // load() targets the ACTIVE pane, so instead:
                }
            }
```

   NOTE — `load()` acts on the Deref target (the active tab), but opening the split must load
   the NEW right pane. So give `load` an explicit target or add a small
   `fn load_pane(&mut self, i: usize)` / `fn load_split(&mut self)`. Simplest: factor the body
   of `load()` into `fn load_into(&mut self, pane: &mut Pane)` and have both call sites use it.
   Do that here (it also makes Task 3's routing cleaner).

```rust
            Action::ToggleSplit => {
                if self.split.is_some() {
                    self.split = None;
                    self.active_pane = false;
                } else {
                    let mut pane = Pane::new(self.cwd.clone());
                    self.load_into(&mut pane);
                    self.split = Some(pane);
                }
            }
            Action::SwitchPane => {
                if self.split.is_some() {
                    self.active_pane = !self.active_pane;
                }
            }
```

4. Keys: F3 and F6 in `key()` (they must NOT fire while the editor is open — `keys()` already
   checks the editor first, so plain `key()` placement is correct):

```rust
            Key::F3 => return self.pending.push(Action::ToggleSplit),
            Key::F6 => return self.pending.push(Action::SwitchPane),
```

5. Draw: in `draw()`, when `self.split.is_some()`, split the central area into two side-by-side
   halves, each calling `self.folder(ui)` for its pane. `folder()` reads through DerefMut, so
   it must be called with the RIGHT pane active — wrap each half:

```rust
        if let Some(_) = self.split.as_ref() {
            ui.horizontal(|ui| {
                // Left half: force the tab pane.
                let saved = std::mem::replace(&mut self.active_pane, false);
                let left = ui.available_width() / 2.0;
                egui::SidePanel::middle("split-left").exact_size(left).show_inside(ui, |ui| {
                    self.folder(ui);
                });
                let _ = saved;
                // Right half: force the split pane.
                self.active_pane = true;
                self.folder(ui);
            });
        } else {
            self.folder(ui);
        }
```

   (If `SidePanel::middle` fights the layout, use two `ui.allocate_rect` halves instead — pick
   whichever compiles cleanly and keeps each pane's scroll area independent. The requirement:
   two independent scrollable listings, one sidebar.)

6. Status bar shows the active pane's counts (already Deref-based — verify).

7. `help()`: add F3 Toggle Split View, F6 Switch Pane. i18n: the two strings to es.json.

- [ ] **Step 4: Run tests** — `bin/test split_tests` passes; full `bin/test` green.

- [ ] **Step 5: Lint and commit**

```sh
cargo fmt && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked
git add src/app.rs src/i18n/es.json
git commit -m "Add an optional split view"
```


---

### Task 5: Undo and redo

**Files:**
- Modify: `src/app.rs` (`Action`, `apply`, `confirm()` ~3618, the paste/copy/move/trash capture points, `keys()`, menus, `help()`)
- Modify: `src/files.rs` (make `paste` report what it actually did)
- Modify: `src/i18n/es.json`, `src/i18n/en.json`

**Interfaces:**
- Consumes: everything above.
- Produces:
  - `enum Op { Move{from: Vec<PathBuf>, to: Vec<PathBuf>}, Copy{…same…}, Rename{from, to}, CreateFolder(PathBuf), CreateFile(PathBuf), Trash(Vec<PathBuf>) }` — each variant stores the ACTUAL source and destination paths.
  - `App.undo: Vec<Op>`, `App.redo: Vec<Op>` (depth-capped at 20).
  - `Action::Undo`, `Action::Redo`; keys Ctrl+Z / Ctrl+Shift+Z.
  - `files::paste` returns the pairs it made, so undo knows the real destinations (it renames
    on clash via `unique_name`).

Design rules (from the approved spec): one user operation = one entry; reversing verifies the
item still exists AND the destination is free, else refuse with a status message (never
overwrite); all-or-nothing per entry; permanent delete is NOT undoable; menu items say
"Undo Move", "Undo Rename", etc.

New strings: `"Undo"` → `"Deshacer"`, `"Redo"` → `"Rehacer"`,
`"Undo Move"` → `"Deshacer mover"`, `"Undo Copy"` → `"Deshacer copiar"`,
`"Undo Rename"` → `"Deshacer renombrar"`, `"Undo New Folder"` → `"Deshacer carpeta nueva"`,
`"Undo New File"` → `"Deshacer archivo nuevo"`, `"Undo Delete"` → `"Deshacer eliminar"`,
`"Cannot undo: %1 already exists"` → `"No se puede deshacer: %1 ya existe"`,
`"Cannot undo: %1 is gone"` → `"No se puede deshacer: %1 ya no está"`.

- [ ] **Step 1: Make `files::paste` report what it did.** In `src/files.rs`, change the paste
      fn to return the pairs (read its current signature first — it takes sources + dest dir):

```rust
/// Copies or moves `paths` into `dest`; returns the (source, actual destination) pairs made.
/// Clashes get a new name through unique_name, so the returned destinations are the truth.
pub fn paste(paths: &[PathBuf], dest: &Path, move_: bool) -> Result<Vec<(PathBuf, PathBuf)>, String>
```

      Internally it already computes each destination via `unique_name`; collect those into a
      `Vec<(PathBuf, PathBuf)>` and return it. Update its one caller in `app.rs` (the paste
      job ~2119) to use the returned pairs — that's where the undo entry is captured (step 3).

- [ ] **Step 2: Write the failing tests** — new module (copy `settle`). Cover each op plus the
      refuse path and depth eviction:

```rust
#[cfg(test)]
mod undo_tests {
    use super::*;

    fn settle(ctx: &egui::Context, app: &mut App) { /* same as tabs_tests */ }

    #[test]
    fn undo_and_redo_a_move() {
        let dir = std::env::temp_dir().join(format!("ferry-undo-move-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("dst")).unwrap();
        std::fs::write(dir.join("src/f.txt"), "f").unwrap();
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, dir.clone(), None);
        settle(&ctx, &mut app);

        // Move src/f.txt into dst/ (select it, copy, go to dst, paste-move).
        app.apply(Action::Go(dir.join("src")));
        settle(&ctx, &mut app);
        app.apply(Action::SelectAll);
        app.apply(Action::Copy);
        app.apply(Action::Go(dir.join("dst")));
        settle(&ctx, &mut app);
        app.apply(Action::PasteMove); // use the real variant name for cut/paste-move
        settle(&ctx, &mut app);
        assert!(dir.join("dst/f.txt").exists());
        assert!(!dir.join("src/f.txt").exists());

        // Undo puts it back; redo moves it again.
        app.apply(Action::Undo);
        settle(&ctx, &mut app);
        assert!(dir.join("src/f.txt").exists());
        assert!(!dir.join("dst/f.txt").exists());
        app.apply(Action::Redo);
        settle(&ctx, &mut app);
        assert!(dir.join("dst/f.txt").exists());
    }

    #[test]
    fn undo_refuses_when_the_destination_is_taken() {
        let dir = std::env::temp_dir().join(format!("ferry-undo-refuse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("dst")).unwrap();
        std::fs::write(dir.join("src/f.txt"), "f").unwrap();
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, dir.clone(), None);
        settle(&ctx, &mut app);

        // (same move as above) … then occupy the original spot before undoing.
        // [perform the move exactly like the first test]
        std::fs::write(dir.join("src/f.txt"), "someone else").unwrap();
        app.apply(Action::Undo);
        settle(&ctx, &mut app);
        // Refused: the file stays where it is, and a status message was set.
        assert!(dir.join("dst/f.txt").exists());
        assert_eq!(std::fs::read_to_string(dir.join("src/f.txt")).unwrap(), "someone else");
        assert!(app.status.is_some());
    }

    #[test]
    fn undo_depth_is_capped_at_twenty() {
        let dir = std::env::temp_dir().join(format!("ferry-undo-depth-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx, dir.clone(), None);
        settle(&ctx, &mut app);
        for i in 0..21 {
            std::fs::write(dir.join(format!("n{i}.txt")), "x").unwrap();
            // create-then-… use the real "New File" confirm path or a rename; simplest:
            // rename n{i}.txt to m{i}.txt through Action + confirm.
        }
        assert!(app.undo.len() <= 20, "oldest entries fall off");
    }
}
```

      NOTE for the implementer: use the REAL action names and confirm-dialog flow (read
      `confirm()` ~3618 — Rename/NewFile/NewFolder are synchronous fs ops there; Trash is
      `Action::Trash` ~1791). The depth test may be simpler as 21 renames driven through
      `app.confirm` the way existing tests do. Keep the assertions: after 21 ops,
      `app.undo.len() == 20`.

- [ ] **Step 3: Run to verify it fails** — `bin/test undo_tests` → compile error (`Op`,
      `Action::Undo` don't exist).

- [ ] **Step 4: Implement.** In `src/app.rs`:

1. The `Op` enum (near `Loaded`):

```rust
/// One user operation, recorded so it can be reversed. Paths are the ACTUAL ones on disk.
enum Op {
    Move { from: Vec<PathBuf>, to: Vec<PathBuf> },
    Copy { from: Vec<PathBuf>, to: Vec<PathBuf> },
    Rename { from: PathBuf, to: PathBuf },
    CreateFolder(PathBuf),
    CreateFile(PathBuf),
    Trash(Vec<PathBuf>),
}
```

2. On `App`: `undo: Vec<Op>`, `redo: Vec<Op>` (init empty). Helper:

```rust
    /// Records an operation; a new one clears the redo stack, and the oldest falls off at 20.
    fn record(&mut self, op: Op) {
        self.redo.clear();
        self.undo.push(op);
        if self.undo.len() > 20 {
            self.undo.remove(0);
        }
    }
```

3. Capture points (five):
   - **Paste/copy/move** (~2119, the job that calls `files::paste`): after it succeeds with
     the returned pairs, `self.record(Op::Move{…})` or `Op::Copy{…}` depending on which was
     done. The job runs off-thread and ends in a status+refresh, so capture the pairs there
     and push the record on the UI thread (the job's result handler) — read how `job` reports
     back and add the Op to that path.
   - **Rename** (`confirm()` ~3620s): after the synchronous rename succeeds,
     `self.record(Op::Rename{from, to})`.
   - **New Folder / New File** (`confirm()`): after creation, `record(Op::CreateFolder(p))` /
     `Op::CreateFile(p)`.
   - **Trash** (`Action::Trash` ~1791): after the gio trash succeeds,
     `record(Op::Trash(paths))`.

4. Reversal in `apply`:

```rust
            Action::Undo => self.reverse(true),
            Action::Redo => self.reverse(false),
```

```rust
    /// Undoes (or redoes) the top operation, refusing if the disk no longer allows it.
    fn reverse(&mut self, undo: bool) {
        let op = if undo { self.undo.pop() } else { self.redo.pop() };
        let Some(op) = op else { return };
        // Verify before touching anything; refuse rather than overwrite.
        let (ok, message) = match &op {
            Op::Move { from, to } | Op::Copy { from, to } => {
                if undo {
                    // reverse: move each `to` back to `from`; `from` must be free, `to` present.
                    let bad_src = to.iter().find(|p| !p.exists());
                    let bad_dst = from.iter().find(|p| p.exists());
                    match (bad_src, bad_dst) {
                        (Some(p), _) => (false, tr("Cannot undo: %1 is gone").replace("%1", &p.display().to_string())),
                        (_, Some(p)) => (false, tr("Cannot undo: %1 already exists").replace("%1", &p.display().to_string())),
                        _ => (true, String::new()),
                    }
                } else { /* redo: `from` must be present, `to` free — mirror of the above */ }
            }
            Op::Rename { from, to } => { /* same shape, single paths */ }
            Op::CreateFolder(p) | Op::CreateFile(p) => {
                if undo { (p.exists(), String::new()) .into() } else { (!p.exists(), …) }
            }
            Op::Trash(paths) => (undo && paths.iter().all(|p| !p.exists()), …),
        };
        if !ok {
            // Put the op back so it isn't lost, and tell the user.
            if undo { self.undo.push(op); } else { self.redo.push(op); }
            return self.error(message);
        }
        // Perform the reverse on a job (disk work), then refresh.
        // [match each variant to its inverse: Move-undo = files::paste(move) to the parent of
        //  `from`; Copy-undo = delete the copies; Rename-undo = rename back; Create-undo =
        //  remove; Trash-undo = restore from trash via gio. Redo mirrors.]
    }
```

      Implement each variant's inverse using existing helpers (`files::paste` for move-back,
      `std::fs::remove_file`/`remove_dir_all` guarded by the pre-checks, gio trash restore the
      same way `trash()` uses gio but in reverse — read `files::trash` ~173 first).

5. Keys: Ctrl+Z / Ctrl+Shift+Z in the always-active set in `keys()` (undo should work even
   while a text field has focus, matching Ctrl+Q/L/F/I):

```rust
            Key::Z if m.ctrl && !m.shift => return self.pending.push(Action::Undo),
            Key::Z if m.ctrl && m.shift => return self.pending.push(Action::Redo),
```

6. Menus: in `item_menu`/`folder_menu` (or the toolbar, matching where Edit-style actions
   live) add "Undo <what>" and "Redo" items that are enabled only when the stack is non-empty;
   the label names the top op (`tr("Undo Move")` etc. chosen by `matches!(&self.undo.last()…)`).

7. `help()`: Ctrl+Z Undo, Ctrl+Shift+Z Redo. i18n: all new strings to es.json (counted ones
   also en.json — none here are counted).

- [ ] **Step 5: Run tests** — `bin/test undo_tests` passes; full `bin/test` green.

- [ ] **Step 6: Lint and commit**

```sh
cargo fmt && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked
git add src/app.rs src/files.rs src/i18n/es.json src/i18n/en.json
git commit -m "Add undo and redo"
```


---

### Task 6: Document it and close the README limitation

**Files:**
- Modify: `README.md` (line ~220 "No tabs, split view, or undo yet."; the keyboard-shortcuts section)
- Modify: `CHANGELOG.md` (Unreleased)

**Interfaces:** consumes nothing; produces user-facing docs.

- [ ] **Step 1:** In `README.md`, remove "No tabs, split view, or undo yet." from Known
      limitations (or rewrite the section if it becomes empty). Add the new keys to the
      keyboard-shortcuts table: Ctrl+T New Tab, Ctrl+W Close Tab, Ctrl+Tab / Ctrl+Shift+Tab
      switch tab, F3 Toggle Split View, F6 Switch Pane, Ctrl+Z Undo, Ctrl+Shift+Z Redo.

- [ ] **Step 2:** `CHANGELOG.md` under *Unreleased*: "Tabs (Ctrl+T/W), an optional split view
      (F3/F6) and undo/redo for move, copy, rename, new folder/file and delete-to-trash."

- [ ] **Step 3: Lint and commit**

```sh
cargo fmt && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked
git add README.md CHANGELOG.md
git commit -m "Document tabs, split view and undo"
```

---

## Self-Review Notes (for the implementer)

- Task 1 is pure refactor: the existing suite must pass with zero test-body changes except
  where a test now reads `app.tabs`/`app.active_tab`.
- Task 3's `load_into` factor-out should be done in Task 4 step 3 (opening the split needs it);
  if you do Task 3 first, keep `load()` as-is and only add per-pane routing.
- Undo capture for paste happens off-thread; make sure the Op is recorded on the UI thread in
  the job's result handler, not inside the spawned closure.
- The refuse-on-conflict test asserts a status message was set — that's the contract users see.

