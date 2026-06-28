# Plan: Autocomplete and Real-Time Preview for `@` File Attachments

## Objective

Add an interactive `@` attachment picker to MiMo-TUI so users can type `@`, see fuzzy-matched file and directory suggestions, navigate them with the keyboard, attach a selected item with `Tab` or `Enter`, cancel with `Escape`, and see a real-time preview of the currently selected entry. Preserve the current `@path` + `Tab` attachment path and existing system-message injection behavior.

## Requirements Snapshot

- **R1:** When the current input token starts with `@`, show a dropdown/popup of file and directory suggestions and refresh it as the user types more characters.
- **R2:** Suggestions must support both files and directories and use fuzzy matching for better ranking than simple prefix filtering.
- **R3:** While the attachment picker is active, `Up`/`Down` navigate suggestions, `Tab` or `Enter` confirms the selected suggestion, and `Escape` cancels the picker.
- **R4:** When a suggestion is selected, show a real-time preview panel that updates as selection changes and includes metadata such as size and type plus the first approximately 50 lines for files.
- **R5:** Preserve existing behavior: typing `@path` and pressing `Tab` still attaches a valid file or directory; confirmed attachments still feed `attachment_messages()` and are injected as system context; duplicate and removal behavior remain intact.
- **R6:** Keep changes minimal and focused, preserve Rust edition 2024, ratatui/crossterm architecture, workspace layout, and root cargo validation commands.

## Scope

- Modify `crates/mimo-tui/src/attachments.rs` for attachment token parsing, suggestion discovery, fuzzy ranking, preview generation, and shared attach-confirmation helpers.
- Modify `crates/mimo-tui/src/app.rs` for autocomplete state ownership, keyboard event flow, state refresh, and ratatui rendering of the picker and preview.
- Modify `crates/mimo-tui/Cargo.toml` only if tests need `tempfile.workspace = true` as a dev-dependency. Prefer an internal fuzzy matcher to avoid a new runtime dependency.
- Modify `crates/mimo-tui-core/src/input.rs` only if an additional small accessor is strictly needed; the existing `InputBuffer::current_token_bounds()` and `replace_char_range()` should be enough.
- Do not add or reorganize a `crates/mimo-tui/src/ui/` directory unless the repository has already been refactored by the time this plan is executed. Rendering currently lives in `crates/mimo-tui/src/app.rs`.

## Assumptions and Constraints

- No `.opencode/task.md` exists; this plan is based on the user-provided requirements and direct inspection of the current repository.
- Directory suggestions are attachable, matching current `attachment_messages()` support for directories through `summarize_directory()`. Navigating into directories can be done by typing a path separator, e.g. `@src/`; drilling into directories with a separate key is out of scope unless maintainers choose to add it later.
- Suggestions should be built from the current working directory for relative paths and from the home directory for `~/` paths, matching the existing attachment resolver behavior.
- File-system operations should be bounded and should not happen inside rendering methods. Suggestion and preview data should be cached in app state and refreshed from event handlers.
- Preview should be best effort. Permission errors, binary/non-UTF-8 content, deleted files, or unreadable directories should show a user-facing preview message rather than crashing the TUI.
- Existing slash command menu, command palette, model/session pickers, approval overlay, message pager, and other modal flows keep precedence over the attachment picker.

## Risks and Areas Requiring Care

- Key conflicts are the highest-risk area: `Enter` currently submits, `Tab` currently attaches exact paths or slash-completes, and `Up`/`Down` currently scroll or move slash-menu selection. The attachment picker must intercept those keys only while active.
- Avoid doing `fs::read_dir()` or file reads in `render()`; repeated I/O during drawing can make the TUI feel laggy.
- Fuzzy matching must be deterministic so tests can assert ordering and UI selection remains stable as the query changes.
- Large directories and large files must be capped: limit suggestion count and preview content.
- Path parsing must work for empty `@`, partial relative paths, nested paths such as `@crates/mimo`, home paths such as `@~/notes`, and paths containing Unicode.
- `InputBuffer` indexes are character indexes, not byte indexes. Any replacement of the `@` token must use the existing character-range helpers.
- Small terminals may not have enough space for side-by-side suggestions and preview. Rendering should degrade by reducing heights or showing a concise preview.

