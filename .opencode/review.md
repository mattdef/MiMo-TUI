# Code Review Summary

**Scope**: `@` file attachment autocomplete and real-time preview in `crates/mimo-tui/src/attachments.rs` and `crates/mimo-tui/src/app.rs`
**Overall risk**: Medium
**Verdict**: Request changes

## Findings

### [P0] Blocking

- None.

### [P1] High

- None.

### [P2] Medium

- **Trailing-slash directory queries attach the wrong path**
  - **Location**: `crates/mimo-tui/src/attachments.rs:488-503`, `crates/mimo-tui/src/app.rs:2008-2025`
  - **Why it matters**: Users who type an exact directory path like `@src/` or `@~/` cannot actually attach that directory when it is non-empty. The TUI silently attaches the first child suggestion instead, so the model receives the wrong workspace context.
  - **Evidence**: Queries ending in `/` are resolved into `search_root = <that directory>` with an empty fragment, so the picker lists the directory contents instead of the directory itself. On confirm, `confirm_attachment_picker_selection()` always prefers `selected_suggestion()` and only falls back to `try_attach_from_input()` when there are no suggestions. For any non-empty directory, Enter/Tab therefore resolves to `<dir>/<first entry>`, not the exact path the user typed.
  - **Fix**: If the raw query already resolves to an existing path, prefer the exact-path attach over the selected child; alternatively, inject an explicit “attach this directory” suggestion for trailing-separator queries. Add a regression test for non-empty `@dir/` and `@~/`.

- **Real-time preview does full synchronous filesystem work on every edit**
  - **Location**: `crates/mimo-tui/src/app.rs:534-626`, `crates/mimo-tui/src/app.rs:1956-1987`, `crates/mimo-tui/src/attachments.rs:123-199`, `crates/mimo-tui/src/attachments.rs:312-459`
  - **Why it matters**: This runs on the main input path, so large directories or slow filesystems can make the TUI visibly stall while the user types `@...` or moves through suggestions.
  - **Evidence**: Nearly every edit/navigation key calls `sync_attachment_picker()`. That immediately performs `read_dir(search_root)`, scores every entry, sorts the full result set, and then builds a preview for the selected item. Directory previews do another full `read_dir` + sort before truncating to 50 visible lines, and file previews synchronously open/read up to 64 KiB. There is no caching, debouncing, or background loading.
  - **Fix**: Move suggestion/preview generation off the UI thread (or at least debounce it), cache results while the query is unchanged, and cap directory scanning work before full sorting when possible.

### [P3] Low

- None.

## Suggested Next Steps

- [ ] Fix the trailing-slash exact-attach behavior before merge
- [ ] Add regression tests for `@dir/`, `@~/`, and large-directory picker behavior
- [ ] Re-run relevant validation after fixes
