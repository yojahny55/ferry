# Changelog

All notable changes to Ferry are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and Ferry uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.4.0] - 2026-11-24

### Fixed

- The active tab no longer has a line drawn across its name, and the tab's × sits centered on
  the right with room before it (it highlights on hover).
- Clicking anywhere in a split pane (a file, empty space, a right-click) now makes it the active
  one, so the toolbar and view switches act on it (F6 already did; the mouse didn't). Each half also keeps its own grid column width instead of sharing one.

### Added

- "Show in folder" from a browser or another app opens Ferry with the file selected: Ferry now
  answers the desktop's standard org.freedesktop.FileManager1 request over D-Bus.
- Tabs: Ctrl+T opens one in the current folder, Ctrl+W closes it (quitting when it is the last),
  Ctrl+Tab and Ctrl+Shift+Tab move between them. A tab strip appears once there are two or more;
  each tab shows its folder, clicking switches to it, and its × closes it. Alt+X opens a terminal
  in the active pane's folder, like F4.
- Split view: F3 splits the window into two side-by-side panes that browse independently (one
  shared sidebar), F6 switches between them, F3 again closes it. With no destination given, a
  copy or move goes from the active pane to the other one.
- Undo and redo: Ctrl+Z and Ctrl+Shift+Z reverse moves, copies, renames, new folders and files,
  and deletes to the trash (up to 20 steps). A reversal is refused with a message when it would
  overwrite something or its item is gone; permanent deletes can't be undone.

## [0.3.1] - 2026-11-23

### Fixed

- Pasting text copied in other apps into the editor and text fields, and Copy Path reaching
  other apps: Ferry was built without the system clipboard, so it only saw its own copies.

## [0.3.0] - 2026-11-23

### Added

- Compress and extract. Opening an archive shows what's inside, then extracts it: an archive
  holding one folder comes out as that folder, anything else goes into a new folder named after
  the archive, and nothing is ever overwritten. *Extract Here*, *Extract To…* (the desktop's folder picker, through xdg-desktop-portal) and
  *Compress…* (zip, tar.zst, tar.gz, tar.xz, 7z) are in the file menu. It all goes through
  bsdtar (libarchive, which every Arch system has), so zip, 7z, rar, tar in every compression,
  iso, cpio and deb open, as do single compressed files (`.gz`, `.xz`, `.zst`, `.bz2`, `.lz4`).
  Work happens beside the result and is renamed into place, so a failure leaves nothing behind.
- Properties (Alt+Enter, or in the right-click menu): type, location, size and size on disk,
  a folder's contents counted in the background, dates, owner and group, read/write/run
  permissions for owner, group and others, and SHA-256/MD5 checksums with a paste-to-compare
  field. Several selected items show their combined contents.
- *Make Executable* / *Make Not Executable* in the file menu, for scripts and programs.
- Archives are recognized by their whole suffix (`.tar.zst`, `.tgz`, `.cbz`…) and get the
  archive icon.

## [0.2.1] - 2026-11-23

### Added

- Each release carries a pacman package, `ferry-fm-<version>-1-x86_64.pkg.tar.zst`, built on
  a clean Arch machine: install it with `sudo pacman -U`. The package is named `ferry-fm`
  (the command is still `ferry`), since another app already claims the name `ferry`.
- An AUR PKGBUILD for `ferry-fm` in `aur/`, published as soon as AUR registration reopens.
  Until then, `makepkg -si` with it builds and installs Ferry from source.

## [0.2.0] - 2026-11-23

### Added

- A built-in text editor and viewer. Text files (notes, Markdown, code, configs) open inside
  Ferry with line numbers, undo and zoom; Ctrl+S saves, Esc or Ctrl+W goes back to the folder.
  Saving writes beside the file and renames it into place, keeps permissions and symlinks, and
  asks before overwriting changes made by another program. Closing or quitting with unsaved
  changes asks first. Files that aren't valid UTF-8 open read-only; binary files and files over
  5 MiB open in the default app.
- *Open in Text Editor* and *Open With Default App* in the file context menu.
- Search in subfolders (Ctrl+Shift+F), by name or by contents, built on ripgrep's crates:
  parallel, respects `.gitignore` and hidden files unless asked not to, skips binary files,
  streams results, and cancels a search as soon as the query changes. Names are matched fuzzily
  with nucleo. Contents results show the matching lines and open the file at that line. Pressing
  Enter in the filter when nothing in the folder matches searches the subfolders.
- In the text editor: syntax highlighting colored from the Omarchy theme, a find bar (Ctrl+F,
  Enter / Shift+Enter, F3), and a rendered Markdown preview (Ctrl+E), which Markdown files open in.
- *New Text File…* (Ctrl+Alt+N).
- Drives show how full they are, with an eject button, and *Mount*, *Unmount*, *Eject* and
  *Format…* in their menu, through UDisks. Plugged-in drives that aren't mounted yet show too,
  and mount when opened. Drives without a label go by their size instead of their UUID.

### Changed

- Ferry follows the desktop's text size (GTK's `text-scaling-factor`), so on a high-resolution
  screen it's as big as your other apps instead of tiny.
- Glassy Omarchy windows make Ferry see-through, like the terminals: the background lets the
  blurred desktop through while text and icons stay solid. Opaque and default windows stay solid.
- An Appearance menu sets size and transparency by hand, saved between launches.
- Secondary text, icons and section titles are made readable on every theme (WCAG AA, 4.5:1),
  and the theme's `muted` color draws lines and tracks. Light themes get light controls.
- Sidebar rows are taller, and the current place is a soft accent pill instead of a hard bar.

### Fixed

- NFS and other network shares showed as USB drives. They now have their own *Network* section,
  read from `/proc/mounts` and `/etc/fstab` without touching the network.

## [0.1.1] - 2026-11-23

### Fixed

- A crash when quitting: the drag-and-drop thread could clean up Wayland objects after the
  window had already closed their connection.

### Changed

- Release binaries keep function names, so crash reports show where a crash happened.

## [0.1.0] - 2026-11-23

The first release.

### Added

- Details and icons views with sortable columns, natural name ordering, and a filter.
- Places sidebar with home, user folders, trash, root and mounted drives.
- Bookmarks: drag folders onto the sidebar, press **+**, or Ctrl+D; shared with GTK's bookmarks file.
- Copy, cut, paste, rename, new folder, move to trash, and permanent delete with confirmation.
  Paste never overwrites existing files.
- Previews of images, videos, PDFs and office files through the freedesktop thumbnail cache and
  the system's thumbnailers.
- Zoom per view with Ctrl+wheel, Ctrl+± and Ctrl+0.
- Dragging files out to other apps through native Wayland drag-and-drop.
- Live theme, window rounding, font and language following Omarchy.
- English and Spanish translations.
- Settings (view, zoom, sorting, hidden files) remembered between launches.
- Background folder loading, so slow or unreachable network mounts never freeze the window.
- `FERRY_DEBUG=1` logging for drags, thumbnails and settings.

[Unreleased]: https://github.com/yojahny55/ferry/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/yojahny55/ferry/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/yojahny55/ferry/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/yojahny55/ferry/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/yojahny55/ferry/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/yojahny55/ferry/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/yojahny55/ferry/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/yojahny55/ferry/releases/tag/v0.1.0