## Core Concepts

- **Attachment token:** The current non-whitespace token under the input cursor. It is eligible for autocomplete when it starts with `@`. The token's character start/end bounds are reused when confirming an attachment so the token can be removed exactly like the existing `try_attach_from_input()` flow.
- **Attachment query:** The text after `@`, split into a search directory and a fuzzy filter fragment. Examples: `@` searches the current directory with an empty filter; `@src/ap` searches `src/` with filter `ap`; `@~/Do` searches the user's home directory with filter `Do`.
- **Suggestion:** A cached entry containing display path, resolved path, file/directory kind, optional size/type metadata, and fuzzy score. The selected suggestion drives both confirmation and preview.
- **Preview:** A cached, best-effort summary of the selected suggestion. For files, show metadata and the first ~50 text lines. For directories, show metadata and a short directory listing because there is no file body to display.
- **Event-driven refresh:** Input-editing keys and selection-navigation keys update the attachment picker state. Rendering only displays the current state.

## Sub-Tasks

### Sub-Task 1: Add attachment autocomplete data model and token parsing

- **Status:** Pending
- **Estimated Complexity:** Medium
- **Objective:** Define the data structures and parsing helpers needed to know when the current input should show attachment autocomplete.
- **Related Requirements:** R1, R3, R5, R6
- **Dependencies and Preconditions:** Current `attachments.rs` and `InputBuffer` behavior remain available.
- **In Scope for This Sub-Task:**
  - `crates/mimo-tui/src/attachments.rs`
  - Optional, only if unavoidable: `crates/mimo-tui-core/src/input.rs`
- **Out of Scope for This Sub-Task:**
  - File-system scanning.
  - Fuzzy ranking.
  - UI rendering.
- **Instructions:**
  1. Add public or `pub(crate)` attachment autocomplete types in `attachments.rs`, such as an attachment query type, suggestion type, preview type, and picker state type.
  2. Add a helper that inspects `InputBuffer::current_token_bounds()` and returns an attachment query only when the current token starts with `@`.
  3. Preserve the token start/end character bounds in the query result for later confirmation.
  4. Treat bare `@` as an active empty query.
  5. Treat non-`@` tokens, whitespace-only input, and cursor positions outside an `@` token as inactive.
  6. Do not change `InputBuffer` unless parsing cannot be implemented cleanly with existing methods. If changed, keep it to a small accessor and add focused tests.
- **Acceptance Criteria:**
  - `@`, `@src`, `@src/lib`, and `@~/notes` are recognized as active attachment queries.
  - Tokens not starting with `@` are ignored.
  - Query results include token bounds so confirmation can remove the original token.
  - Existing `try_attach_from_input()` behavior is not changed by this sub-task.
- **Cautionary Points (Risks & Edge Cases):**
  - Reuse character-index logic; do not slice strings by arbitrary byte indexes.
  - Do not require the query path to exist at parsing time; existence is checked during suggestion discovery or final attachment.
- **Implementation Suggestions:**
  - Move the existing private `slice_chars()` helper into shared use within `attachments.rs`.
  - Keep query parsing independent of `App` so it can be unit-tested without terminal state.
- **Testing Suggestions:**
  - Add unit tests in `attachments.rs` for active/inactive query detection and token bounds.
  - If `InputBuffer` changes, add or update tests in `crates/mimo-tui-core/src/input.rs`.
- **Done When:**
  - Attachment query detection is test-covered and ready for suggestion lookup.

### Sub-Task 2: Implement bounded suggestion discovery and fuzzy ranking

