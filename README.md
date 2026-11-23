<div align="center">

<img src="pkgbuild/ferry.svg" width="96" alt="Ferry icon">

# Ferry

**A fast, keyboard-first file manager for [Omarchy](https://omarchy.org) that follows your theme live.**

In the spirit of Dolphin, written in Rust, and at home on Hyprland.

[![CI](https://github.com/yojahny55/ferry/actions/workflows/ci.yml/badge.svg)](https://github.com/yojahny55/ferry/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/yojahny55/ferry)](https://github.com/yojahny55/ferry/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Rust 1.95+](https://img.shields.io/badge/rust-1.95%2B-orange.svg)

<img src="docs/screenshots/icons.png" alt="Ferry showing a folder of photos with previews, in Omarchy's default theme">

</div>

## Why Ferry

Omarchy's look is consistent until you open a file manager. Ferry takes its whole palette from
the current Omarchy theme, not just the accent, and changes with it the moment you switch
themes. Corners follow Hyprland's rounding, text uses your Omarchy font, and the window speaks
the language you picked in Omarchy's setup.

It is also built to stay out of your way: every action has a key, folders load in the
background so a sleeping NAS never freezes the window, and the app starts instantly.

## Features

- **Follows the desktop, live**: theme colors, window rounding, font, and language, with no restart.
- **Details and icons views**, with sortable columns and natural ordering (`photo2` before `photo10`).
- **Zoom** with Ctrl+wheel or Ctrl+±, kept separately for each view and remembered.
- **Previews** of images, videos, PDFs and office files, read from and written to the shared
  freedesktop thumbnail cache, so previews other apps already made appear instantly.
- **Bookmarks** you can drag folders onto or add with **+**, shared with GTK's bookmarks, so
  they also show up in the desktop's file pickers.
- **Drag files out** into other apps (a browser, a chat, another file manager) through native Wayland drag-and-drop.
- **Safe file operations**: paste never overwrites (`name (1).ext`), Delete moves to the trash,
  and only Shift+Delete deletes, after asking.
- **Never blocks on slow disks**: folder loading and change checks run in the background, so an
  unreachable network mount shows *Loading…* instead of freezing the window.
- **Type to jump** to a name, filter as you type, and edit the path directly.
- Copy and paste interoperate with the terminal: copied files go to the clipboard as paths, and pasted paths become files.

<details>
<summary>Details view</summary>

<img src="docs/screenshots/details.png" alt="Ferry's details view with name, size and modified columns">

</details>

## Install

Ferry targets Omarchy and other Arch-based Hyprland desktops. It needs a Wayland session for
dragging files out; the rest also builds for X11, but that path is untested.

### From a release

Download the latest `ferry-*-x86_64-linux.tar.gz` from
[Releases](https://github.com/yojahny55/ferry/releases/latest), then:

```sh
tar xf ferry-*-x86_64-linux.tar.gz
cd ferry-*-x86_64-linux
sudo install -Dm755 ferry /usr/local/bin/ferry
sudo install -Dm644 ferry.desktop /usr/local/share/applications/ferry.desktop
sudo install -Dm644 ferry.svg /usr/local/share/icons/hicolor/scalable/apps/ferry.svg
```

### From source (Arch / Omarchy)

```sh
git clone https://github.com/yojahny55/ferry
cd ferry
bin/install        # builds a pacman package with makepkg and installs it
```

This puts Ferry in the app launcher (Super + Space). Building needs Rust 1.95 or newer
(`sudo pacman -S rust`).

### Make it the default

```sh
xdg-mime default ferry.desktop inode/directory
```

To open it with Super+E on Omarchy, add this to `~/.config/hypr/bindings.lua`:

```lua
o.bind("SUPER + E", "File manager", { launch = "ferry" })
```

### Runtime dependencies

Ferry uses tools Omarchy already ships: `gio` (trash), `xdg-open` (opening files), and
`coreutils` (`cp`, `mv`). Optional: `xdg-terminal-exec` for *Open Terminal Here*, and the
thumbnailers in `/usr/share/thumbnailers` (glycin, ffmpegthumbnailer, papers…) for previews.

## Keyboard shortcuts

Press `?` in the app for this list.

| Key | Action |
| --- | --- |
| Enter / Space | Open |
| Backspace / Alt+Left | Back |
| Alt+Right | Forward |
| Alt+Up | Parent folder |
| Alt+Home | Home folder |
| Ctrl+L | Edit the path |
| Ctrl+F | Filter |
| a–z | Jump to a name |
| Ctrl+C / Ctrl+X / Ctrl+V | Copy, cut, paste |
| Ctrl+Alt+C | Copy path |
| F2 | Rename |
| Del | Move to trash |
| Shift+Del | Delete permanently |
| Ctrl+Shift+N | New folder |
| Ctrl+D | Bookmark the selected folders, or this one |
| Ctrl+A | Select all |
| Ctrl+H | Show hidden files |
| Ctrl+1 / Ctrl+2 | Details or icons |
| Ctrl+Wheel, Ctrl++ / Ctrl+- | Zoom the current view |
| Ctrl+0 | Reset zoom |
| F4 | Open a terminal here |
| F5 | Refresh |
| ? | Keyboard shortcuts |
| Ctrl+Q | Quit |

## Files Ferry reads and writes

| Path | What for |
| --- | --- |
| `~/.local/state/omarchy/current/theme/colors.toml` | Theme palette (read; Omarchy's yellow accent when absent) |
| `~/.config/omarchy/language` | Interface language (read) |
| `~/.config/gtk-3.0/bookmarks` | Bookmarks, shared with GTK apps (read and written; other entries are kept as they are) |
| `~/.cache/thumbnails/` | Shared preview cache, per the freedesktop spec (read and written) |
| `~/.config/ferry/settings.json` | View, zoom, sorting, hidden files (written) |

Ferry never changes Omarchy's or Hyprland's configuration.

## Languages

English and Spanish ship today, following Omarchy's language setting. Adding a language is a
single JSON file; see [CONTRIBUTING.md](CONTRIBUTING.md#translations).

## Known limitations

- Dropping files *into* Ferry from other apps isn't supported yet.
- No tabs, split view, or undo yet.
- Right-to-left languages aren't laid out right to left yet.
- *Open with…* and file properties aren't there yet.

See the [issues](https://github.com/yojahny55/ferry/issues) for what's planned, and feel free to
open one for what you'd like.

## Troubleshooting

Run Ferry from a terminal with `FERRY_DEBUG=1 ferry` to log drags, thumbnails and settings.
Please include that output when you [report a bug](https://github.com/yojahny55/ferry/issues/new/choose).

## Contributing

Contributions are welcome: bug reports, translations, and code. Start with
[CONTRIBUTING.md](CONTRIBUTING.md). Everyone taking part is expected to follow the
[Code of Conduct](CODE_OF_CONDUCT.md). To report a security problem, see [SECURITY.md](SECURITY.md).

## Credits

- Built with [egui](https://github.com/emilk/egui) and [eframe](https://github.com/emilk/egui/tree/main/crates/eframe).
- Theme following is modeled on Omarchy's own apps, such as [Omacut](https://github.com/omacom-io/omacut) (MIT, David Heinemeier Hansson).
- Inspired by KDE's [Dolphin](https://apps.kde.org/dolphin/).

## License

[MIT](LICENSE) © Yojahny Chavez
