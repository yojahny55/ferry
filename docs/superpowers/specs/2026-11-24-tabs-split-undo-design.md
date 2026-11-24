# Tabs, Split View & Undo — Design

Date: 2026-11-24
Status: Approved by user

## Problem

README "Known limitations": "No tabs, split view, or undo yet." All three are requested as one project.

## Decisions (all confirmed with user)

| Question | Decision |
|----------|----------|
| Undo operations | move, copy, rename, create folder/file, delete-to-trash. Permanent delete NOT undoable. |
| Tabs | Independent views (own cwd/history/selection). No persistence across launches. Window-level tab strip. |
| Split view | Optional, toggled with F3. Single-pane by default; F3 splits into two side-by-side panes; F3 again closes it. |
| Tabs × split interaction | Tabs are window-level (one strip at top). Split is a window-level layout toggle, not per-tab. |
| Pane switch key | F6 (Total Commander convention). |
| Terminal in pane | Alt+X opens terminal in active pane's cwd (same action as existing F4 `Action::Terminal`). |
| Undo depth | 20 entries, in-memory, per-process. Does not survive restart. |
| Undo unit | One user operation = one entry. Batch rename of N files = one undo action reverting all N. |
| Undo safety | Before reversing: verify destination path is free and item still exists at new location. Refuse with clear status message rather than overwrite. A failed undo is a correct outcome. |
| New dependencies | None. `egui_frames` pins egui 0.36; Ferry is on 0.37. Hand-roll tab strip + split layout in Ferry's own code. |

## Key bindings (verified free against existing bindings)

