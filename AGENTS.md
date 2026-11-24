# AGENTS.md

Instructions for coding agents working on Ferry. Humans: see [CONTRIBUTING.md](CONTRIBUTING.md),
which this file builds on.

Ferry is a keyboard-first, Dolphin-like file manager for [Omarchy](https://omarchy.org)
(Arch Linux + Hyprland), written in Rust with eframe/egui 0.37. It follows the Omarchy theme,
language and window opacity live. One binary, no runtime besides tools Arch already ships.

## Commands

```sh
cargo build                       # debug build: target/debug/ferry
cargo run -- <folder>             # run against a folder
bin/test                          # cargo test --locked (extra args pass through)
bin/build                         # release build
bin/install                       # release build + makepkg -fsi from pkgbuild/ (installs it)
```

Before every commit, run what CI runs (`.github/workflows/ci.yml`), and fix everything:

```sh
cargo fmt
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

Rust 1.95+, edition 2024: let-chains (`if let … && …`) are fine and used throughout.

## Layout

| File | What it owns |
| --- | --- |
| `src/main.rs` | Startup: window options (transparent), fonts, command-line path, `mod` list |
| `src/app.rs` | The window (~5k lines): `App`, `Action`, `Dialog`, `Loaded`, views, sidebar, keys, menus, dialogs, settings |
| `src/files.rs` | Everything that touches the disk: listing (`Entry`), sorting, paste, trash, bookmarks, `unique_name` |
| `src/archive.rs` | Compress/extract/list through `bsdtar` (and gzip/xz/zstd… for single files) |
| `src/props.rs` | Properties: file facts, folder usage, permissions, checksums |
| `src/portal.rs` | Folder picker via xdg-desktop-portal over D-Bus (zbus 5, blocking) |
| `src/devices.rs` | Drives and network shares: lsblk, /proc/mounts, fstab; mount/eject/format via udisksctl/gdbus |
| `src/search.rs` | Name and content search (`ignore`, `nucleo-matcher`, `grep-*` crates) |
| `src/editor.rs` | Built-in text editor: file side (load, save, find) |
| `src/highlight.rs` | syntect highlighting colored from the Omarchy palette |
| `src/markdown.rs` | Markdown preview (pulldown-cmark) |
| `src/theme.rs` | Omarchy palette, Hyprland rounding, text scale, glass/opacity; watches them live |
| `src/thumbs.rs` | Thumbnails (freedesktop cache + system thumbnailers), MIME lookup |
| `src/dnd.rs` | Dragging files out to other apps over Wayland |
| `src/i18n.rs`, `src/i18n/*.json` | `tr()` / `trn()` translations following Omarchy's language |
| `pkgbuild/` | Local package (`bin/install`) + `.desktop` file + icon |
| `aur/` | The AUR package `ferry-fm` (builds from the release tag tarball) |

## How the app is wired

- **Actions.** User intent is an `Action` variant pushed onto `self.pending` (from menus, keys,
  dialogs) and handled in `App::apply`. Add a variant rather than acting inside UI code.
- **Dialogs.** A `Dialog` variant, drawn in `App::dialogs` (an egui `Modal`); Enter confirms
  through `App::confirm`. Dialogs with their own buttons are excluded from the generic OK/Cancel
  row (see the `matches!` list there). Put a right-to-left button row inside `ui.horizontal`,
  or it grows to the window's height.
- **The UI thread never blocks on the disk, a process or the network.** Either:
  - `self.job(label, || -> Result<String, String>)` for fire-and-forget work that ends with a
    status message and a refresh, or
  - `std::thread::spawn` + `self.load_tx.send(Loaded::…)` + `ctx.request_repaint()`, handled in
    `App::poll`, when the result must update state (a dialog's contents, a selection).
  Results for a dialog must check it is still the same dialog/path before applying.
- **Errors** go to the status bar with `self.error(message)`, in words a user understands.
- **Settings** are `~/.config/ferry/settings.json`, written beside and renamed over.
- **Bookmarks** are GTK's `~/.config/gtk-3.0/bookmarks`, shared with other apps.

## Rules

- **Never lose data.** Don't overwrite: new names go through `files::unique_name` /
  `archive::unique` (`name (1).ext`). Write beside the target, then rename. Ask before deleting.
  Long operations work in a hidden `.ferry-*` temp beside the result and clean it up on failure.
- **Follow the desktop.** Colors come from `self.theme.palette` (`p.fg`, `p.dim`, `p.accent`,
  `p.red`…), never hard-coded. Corners from the theme's rounding.
- **Prefer tools Arch already has** (bsdtar, coreutils, udisksctl, gio, xdg-open) and the
  standard library over new crates. A new runtime tool goes into `depends`/`optdepends` in both
  `aur/PKGBUILD` and `pkgbuild/PKGBUILD`, and the README's runtime dependencies.
- **Every visible string** is `tr("English text")` or `trn("%n items", n)` with `%1`/`%n`
  placeholders, never sentences glued together. Add each new string to `src/i18n/es.json`
  (counted strings as `{"one": …, "other": …}`; also to `en.json` when counted). A test fails
  if a catalog doesn't parse.
- **Comments explain why**, in full sentences. No commented-out code.
- **Commits:** short, imperative ("Add Properties", "Fix paste into a symlinked folder"). No AI
  attribution lines (no `Co-Authored-By` for agents). Update `CHANGELOG.md` under *Unreleased*.
  Don't push or tag unless asked.

## Tests

New behavior ships with a test in the same file (`#[cfg(test)] mod tests`).

- Logic modules (`files`, `archive`, `props`, `search`, `devices`…): plain unit tests. Anything
  touching the disk makes its own directory under `std::env::temp_dir()` named with the process
  id (and thread id when tests in a module run in parallel).
- App flows (`*_tests` modules at the end of `app.rs`): build `App::new(&ctx, dir, None)` on an
  `egui::Context::default()`, call `app.apply(Action::…)`, and step frames with a `settle`
  helper that sleeps briefly, runs `ctx.run_ui(.., |ui| { app.poll(ui.ctx()); app.draw(ui) })`
  and **clears `out.textures_delta`** (egui panics otherwise). Assert on `app.dialog`,
  `app.entries`, `app.selected`, `app.cwd` and the disk.
- Widget-level tests use `egui_kittest` with `Harness::builder().allow_missing_glyphs()`;
  give unlabeled widgets a name with `labelled_by`.
- Ignored tests need a real environment and are run by hand:
  `FERRY_LOOP=1` (loop-device mount/format, may need privileges), `FERRY_PORTAL=<dir>`
  (opens the real folder picker), `FERRY_BENCH` (search benchmark).
- The theme watcher thread is skipped under `cfg(test)`; don't depend on the user's desktop.

## Trying it in the real window

Unit tests don't show layout. To look at a change, run a debug build against made-up data, never
the user's real files:

```sh
S=$(mktemp -d)                         # fake home with invented files and folders
HOME=$S/home XDG_CONFIG_HOME=$S/config XDG_CACHE_HOME=$S/cache ./target/debug/ferry "$S/home/…" &
P=$!
```

- Drive only that instance: find its window by PID (`hyprctl clients -j`, `.pid == $P`), focus
  it, and check `hyprctl -j activewindow` is still that PID before every `wtype` keystroke.
  Stop typing if focus moved.
- Screenshot just that window with `grim -g "<x>,<y> <w>x<h>"`.
- Don't trigger polkit password prompts (formatting, mounting other users' drives), don't change
  the user's Omarchy settings or theme, and restore the clipboard if a test uses it.
- Screenshots for the README use made-up content only.
- `FERRY_DEBUG=1` logs drags, thumbnails and settings to stderr.

## Releasing (only when asked)

1. Bump `version` in `Cargo.toml` and `pkgver` in `pkgbuild/PKGBUILD`; move *Unreleased* in
   `CHANGELOG.md` under `## [X.Y.Z] - <date>` and add the compare link at the bottom.
2. `cargo test`, commit "Release X.Y.Z", tag `git tag -a vX.Y.Z -m "Ferry X.Y.Z"`, push
   `main` and the tag.
3. `.github/workflows/release.yml` builds the tarball and the `ferry-fm` pacman package and
   attaches both with `.sha256` files. Watch it with `gh run watch`.
4. Then point `aur/PKGBUILD` at the tag: `pkgver`, and `sha256sums` of
   `https://github.com/yojahny55/ferry/archive/refs/tags/vX.Y.Z.tar.gz`; regenerate
   `aur/.SRCINFO` with `makepkg --printsrcinfo`; commit "Point the AUR package at X.Y.Z".
   `bin/aur-publish` pushes it to the AUR (maintainer's SSH key).
