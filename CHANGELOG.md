# Changelog

All notable changes to Ferry are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and Ferry uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

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

[Unreleased]: https://github.com/yojahny55/ferry/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/yojahny55/ferry/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/yojahny55/ferry/releases/tag/v0.1.0