- **Status:** Pending
- **Estimated Complexity:** Medium
- **Objective:** Produce deterministic file and directory suggestions for the active attachment query.
- **Related Requirements:** R1, R2, R6
- **Dependencies and Preconditions:** Sub-Task 1 completed.
- **In Scope for This Sub-Task:**
  - `crates/mimo-tui/src/attachments.rs`
  - `crates/mimo-tui/Cargo.toml` only if test-only `tempfile` is needed.
- **Out of Scope for This Sub-Task:**
  - Real-time preview generation.
  - Async/background directory scanning.
  - Recursive search through the whole workspace.
- **Instructions:**
  1. Add a suggestion function that accepts the parsed attachment query and an explicit base/current directory, then returns a capped list of suggestions.
  2. Resolve search roots consistently with existing attachment behavior:
     - empty or relative query uses `std::env::current_dir()` or an explicit test root;
     - `~/` uses `dirs::home_dir()`;
     - nested paths search the typed parent directory and fuzzy-match only the final fragment.
  3. Use `fs::read_dir()` on only the immediate search directory; do not recurse.
  4. Include both files and directories in results. Mark kind clearly so the UI can show `[file]` and `[dir]` or icons.
  5. Add a small in-crate fuzzy scoring helper to avoid new runtime dependencies unless maintainers explicitly prefer a crate. The helper should match query characters in order, case-insensitively, and score exact/prefix/contiguous matches higher.
  6. Sort suggestions deterministically, for example by descending fuzzy score, directories before files on ties, then display name/path ascending.
  7. Cap suggestions to a reasonable maximum, such as 50, to avoid huge overlays.
  8. Handle unreadable or missing directories by returning an empty list plus an optional status/error string for the picker rather than propagating a fatal error.
- **Acceptance Criteria:**
  - Bare `@` lists files and directories from the current directory.
  - `@src/ap` lists matching immediate entries under `src/`.
  - Fuzzy matches find non-prefix candidates when query characters appear in order.
  - Suggestion order is stable and testable.
  - Large directories are capped.
- **Cautionary Points (Risks & Edge Cases):**
  - `read_dir()` order is platform-dependent; always sort after collecting.
  - Do not panic if a directory entry disappears between listing and metadata lookup.
  - Keep hidden-file behavior simple and deterministic. Unless maintainers request otherwise, include entries returned by `read_dir()` and let fuzzy filtering/ranking handle them.
- **Implementation Suggestions:**
  - Include both a display path relative to the current working directory and a resolved path for I/O/attachment.
  - Preserve path separators in the display path so selecting `crates/mimo-tui` is understandable.
  - Add `tempfile.workspace = true` under `[dev-dependencies]` for `mimo-tui` if temporary directory tests are added.
- **Testing Suggestions:**
  - Unit test files and directories appearing together.
  - Unit test nested query behavior using a temporary directory tree.
  - Unit test fuzzy ranking with deterministic fixture names.
  - Unit test missing/unreadable search directory behavior as best effort for the platform.
- **Done When:**
  - The attachment module can return ranked suggestions for active `@` queries without UI involvement.

### Sub-Task 3: Implement selected-suggestion preview generation

- **Status:** Pending
- **Estimated Complexity:** Medium
- **Objective:** Generate cached preview content and metadata for the selected suggestion.
- **Related Requirements:** R4, R6
- **Dependencies and Preconditions:** Sub-Task 2 completed so suggestions include resolved paths and kinds.
- **In Scope for This Sub-Task:**
  - `crates/mimo-tui/src/attachments.rs`
- **Out of Scope for This Sub-Task:**
  - Rendering the preview panel.
  - Changing how attached file contents are injected into chat messages.
