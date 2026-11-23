# Contributing to Ferry

Thanks for helping. This guide covers how to report problems, build Ferry, and send changes.

## Reporting bugs and asking for features

- Search the [existing issues](https://github.com/yojahny55/ferry/issues) first.
- Use the issue templates. For bugs, include your Ferry version, your Omarchy version (or
  distro and compositor), and the output of `FERRY_DEBUG=1 ferry` while reproducing the problem.
- One problem per issue.

Security problems go through [SECURITY.md](SECURITY.md), not public issues.

## Building

You need Rust 1.95 or newer and a Linux desktop (Wayland recommended).

```sh
git clone https://github.com/yojahny55/ferry
cd ferry
cargo run -- ~/Pictures      # run against a folder
bin/test                     # unit tests
bin/install                  # build a pacman package and install it (Arch/Omarchy)
```

Before sending a pull request, make sure these pass, as CI runs them:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## How the code is organized

| File | What it does |
| --- | --- |
| `src/main.rs` | Startup: window options, fonts, command-line path |
| `src/app.rs` | The window: views, sidebar, keys, dialogs, background folder loading |
| `src/files.rs` | Everything that touches the disk: listing, sorting, paste, trash, bookmarks |
| `src/theme.rs` | Reads the Omarchy palette and Hyprland rounding, and styles egui |
| `src/thumbs.rs` | Previews: the freedesktop thumbnail cache and system thumbnailers |
| `src/dnd.rs` | Dragging files out to other apps over Wayland |
| `src/i18n.rs` | Translations, following Omarchy's language setting |

A few principles hold the code together:

- **The UI thread never waits on the disk.** Anything that can be slow (listing a folder,
  checking a network mount) runs on a background thread and reports back. The window always
  keeps drawing.
- **Never lose data.** Don't overwrite files; write configuration beside the target and rename.
  Ask before deleting.
- **Say what's wrong** in the window instead of silently doing less.
- **Follow the desktop.** Colors come from the theme; don't hard-code them.
- **Keep it small.** Prefer the standard library and tools the desktop already has over new
  dependencies. Comments explain *why*, not *what*.

New behavior comes with a test in the same file (`#[cfg(test)] mod tests`). Tests that touch
the disk use their own temporary directory.

## Translations

Translations live in `src/i18n/<code>.json`, one per
[Omarchy language code](https://github.com/basecamp/omarchy) (`da`, `de`, `pt-BR`, `es`…). Each
file maps the English text to its translation:

```json
{
  "Rename": "Renombrar",
  "%n files": { "one": "%n archivo", "other": "%n archivos" }
}
```

- Keys are the English text exactly as it appears in `tr("…")` / `trn("…", n)` in the code.
- Keep `%1` and `%n` placeholders.
- Counted strings take `one` and `other` forms.
- Missing strings fall back to English, so a partial translation is fine.

To add a language: create the file, add it to `CATALOGS` in `src/i18n.rs`, add `Name[<code>]` and
`Comment[<code>]` lines to `pkgbuild/ferry.desktop`, and run `cargo test` (a test checks that
every catalog parses).

## Pull requests

- Keep each pull request to one change, and describe what it changes and why.
- Use short, imperative commit messages ("Add tabs", "Fix paste into a symlinked folder").
- Add screenshots for visible changes, using made-up files rather than your own.
- Update `CHANGELOG.md` under *Unreleased*.

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).