| Key | Action |
|-----|--------|
| `Ctrl+T` | New tab (opens at active pane's cwd) |
| `Ctrl+W` | Close current tab (last tab closes the window) |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Next / previous tab |
| `F3` | Toggle split view |
| `F6` | Switch active pane (only when split is on) |
| `Alt+X` | Open terminal in active pane (same as F4) |

No collisions: F1–F5 are Help/Rename/Terminal/Refresh; F3-in-editor is find-next but only fires while the editor is open (checked first); bare Tab is search-view only; Ctrl+W in file view is unused (editor-only).

## Architecture

### Pane extraction

The ~15 view-state fields currently on `App` move into a new `Pane` struct:

- `cwd`, `back`, `forward` (history stacks)
- `entries`, `view`, `selected`, `cursor`
- scroll position, grid/list zoom state

`App` then owns:

```rust
struct App {
    tabs: Vec<Tab>,          // window-level tab list
    active_tab: usize,
    split: bool,             // F3 toggle
    panes: [Pane; 2],        // second pane only live when split
    active_pane: usize,      // which pane gets keys (F6 swaps)
    undo: Vec<UndoEntry>,    // depth 20, in-memory
    // ... existing window-level state (theme, settings, dialogs, editor, search)
}

struct Tab {
    cwd: PathBuf,            // where this tab is
    // per-tab history/selection live in the pane when the tab is active;
    // simplest form: a Tab IS a Pane, and panes are derived from tabs.
}
```

**Simplest consistent model:** a `Tab` *is* a `Pane`. `App.tabs: Vec<Pane>` + `active_tab`. When split is on, the second pane is a separate `Pane` (created as a copy of the active tab's cwd when F3 turns split on; discarded when F3 turns it off). This avoids a Tab/Pane indirection layer.

### Layout

- **Single-pane mode:** existing layout unchanged (sidebar + file view), plus a tab strip above the file view.
- **Split mode:** sidebar stays on the left (shared, bookmarks act on active pane). The file area splits into two side-by-side panes with a thin divider. Each pane draws its own file list/grid. The active pane is visually indicated (e.g., accent border or brighter header).

### Tab strip

Hand-rolled egui: horizontal row of tab buttons above the file view, each showing the folder name (basename of cwd), closable with an × when more than one tab is open. Click to switch; Ctrl+T adds; Ctrl+W closes (last tab → close window). ~100 lines.

### Copy/move between panes

When split is on and no explicit destination is given: active pane = source, other pane = destination. This matches Total Commander behavior. When split is off, existing behavior (paste dialog / current folder) is unchanged.

## Undo design

### Entry shape

```rust
enum UndoEntry {
    Move { from: Vec<PathBuf>, to: Vec<PathBuf> },
    Copy { src: Vec<PathBuf>, dst: Vec<PathBuf> },
    Rename { old: PathBuf, new: PathBuf },   // or batch: Vec<(old, new)>
    Create { paths: Vec<PathBuf> },          // folders/files created
    Trash { paths: Vec<PathBuf> },           // moved to trash; undo restores
}
```

Stack capped at 20 (oldest evicted). Ctrl+Z pops and reverses; Ctrl+Shift+Z re-applies (redo stack, also depth 20 — or omit redo if keeping it minimal; **decision: include redo**, it's ~10 lines of symmetric code and every file manager has it).

### Reversal rules

- **Move undo:** move items back from `to` to `from`. Pre-check: each `from` path is free (doesn't exist) AND each `to` path still exists. If any check fails, refuse with a status message naming the conflict; do not partially apply.
- **Copy undo:** delete the copies at `dst`. Pre-check: each `dst` exists. (Originals are untouched by copy, so no source check needed.)
- **Rename undo:** rename back. Pre-check: `old` is free and `new` still exists.
- **Create undo:** remove the created items. Folders must still be empty; files unchanged — if a created file was modified or a folder now has contents, refuse (matches Luciole's rule).
- **Trash undo:** restore from trash to original path. Pre-check: original path is free.

All checks happen before any filesystem mutation; the whole entry applies atomically (all-or-nothing per entry).

### Menu integration

Menu items show the action name: "Undo Move", "Undo Rename", etc. (not bare "Undo"). When the stack is empty, the item is disabled/hidden.

## i18n

Every new visible string goes through `tr()` / `trn()`:
- Tab close tooltip, "New Tab" menu item, "Close Tab", "Toggle Split View", "Switch Pane"
- Undo/redo menu items: "Undo Move", "Undo Copy", "Undo Rename", "Undo Create", "Undo Delete", "Redo …"
- Status messages: undo refusal messages ("Cannot undo: <path> already exists"), split on/off, tab closed
- Counted strings (e.g. "%n tabs") go in `en.json` as `{"one": …, "other": …}` and always in `es.json`.

## Tests

App-flow tests (`*_tests` modules at end of `app.rs`), using the existing `settle` helper:

1. **Tabs:** Ctrl+T opens a second tab (assert `tabs.len() == 2`, active switches); navigate in tab 2, switch back, assert tab 1's cwd unchanged; Ctrl+W closes; last tab close → window-close path.
2. **Split:** F3 toggles `split` on (assert two panes, second pane cwd = first's); F6 swaps `active_pane`; F3 off restores single-pane state.
3. **Undo — each operation:** perform the op via `app.apply(Action::…)`, assert disk changed; Ctrl+Z, assert disk reverted. Include the refuse-on-conflict path (create a file at the old path, attempt undo, assert refusal + status message + no partial change).
4. **Undo depth:** push 21 entries, assert oldest evicted.

Logic tests for `unique_name`-adjacent undo pre-checks if they live in `files.rs`.

## Out of scope (named, per ponytail)

- Tab persistence across launches — user chose no persistence.
- Drag-to-reorder tabs, middle-click close, tab thumbnails — not requested.
- Vertical split orientation — side-by-side only.
- More than two panes — F3 is a binary toggle.
- Undo of permanent delete — agreed non-undoable everywhere.
- `egui_frames` dependency — version mismatch; hand-rolled instead.

## Files touched (expected)

- `src/app.rs` — Pane extraction, tab strip, split layout, key bindings, undo stack + reversal, menu items
- `src/files.rs` — undo pre-check helpers if they don't fit inline
- `src/i18n/en.json`, `src/i18n/es.json` — new strings
- `README.md` — remove "No tabs, split view, or undo yet." from Known limitations; document keys
- `CHANGELOG.md` — Unreleased section