- **Instructions:**
  1. Add a preview builder that accepts a selected suggestion and returns preview metadata plus preview lines.
  2. For files:
     - read only enough content to display approximately the first 50 lines;
     - bound bytes read so a large single-line file cannot allocate excessively;
     - display file size from metadata;
     - display a simple type label such as extension, `text`, `binary/non-UTF-8`, or `file`.
  3. For directories:
     - display type `directory` and an appropriate size/entry-count label when available;
     - show a short listing of the first ~50 immediate entries instead of file content.
  4. For errors, return preview text such as `Preview unavailable: ...` instead of failing the whole event handler.
  5. Keep preview generation outside render code; the app should call it when suggestions refresh or selected index changes.
- **Acceptance Criteria:**
  - Selected file preview includes size/type and the first ~50 lines.
  - Selected directory preview includes directory metadata and a short listing.
  - Binary or invalid UTF-8 files do not crash; they show a clear preview-unavailable or binary-content message.
  - Deleted/unreadable paths show an error preview and leave the TUI usable.
- **Cautionary Points (Risks & Edge Cases):**
  - `fs::read_to_string()` on large or binary files is risky. Prefer a bounded read with UTF-8/lossy handling.
  - Avoid repeating expensive metadata reads if suggestion metadata can be reused, but keep the implementation simple.
- **Implementation Suggestions:**
  - Add a small human-readable byte formatter local to `attachments.rs`.
  - Keep preview line count and byte cap as constants, e.g. `ATTACHMENT_PREVIEW_MAX_LINES = 50`.
- **Testing Suggestions:**
  - Unit test a text file with more than 50 lines and assert truncation/line cap.
  - Unit test metadata display includes size and type.
  - Unit test directory preview lists immediate entries.
  - Unit test invalid UTF-8 or binary bytes do not panic.
- **Done When:**
  - Preview data is available and safe for the UI to render from cached state.

### Sub-Task 4: Refactor attachment confirmation while preserving existing `Tab` behavior

- **Status:** Pending
- **Estimated Complexity:** Low to Medium
- **Objective:** Share attachment-confirmation logic between the legacy exact-path flow and the new selected-suggestion flow.
- **Related Requirements:** R3, R5, R6
- **Dependencies and Preconditions:** Sub-Tasks 1-2 completed.
- **In Scope for This Sub-Task:**
  - `crates/mimo-tui/src/attachments.rs`
- **Out of Scope for This Sub-Task:**
  - App key handling changes.
  - UI rendering changes.
- **Instructions:**
  1. Extract shared logic from `try_attach_from_input()` so both exact `@path` attachment and selected-suggestion attachment:
     - resolve/display the path consistently;
     - check duplicates consistently;
     - push `FileAttachment::new(display_path)` consistently;
     - remove the original `@` token from the input using token bounds.
  2. Add a helper for confirming a selected suggestion using the query token bounds captured by autocomplete state.
  3. Keep `try_attach_from_input()` public signature and return behavior intact so existing `App::handle_tab_key()` fallback keeps working.
  4. Confirming a directory should be allowed because existing `summarize_attachment()` already supports directories.
- **Acceptance Criteria:**
  - Existing `@path` + `Tab` still attaches when no picker selection is being confirmed.
  - New selected-suggestion confirmation produces the same attachment status style as existing attachments.
  - Duplicate selected suggestions produce the existing duplicate status and do not add another attachment.
  - Input token removal remains correct for Unicode and nested paths.
- **Cautionary Points (Risks & Edge Cases):**
  - Do not change `attachment_messages()` truncation/summarization behavior in this task.
  - Do not clear unrelated input around the `@` token.
- **Implementation Suggestions:**
  - A shared internal helper can accept `start`, `end`, and a resolved `PathBuf`.
  - Preserve `display_path()` behavior so session persistence and UI previews continue to use relative paths when possible.
- **Testing Suggestions:**
  - Unit test selected-suggestion confirmation adds one `FileAttachment` and removes the token.
  - Unit test duplicate selected-suggestion confirmation matches existing duplicate behavior.
  - Regression test exact `try_attach_from_input()` still works with a real temp file and directory.
