# Changelog

All notable changes to Ferry are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and Ferry uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

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

[Unreleased]: https://github.com/yojahny55/ferry/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/yojahny55/ferry/releases/tag/v0.1.0