- **Done When:**
  - Both legacy and autocomplete confirmation paths share behavior and are covered by tests.

### Sub-Task 5: Wire autocomplete state into `App` event handling

- **Status:** Pending
- **Estimated Complexity:** High
- **Objective:** Make the picker open, refresh, navigate, confirm, and cancel correctly from crossterm key events.
- **Related Requirements:** R1, R3, R4, R5, R6
- **Dependencies and Preconditions:** Sub-Tasks 1-4 completed.
- **In Scope for This Sub-Task:**
  - `crates/mimo-tui/src/app.rs`
  - `crates/mimo-tui/src/attachments.rs` if small state-helper methods are needed.
- **Out of Scope for This Sub-Task:**
  - Visual styling beyond data needed by the renderer.
  - Background async scanning.
- **Instructions:**
  1. Add an attachment autocomplete/picker state field to `App` and initialize it in `App::new()`.
  2. Add helper methods on `App` for:
     - refreshing picker state from the current input token;
     - moving selection up/down and rebuilding preview;
     - confirming the selected suggestion;
     - canceling/dismissing the picker.
  3. Refresh suggestions after input-mutating events: character insertion, paste, backspace, delete, and draft-clear where appropriate.
  4. Refresh or close suggestions after cursor movement events: left, right, home, end.
  5. Intercept picker keys after higher-priority modals are handled but before global `Esc`, `Enter`, `Tab`, and scroll handling:
     - `Esc`: close/dismiss the picker without clearing input;
     - `Up`/`Down`: move selected suggestion and update preview;
     - `Tab`/`Enter`: confirm selected suggestion, clear/dismiss picker, and do not submit the prompt;
     - if no suggestions are available, `Tab` should fall back to the existing exact `@path` attach behavior.
  6. Track dismissal so pressing `Escape` does not immediately reopen the picker on the next render/event while the same unchanged `@` token remains. Reopen when the token changes or cursor leaves and re-enters an active `@` query.
  7. Clear picker state when a prompt is submitted, conversation input is cleared, an attachment is confirmed, or another modal overlay takes over.
  8. Keep existing slash menu behavior intact. Since slash menu is active for leading `/` commands and attachment picker is active for current `@` tokens, they should not conflict, but modal precedence should still be explicit.
- **Acceptance Criteria:**
  - Typing `@` opens suggestions.
  - Typing more characters filters suggestions.
  - `Up`/`Down` changes selection and preview.
  - `Tab` and `Enter` attach the selected suggestion instead of submitting while the picker is active.
  - `Escape` closes the picker and leaves input unchanged.
  - With the picker inactive or dismissed, existing `@path` + `Tab`, slash command completion, prompt submission, scrolling, and backspace attachment removal still behave as before.
- **Cautionary Points (Risks & Edge Cases):**
  - Be careful where picker handling is inserted in `handle_terminal_event()`. Higher-priority overlays already return early and should continue to do so.
  - `Enter` with `Alt`/newline modifiers currently inserts newlines; decide consistently that active picker confirmation wins only for plain `Enter`, unless maintainers want all Enter variants to confirm.
  - Do not call `submit()` after confirming a suggestion.
  - Avoid overwriting status messages unnecessarily during every filter refresh; use status mainly for attach/cancel/error outcomes.
- **Implementation Suggestions:**
  - Add a small `attachment_picker_visible()` helper analogous to `slash_menu_visible()`.
  - Add `sync_attachment_picker()` and call it at the end of input-editing branches instead of duplicating refresh logic in every branch.
  - Keep selected index clamped when suggestion count changes.
- **Testing Suggestions:**
  - Add app-level tests using `crossterm::event::Event::Key` and the existing `test_app()`/`unbounded_channel()` pattern.
  - Test that `Escape` dismisses the picker without clearing input.
  - Test that `Down` changes selected suggestion and updates preview state.
  - Test that `Enter` confirms an active selected suggestion rather than submitting a prompt.
  - Test that `Tab` still falls back to `try_attach_from_input()` when the picker is inactive.
- **Done When:**
  - The app owns and updates attachment picker state correctly without breaking existing key flows.

### Sub-Task 6: Render the autocomplete dropdown and preview panel

- **Status:** Pending
- **Estimated Complexity:** Medium
- **Objective:** Display suggestions and selected-suggestion preview in ratatui.
- **Related Requirements:** R1, R2, R4, R6
- **Dependencies and Preconditions:** Sub-Tasks 2-5 completed so state contains suggestions and preview data.
- **In Scope for This Sub-Task:**
  - `crates/mimo-tui/src/app.rs`
- **Out of Scope for This Sub-Task:**
  - A broad UI module reorganization.
  - Mouse support.
  - Syntax highlighting.
- **Instructions:**
  1. Add rendering precedence for the attachment picker in `App::render()` after higher-priority overlays and before or alongside the slash menu overlay.
  2. Add `render_attachment_picker_overlay()` that uses `Clear`, `Block`, `Layout`, `Paragraph`, `Text`, `Line`, and `Span`, following existing overlay patterns in `app.rs`.
  3. Render a two-column panel when space allows:
     - left column: suggestions with selected-row highlighting, kind marker, and display path;
     - right column: preview metadata and preview lines.
  4. Add a compact fallback for small terminal sizes, such as suggestions first with a shortened preview below or a preview-unavailable note.
  5. Include a concise help hint in the title or footer, e.g. `↑/↓ move · Tab/Enter attach · Esc cancel`.
  6. Avoid reading files or directories inside the render method. Render only cached strings/lines from state.
  7. Suppress or carefully place the input cursor while the picker is open by updating the existing cursor-visibility conditions in `render_draft_box()` and `render_landing_prompt()`.
- **Acceptance Criteria:**
  - The picker is visible when the active state has suggestions or a query status to show.
  - The selected suggestion is visibly highlighted.
  - File/directory kind is visible.
  - Preview panel shows metadata and preview lines for the selected entry.
  - The UI remains usable on both the landing prompt and normal workspace screen.
- **Cautionary Points (Risks & Edge Cases):**
  - Existing overlay helpers use centered popups. A bottom-anchored popup may feel more like a dropdown, but keep geometry simple and robust.
  - Do not cover approval prompts or modal pickers; those must remain highest priority.
  - Long paths and long preview lines should wrap or truncate gracefully.
- **Implementation Suggestions:**
  - Start with a centered or bottom-biased popup using existing `centered_rect()`/`landing_popup_rect()` style helpers. Only add a new geometry helper if needed.
  - Use `Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD)` or a similar existing highlight style for selected entries to stay visually consistent with slash menu rendering.
- **Testing Suggestions:**
  - Use `ratatui::backend::TestBackend` through the existing `render_screen()` test helper.
  - Render a seeded picker state and assert the screen contains the overlay title, selected file name, preview metadata, and keyboard hint.
  - Render with no picker state and assert normal prompt rendering still works.
- **Done When:**
  - The autocomplete and preview UI renders from state with no render-time I/O and no regressions in existing overlays.

### Sub-Task 7: Add regression tests and run workspace validation

- **Status:** Pending
- **Estimated Complexity:** Medium
- **Objective:** Cover the new attachment picker behavior and verify the full workspace remains healthy.
- **Related Requirements:** R1, R2, R3, R4, R5, R6
- **Dependencies and Preconditions:** Sub-Tasks 1-6 completed.
- **In Scope for This Sub-Task:**
  - Unit tests in `crates/mimo-tui/src/attachments.rs`.
  - App/event/render tests in `crates/mimo-tui/src/app.rs`.
  - Optional `crates/mimo-tui/Cargo.toml` dev-dependency for temp-file fixtures.
- **Out of Scope for This Sub-Task:**
  - Tests requiring a live MiMo API key.
  - End-to-end terminal automation outside the existing Rust test suite.
- **Instructions:**
  1. Add attachment-module tests for:
     - active `@` query detection;
     - file and directory suggestion discovery;
     - fuzzy ranking/order;
     - preview line cap and metadata;
     - exact `@path` attach regression;
     - selected-suggestion attach and duplicate behavior.
  2. Add app-level event tests for:
     - typing `@` opens or prepares picker state;
     - typing more characters refreshes/filter suggestions;
     - `Up`/`Down` navigation changes selection;
     - `Escape` dismisses without mutating input;
     - `Tab`/`Enter` confirms selection without submitting prompt.
  3. Add render tests for the picker overlay and preview panel using `TestBackend`.
  4. Avoid global current-directory mutations in tests where possible by making suggestion helpers accept an explicit base directory.
  5. Run targeted tests before workspace-wide validation.
- **Acceptance Criteria:**
  - New unit tests cover happy paths and key edge cases.
  - Existing app tests still pass.
  - Root workspace commands pass.
- **Cautionary Points (Risks & Edge Cases):**
  - Tests that depend on directory listing order must assert post-sort behavior, not OS `read_dir()` order.
  - If tests must change `current_dir`, isolate them carefully and restore it, but prefer explicit path parameters instead.
  - Keep test fixtures small and platform-neutral.
- **Implementation Suggestions:**
  - Use `tempfile` for file trees if added as a `mimo-tui` dev-dependency.
  - Reuse the existing app test helpers around `test_app()`, `render_screen()`, and `unbounded_channel()`.
- **Testing Suggestions:**
  - Run, in order:
    1. `cargo test -p mimo-tui attachments`
    2. `cargo test -p mimo-tui app`
    3. `cargo fmt --check`
    4. `cargo check`
    5. `cargo clippy --workspace -- -D warnings`
    6. `cargo test --workspace`
- **Done When:**
  - Automated tests and validation commands confirm the autocomplete and preview features without regressions.

## Final Integration & Verification

- **System-Wide Test:**
  1. Launch the TUI with `cargo run`.
  2. Type `@` in an empty prompt and verify file/directory suggestions appear.
  3. Type a partial nested path such as `@crates/mimo` and verify suggestions filter with fuzzy matching.
  4. Use `Up`/`Down` and verify the highlighted suggestion and preview panel update together.
  5. Select a text file and verify preview shows size/type and approximately the first 50 lines.
  6. Select a directory and verify it can be attached and previewed.
  7. Press `Escape` and verify the picker closes without deleting input.
  8. Press `Tab` or `Enter` on a selected suggestion and verify the attachment appears in the existing attached-context UI.
  9. Type an exact `@path` and press `Tab` with the picker inactive/dismissed to verify legacy behavior still works.
  10. Submit a prompt with an attachment and verify attached context is still injected through `attachment_messages()`.
- **Completion Checklist:**
  - [ ] `@` attachment picker opens and refreshes as the current token changes.
  - [ ] Suggestions include files and directories.
  - [ ] Fuzzy matching is deterministic and tested.
  - [ ] `Up`/`Down`, `Tab`/`Enter`, and `Escape` work only when the picker is active.
  - [ ] Preview shows metadata and first ~50 file lines, with graceful handling for directories/errors/binary content.
  - [ ] Existing `@path` + `Tab`, duplicate detection, attachment removal, and system-message injection still work.
  - [ ] Rendering uses ratatui state only and does not perform file I/O inside `render()`.
  - [ ] No broad architecture or workspace-layout changes were introduced.
  - [ ] `cargo fmt --check`, `cargo check`, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace` pass.

## Open Questions

- None blocking. If maintainers prefer `Enter` on a directory to drill into that directory instead of attaching it, update Sub-Tasks 4-6 consistently before implementation.
