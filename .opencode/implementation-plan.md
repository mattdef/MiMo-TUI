# Implementation Plan: Conversation Branching for MiMo-TUI

## Objective

Implement conversation branching so a user can choose a prior message, fork the visible transcript from that point, enter a new prompt, switch between preserved branches, and save/load the branch tree across sessions.

This plan follows the 7 sub-tasks from `.opencode/plan.md`, but adapts the details to the current codebase to keep changes minimal and avoid leaking branch metadata into OpenAI-compatible API requests.

## Requirements Snapshot

- **R1:** Users can select any non-system historical message in the active conversation path.
- **R2:** Selecting a message creates a new active branch from that point.
- **R3:** Existing/original conversation paths remain accessible after branching.
- **R4:** After branching, the user can type a new prompt to continue from the branch point.
- **R5:** Branch trees persist across saved sessions and legacy flat sessions still load.
- **R6:** The UI indicates active branch state and branch points.
- **R7:** Existing commands (`/retry`, `/undo`, `/compact`) continue to work in the active branch.
- **R8:** Branching overhead remains small for normal conversation sizes.

## Current Architecture Notes

- `crates/mimo-protocol/src/lib.rs` defines `ChatMessage`, which is also serialized directly in `mimo-client` request payloads. Do **not** add branch metadata that will serialize to MiMo API requests.
- `crates/mimo-tui/src/app.rs` currently treats `messages: Vec<ChatMessage>` as the full visible transcript, with `assistant_index: Option<usize>` tracking the streaming assistant message.
- Slash commands are parsed in `crates/mimo-tui-core/src/commands.rs` and executed in `App::execute_command`.
- Session persistence currently stores only flat `SavedSession.messages` in `crates/mimo-state/src/session_store.rs`.
- Workspace snapshots in `crates/mimo-tools/src/history.rs` are unrelated to conversation branching; keep `/undo` and `/restore` behavior tied to workspace snapshots.

## Scope

- Add branch-aware conversation storage in `mimo-state`.
- Keep `App.messages` as the active-branch visible cache during this implementation.
- Add `/branch`, `/branches`, and `/switch` commands.
- Add Ctrl+B branch-selection mode and branch indicators in the conversation panel/footer.
- Persist branch trees while keeping legacy session compatibility.
- Update help/keybinding docs and README.

## Assumptions and Constraints

- Use deterministic sequential IDs such as `msg-1` and `branch-1`; do not add a `uuid` dependency unless a later product decision requires globally unique IDs.
- A branch is a stable branch record with a `branch_id` and a path of message IDs. The active path is the path for the active branch.
- Branching from system messages is rejected to avoid command/status metadata becoming fork points.
- `/branch <n>` uses 1-based visible message numbering to match user-facing command conventions such as `/plan done <n>`.
- The flat `messages` field remains the source used by existing rendering code, but it must be synchronized from `ConversationTree` after branch mutations and session loads.
- Branch metadata must not be sent to MiMo API requests.

## Risks and Areas Requiring Care

- **API compatibility:** `ChatMessage` is a transport type; branch IDs must not appear in JSON sent to `/chat/completions`.
- **Streaming consistency:** streamed assistant deltas must update both `App.messages[assistant_index]` and the corresponding tree node.
- **Persistence compatibility:** sessions saved before branching must deserialize and migrate to a single-branch tree.
- **Command semantics:** `/retry`, `/compact`, `/undo`, `/restore`, `/save`, `/load`, and `/export` must act on the active branch unless explicitly documented otherwise.
- **Index ambiguity:** UI and `/branch <n>` should clearly use 1-based message numbers.

## Core Design

Keep `ChatMessage` as the API payload and store branch metadata in a new `mimo-state` wrapper:

```rust
pub type MessageId = String;
pub type BranchId = String;

pub struct ConversationNode {
    pub id: MessageId,
    pub parent_id: Option<MessageId>,
    pub child_ids: Vec<MessageId>,
    pub message: ChatMessage,
}

pub struct ConversationBranch {
    pub id: BranchId,
    pub path: Vec<MessageId>,
    pub created_from: Option<MessageId>,
}

pub struct ConversationTree {
    pub nodes: BTreeMap<MessageId, ConversationNode>,
    pub root_ids: Vec<MessageId>,
    pub branches: BTreeMap<BranchId, ConversationBranch>,
    pub current_branch_id: BranchId,
    pub next_message_seq: u64,
    pub next_branch_seq: u64,
}
```

`App.messages` remains a cache of `ConversationTree::current_messages()` so the existing UI can be migrated incrementally with minimal churn.

## Sub-Tasks

### Sub-Task 1: Define Branch-Aware Data Structures

- **Status:** Pending
- **Objective:** Add core branch/tree types and operations without changing TUI behavior yet.
- **Related Requirements:** R1, R2, R3, R8
- **Dependencies and Preconditions:** None.
- **In Scope for This Sub-Task:** New state module, ID generation, core tree operations, tests.
- **Out of Scope for This Sub-Task:** Slash commands, UI, persistence wiring into `SavedSession`.

#### Exact Code Changes

1. Add `crates/mimo-state/src/branch.rs`.
2. In `crates/mimo-state/src/lib.rs`, add:
   - `pub mod branch;`
3. In `branch.rs`, define:
   - `pub type MessageId = String;`
   - `pub type BranchId = String;`
   - `ConversationNode`
   - `ConversationBranch`
   - `ConversationTree`
   - `BranchSummary` for `/branches` output.
4. Derive at least:
   - `Debug`, `Clone`, `Serialize`, `Deserialize`, `PartialEq`, `Eq` where practical.
   - Use `#[serde(default)]` for fields that may be absent in older or hand-edited files.
5. Implement these methods on `ConversationTree`:
   - `pub fn new() -> Self`
   - `pub fn from_flat_messages(messages: Vec<ChatMessage>) -> Self`
   - `pub fn is_empty(&self) -> bool`
   - `pub fn current_branch_id(&self) -> &str`
   - `pub fn current_path(&self) -> &[MessageId]`
   - `pub fn current_messages(&self) -> Vec<ChatMessage>`
   - `pub fn add_message_to_current(&mut self, message: ChatMessage) -> MessageId`
   - `pub fn create_branch_from_message(&mut self, message_id: &str) -> anyhow::Result<BranchId>`
   - `pub fn switch_branch(&mut self, branch_id: &str) -> anyhow::Result<()>`
   - `pub fn message_mut(&mut self, message_id: &str) -> Option<&mut ChatMessage>`
   - `pub fn message_id_at_current_index(&self, zero_based_index: usize) -> Option<&str>`
   - `pub fn current_index_of_message(&self, message_id: &str) -> Option<usize>`
   - `pub fn is_branch_point(&self, message_id: &str) -> bool`
   - `pub fn branch_summaries(&self) -> Vec<BranchSummary>`
   - `pub fn branch_count(&self) -> usize`
   - `pub fn replace_current_branch_messages(&mut self, messages: Vec<ChatMessage>)`
   - `pub fn validate_or_repair(&mut self) -> anyhow::Result<()>`
6. ID generation rules:
   - `next_message_seq` starts at `1`; generated IDs are `msg-{seq}`.
   - `next_branch_seq` starts at `1`; the default branch is `branch-1`.
   - `from_flat_messages` creates a single `branch-1` with sequential message nodes.
7. Do not modify `ChatMessage` for branch metadata. Add a comment in `branch.rs` explaining that `ChatMessage` remains API-only transport data.

#### Acceptance Criteria

- `ConversationTree::from_flat_messages(vec![...])` creates one branch with the same message order.
- `create_branch_from_message` preserves the original branch and makes a new branch active at the selected prefix.
- Adding a message after branch creation only extends the new active branch.
- `is_branch_point` returns true after a node has children in more than one branch path.

#### Testing Suggestions

- Add unit tests in `branch.rs` for:
  - empty tree defaults,
  - flat migration,
  - branch creation from middle message,
  - switching branches,
  - adding messages on different branches,
  - replacement of current branch messages for compaction.
- Run: `cargo test -p mimo-state`.
- Run: `cargo check`.

### Sub-Task 2: Migrate Existing Message Handling to Tree Structure

- **Status:** Pending
- **Objective:** Route all production message mutations through `ConversationTree` while preserving current rendering behavior.
- **Related Requirements:** R2, R3, R4, R7, R8
- **Dependencies and Preconditions:** Sub-Task 1 completed.
- **In Scope for This Sub-Task:** `App` state wiring, send/stream/cancel/fail handling, request construction, compaction, clear/load helpers.
- **Out of Scope for This Sub-Task:** New branch commands and branch UI.

#### Exact Code Changes

1. In `crates/mimo-tui/src/app.rs`, update imports:
   - Add `mimo_state::branch::{ConversationTree, MessageId}`.
2. Add fields to `App`:
   - `conversation_tree: ConversationTree`
   - `assistant_message_id: Option<MessageId>`
3. Initialize in `App::new`:
   - `conversation_tree: ConversationTree::new()`
   - `assistant_message_id: None`
4. Add private helper methods on `App`:
   - `fn sync_messages_from_tree(&mut self)` — sets `self.messages = self.conversation_tree.current_messages()` and updates `last_prompt` from the active branch.
   - `fn push_conversation_message(&mut self, message: ChatMessage) -> MessageId` — adds to tree and pushes the same message to `self.messages`.
   - `fn set_streaming_assistant_content(&mut self, content: String)` or `fn append_streaming_assistant_delta(&mut self, delta: &str)` — updates both flat cache and tree node.
   - `fn visible_message_id(&self, one_based_index: usize) -> Option<&str>` — converts user-facing index to active path message ID.
   - `fn reset_conversation_from_messages(&mut self, messages: Vec<ChatMessage>)` — rebuilds tree and visible cache for tests/session migration.
5. Update `send_prompt`:
   - Build `request_messages` from the current active branch before mutating state.
   - Replace direct `self.messages.push(ChatMessage::user(...))` with `push_conversation_message`.
   - Replace direct assistant push with `push_conversation_message(ChatMessage::assistant(String::new()))`.
   - Set both `assistant_index` and `assistant_message_id`.
6. Update `handle_app_event` for `Delta`, `Finished(Ok)`, and `Finished(Err)`:
   - On delta, append to both visible assistant message and tree node.
   - On success/failure, clear both `assistant_index` and `assistant_message_id`.
   - On failure with empty assistant content, write `Request failed: ...` to both locations.
7. Update `cancel_active_stream`:
   - If the assistant message is empty, set `Request cancelled.` in both locations.
   - Clear `assistant_message_id`.
8. Update direct message mutations:
   - `push_system_message` uses `push_conversation_message`.
   - `/context` handler uses `push_system_message(summary)` instead of direct `self.messages.push`.
   - `clear_conversation` clears both `messages` and `conversation_tree`.
   - `apply_loaded_session` will be fully handled in Sub-Task 6, but add a temporary flat migration with `ConversationTree::from_flat_messages(session.messages.clone())` if needed.
9. Update `request_messages`:
   - Continue building base system/plan/attachment messages as now.
   - Append `self.conversation_tree.current_messages()` instead of `self.messages.clone()`.
   - Keep using `ChatMessage::system/user/assistant` constructors.
10. Update `compact_conversation`:
   - Operate on active branch messages only.
   - Build `summary + recent` exactly like the current flat implementation.
   - Call `conversation_tree.replace_current_branch_messages(new_messages.clone())`.
   - Set `self.messages = new_messages` and clear streaming IDs.
11. Update tests in `app.rs` that directly mutate `app.messages`:
   - Prefer `app.reset_conversation_from_messages(vec![...])` or `app.push_conversation_message(...)` so tests keep the tree and flat cache synchronized.

#### Acceptance Criteria

- Existing visible chat behavior is unchanged before any branch commands exist.
- Streaming assistant responses update one transcript entry in both flat and tree state.
- `request_messages()` contains only the active branch transcript, plus existing system/plan/attachment context.
- `/clear`, `/context`, `/retry`, and `/compact` work on the active branch state.

#### Testing Suggestions

- Run: `cargo test -p mimo-tui`.
- Run: `cargo test --workspace` after fixing all compile errors.
- Add/adjust tests for:
  - `clear_conversation` clears the tree,
  - `request_messages` uses tree-backed active messages,
  - delta/failure/cancel update the tree node matching `assistant_message_id`,
  - compaction replaces only active branch messages.

### Sub-Task 3: Implement `/branch` Command

- **Status:** Pending
- **Objective:** Add command parsing and non-interactive branch creation by message number.
- **Related Requirements:** R1, R2, R4
- **Dependencies and Preconditions:** Sub-Task 2 completed.
- **In Scope for This Sub-Task:** `/branch [message-number]`, command help metadata, command palette inclusion through existing `COMMANDS`, App handler.
- **Out of Scope for This Sub-Task:** Ctrl+B selection UI and branch switching commands.

#### Exact Code Changes

1. In `crates/mimo-tui-core/src/commands.rs`:
   - Add `SlashCommand::Branch { message_index: Option<usize> }`.
   - Add a `COMMANDS` entry:
     - `name: "branch"`
     - `aliases: &["b"]`
     - `usage: "/branch [message-number]"`
     - `description: "Create a new conversation branch from a visible message."`
   - Parse `"branch" | "b"`:
     - no args => `message_index: None`,
     - numeric arg => `Some(index)`,
     - non-numeric or extra tokens => usage error.
   - Add command parser tests for `/branch`, `/branch 2`, `/b 2`, and bad usage.
2. In `crates/mimo-tui/src/app.rs`, add handler in `execute_command`:
   - `SlashCommand::Branch { message_index } => self.handle_branch_command(message_index)`.
3. Add `fn handle_branch_command(&mut self, message_index: Option<usize>) -> Result<()>`:
   - Reject while streaming.
   - Reject empty conversations.
   - If `Some(n)`, treat `n` as 1-based visible message number.
   - If `None`, default to the last non-system visible message before the end; prefer the last user message if present.
   - Reject `0`, out-of-bounds, and selected system messages with a status message instead of panic.
   - Call `conversation_tree.create_branch_from_message(message_id)`.
   - Call `sync_messages_from_tree()` so the visible transcript is truncated to the branch point.
   - Clear `assistant_index` and `assistant_message_id`.
   - Set status similar to: `Created branch branch-2 from message 3; type a new prompt to continue.`
4. Add helper `fn default_branch_source_index(&self) -> Option<usize>` for no-arg `/branch`.

#### Acceptance Criteria

- `/branch 2` creates a new active branch from visible message #2.
- `/branch` creates from a sensible latest non-system message and prompts the user to continue.
- Original branch remains listed in tree state and can be switched to once Sub-Task 5 is complete.
- Invalid indexes and system-message selections produce user-facing status messages, not errors or panics.

#### Testing Suggestions

- Run: `cargo test -p mimo-tui-core`.
- Add `mimo-tui` tests for:
  - explicit index creates a branch and truncates visible messages,
  - no-arg branch selects the expected default,
  - invalid index is rejected,
  - system message branch source is rejected.
- Run: `cargo test -p mimo-tui`.

### Sub-Task 4: Implement Branch Selection UI

- **Status:** Pending
- **Objective:** Add keyboard-driven selection mode to choose branch points interactively.
- **Related Requirements:** R1, R2, R6
- **Dependencies and Preconditions:** Sub-Task 3 completed.
- **In Scope for This Sub-Task:** Ctrl+B shortcut, inline selection state, branch point markers, footer/header guidance, key handling.
- **Out of Scope for This Sub-Task:** Full graphical branch tree sidebar or branch diffing.

#### Exact Code Changes

1. In `crates/mimo-tui/src/app.rs`, define:
   - `#[derive(Debug, Default)] struct BranchSelectionState { open: bool, selected: usize }`
2. Add to `App`:
   - `branch_selection: BranchSelectionState`
3. Initialize in `App::new`.
4. Add shortcut helper near other key helpers:
   - `fn opens_branch_selection(key: KeyEvent) -> bool` matching `Ctrl+B`.
5. In `handle_terminal_event`:
   - If `branch_selection.open`, route to `handle_branch_selection_key` before normal prompt editing.
   - Add Ctrl+B handling when no higher-priority overlay is open.
6. Add methods:
   - `fn open_branch_selection(&mut self)` — rejects streaming/empty/no branchable messages; selects the default branch source index.
   - `fn handle_branch_selection_key(&mut self, key: KeyEvent) -> Result<bool>`.
   - `fn move_branch_selection(&mut self, delta: i32)` — skip system messages where practical.
   - `fn confirm_branch_selection(&mut self) -> Result<()>` — calls the same branch creation helper used by `/branch`.
   - `fn cancel_branch_selection(&mut self)`.
7. Update `message_lines()`:
   - Enumerate active messages with 1-based numbers, e.g. `#3 You:`.
   - If `conversation_tree.is_branch_point(message_id)`, add a compact marker such as `◇` or `↳` to the label.
   - If `branch_selection.open && index == selected`, style the label with a distinctive background/foreground and prefix `> `.
   - Keep markdown rendering unchanged for message body lines.
8. Update scroll behavior:
   - When selected message changes, adjust `self.scroll` so the selected label remains visible.
   - Add a lightweight helper that estimates each message label's line offset using the same rendered line counts as `message_lines()`.
9. Update `render_conversation` title:
   - Normal: `Conversation — branch-1`
   - Selection mode: `Select branch point — Enter create | Esc cancel`
10. Update `footer_summary()`:
   - In selection mode, return explicit instructions instead of context count.
   - Otherwise append a compact branch indicator, e.g. `· branch branch-2/3 · Ctrl+B branch`.
11. In `crates/mimo-tui-core/src/keybindings.rs`, add `Ctrl+B` with description `Select a message to create a conversation branch.`

#### Acceptance Criteria

- Ctrl+B enters selection mode only when there is at least one branchable message.
- Up/Down/Home/End move the selected message.
- Enter creates a branch from the selected message, exits selection mode, truncates visible messages to the branch point, and leaves the draft ready for a new prompt.
- Esc/Ctrl+C cancels selection mode without mutating the tree.
- Existing overlays and prompt editing still behave as before when selection mode is closed.

#### Testing Suggestions

- Add `mimo-tui` tests for:
  - `opens_branch_selection` recognizes Ctrl+B only,
  - Ctrl+B opens selection with messages present,
  - Esc closes selection without branch count change,
  - Enter creates a new branch.
- Render test should verify branch title/marker appears in the conversation panel.
- Run: `cargo test -p mimo-tui`.

### Sub-Task 5: Implement Branch Navigation Commands

- **Status:** Pending
- **Objective:** Let users inspect and switch among preserved branches.
- **Related Requirements:** R2, R3, R6, R7
- **Dependencies and Preconditions:** Sub-Task 4 completed.
- **In Scope for This Sub-Task:** `/branches`, `/switch <branch-id>`, branch summaries, active branch UI status, per-branch command behavior.
- **Out of Scope for This Sub-Task:** Branch deletion, renaming, merging, diffing.

#### Exact Code Changes

1. In `crates/mimo-tui-core/src/commands.rs`:
   - Add `SlashCommand::Branches`.
   - Add `SlashCommand::Switch { branch_id: String }`.
   - Add `COMMANDS` entries:
     - `/branches` — `List conversation branches.`
     - `/switch <branch-id>` — `Switch to a conversation branch.`
   - Parse `/branches` with no args.
   - Parse `/switch <branch-id>` requiring exactly one non-empty token.
   - Add parser tests.
2. In `App::execute_command`, handle:
   - `SlashCommand::Branches => self.show_branches()`.
   - `SlashCommand::Switch { branch_id } => self.switch_branch(&branch_id)`.
3. Add `fn show_branches(&mut self)`:
   - Build a readable branch list from `conversation_tree.branch_summaries()`.
   - Include current marker, branch ID, message count, branch point/source, and short last-message preview.
   - Use `push_system_message` for consistency with `/status` and other informational commands.
4. Add `fn switch_branch(&mut self, branch_id: &str) -> Result<()>`:
   - Reject while streaming.
   - Call `conversation_tree.switch_branch(branch_id)`.
   - Call `sync_messages_from_tree()`.
   - Clear `assistant_index`, `assistant_message_id`, transient tool approvals, and selection mode.
   - Reset or clamp scroll to the bottom.
   - Set status `Switched to branch branch-N`.
5. Update `status_summary()` and `config_summary()` or footer only:
   - At minimum, include active branch and branch count in `status_summary()`.
6. Verify existing command behavior:
   - `/retry` uses the last user prompt from the active branch after `sync_messages_from_tree()`.
   - `/compact` uses `replace_current_branch_messages` and does not mutate inactive branch paths.
   - `/undo` and `/restore` remain workspace snapshot commands and push their system messages into the active branch.

#### Acceptance Criteria

- `/branches` lists all branch records and marks the current one.
- `/switch branch-1` changes the active visible transcript.
- Switching branches updates `last_prompt` to the active branch's last user prompt.
- Invalid branch IDs produce a user-facing status/error without partial state changes.

#### Testing Suggestions

- Run: `cargo test -p mimo-tui-core`.
- Add `mimo-tui` tests for:
  - `/branches` output includes current marker and IDs,
  - `/switch` changes visible messages,
  - `/retry` after switch uses active branch prompt,
  - `/compact` only changes active branch.
- Run: `cargo test -p mimo-tui`.

### Sub-Task 6: Implement Session Persistence for Branches

- **Status:** Pending
- **Objective:** Save/load branch trees while preserving compatibility with legacy flat sessions.
- **Related Requirements:** R3, R5, R7
- **Dependencies and Preconditions:** Sub-Task 5 completed.
- **In Scope for This Sub-Task:** `SavedSession` schema extension, save/load wiring, session picker/CLI branch counts, migration tests.
- **Out of Scope for This Sub-Task:** Branch export/import as separate files.

#### Exact Code Changes

1. In `crates/mimo-state/src/session_store.rs`, import `ConversationTree`.
2. Extend `SavedSession`:
   - Add `#[serde(default, skip_serializing_if = "Option::is_none")] pub conversation_tree: Option<ConversationTree>`.
   - Keep `pub messages: Vec<ChatMessage>` as the active branch snapshot and legacy compatibility field.
3. Extend `SessionEntry`:
   - Add `pub branch_count: usize`.
   - Optionally add `pub active_branch_id: Option<String>` for display.
4. Update `save_session` signature:
   - Add `conversation_tree: Option<&ConversationTree>` near the `messages` parameter.
   - Store `conversation_tree.cloned()` in `SavedSession`.
   - Continue deriving the title from active `messages`.
5. Update the `/save` call in `App::execute_command`:
   - Pass `Some(&self.conversation_tree)`.
6. Update `session_store` tests and any other call sites:
   - Existing tests may pass `None` to verify legacy flat behavior.
   - Add a new test that passes a branch tree and verifies JSON round-trip.
7. Update `load_session` behavior in `App::apply_loaded_session`:
   - If `session.conversation_tree` is `Some(mut tree)`, call `tree.validate_or_repair()`.
   - If the tree is valid, assign it to `self.conversation_tree` and sync messages from it.
   - If missing, build `ConversationTree::from_flat_messages(session.messages.clone())`.
   - If invalid but flat messages exist, fall back to flat migration and set a warning status suffix.
8. Update `list_sessions`:
   - Set `message_count` to active flat `messages.len()` as today.
   - Set `branch_count` from `session.conversation_tree.as_ref().map_or(1, ConversationTree::branch_count)`.
9. Update `crates/mimo-tui/src/session_picker.rs`:
   - Include branch count in rendered entries, e.g. `6 messages | 3 branches`.
10. Update `crates/mimo-cli/src/main.rs` session listing if desired:
   - Include branch count without changing the command contract, e.g. `... | 3 branches | path`.
11. Update `export_markdown` only minimally:
   - Keep exporting the active branch messages passed by `App`.
   - Add a short note at the top when branch count > 1 only if the signature is already being changed; otherwise leave export unchanged to avoid extra churn.

#### Acceptance Criteria

- New saved sessions include `conversation_tree` and can restore active branch, inactive branches, and branch IDs.
- Old session JSON without `conversation_tree` still loads into a single branch.
- Session picker shows branch counts.
- Loading a session clears streaming state and branch selection state.

#### Testing Suggestions

- Run: `cargo test -p mimo-state`.
- Add tests for:
  - legacy flat session deserialization,
  - branch tree save/load round-trip,
  - session list branch count,
  - corrupted/missing tree fallback if practical.
- Run: `cargo test -p mimo-tui` for `apply_loaded_session` behavior.

### Sub-Task 7: Update Documentation and Help System

- **Status:** Pending
- **Objective:** Document branch commands, shortcut, and behavior.
- **Related Requirements:** R1, R3, R4, R6
- **Dependencies and Preconditions:** Sub-Task 6 completed.
- **In Scope for This Sub-Task:** `/help`, keybinding list, README, concise examples.
- **Out of Scope for This Sub-Task:** Screenshots, branch merge/diff documentation.

#### Exact Code Changes

1. In `crates/mimo-tui-core/src/commands.rs`:
   - Ensure `COMMANDS` metadata for `/branch`, `/branches`, and `/switch` is concise and accurate.
   - `command_help` will pick this up automatically.
2. In `crates/mimo-tui-core/src/keybindings.rs`:
   - Ensure Ctrl+B is documented.
3. In `README.md`:
   - Add a highlight bullet for conversation branching.
   - Add `Ctrl+B`, `/branch`, `/branches`, and `/switch` to the essential controls table.
   - Add a short “Conversation branching” section with examples:
     - `Ctrl+B`, select message, Enter, type new prompt.
     - `/branch 3` then type a new prompt.
     - `/branches` and `/switch branch-1`.
   - Mention branches are saved with sessions.
4. Update `AGENTS.md` only if new validation commands or repo conventions are introduced. This feature should not require changes there.

#### Acceptance Criteria

- `/help` and help overlay list branch commands.
- Help overlay lists Ctrl+B.
- README explains the feature without mentioning unsupported merge/diff behavior.

#### Testing Suggestions

- Run: `cargo test -p mimo-tui-core` to catch command metadata/parser regressions.
- Manually inspect `/help branch`, `/help branches`, `/help switch`, and help overlay search for “branch”.

## Final Integration and Verification

### Required Automated Checks

Run from the repository root:

```bash
cargo fmt --check
cargo check
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

### Manual End-to-End Check

1. Start the TUI with `cargo run`.
2. Send 3-4 prompts and wait for responses.
3. Press Ctrl+B.
4. Move to an earlier non-system message and press Enter.
5. Confirm the visible transcript truncates to that message and the footer/header shows the new branch.
6. Type a new prompt and wait for the response.
7. Run `/branches`; verify original and new branches are listed.
8. Run `/switch branch-1`; verify original branch content returns.
9. Run `/switch <new-branch-id>`; verify branched content returns.
10. Run `/retry`; verify it retries the active branch's last user prompt.
11. Run `/compact`; verify only the active branch is compacted.
12. Run `/save`, restart the TUI, then `/load`; verify all branches and active branch state are restored.
13. Load a legacy session JSON without `conversation_tree`; verify it opens as a single branch.

### Completion Checklist

- [ ] All 7 sub-tasks are implemented in order.
- [ ] Branch metadata is not serialized to MiMo API request messages.
- [ ] Active branch cache (`App.messages`) and `ConversationTree` stay synchronized.
- [ ] Streaming, failure, and cancellation update the tree-backed assistant message.
- [ ] Legacy sessions load successfully.
- [ ] New sessions restore inactive branches and current active branch.
- [ ] Help/docs describe the final commands and shortcut.
- [ ] Full workspace validation passes.

## Open Questions

- Should future work add branch naming? Current plan intentionally uses stable generated IDs only.
- Should future work support branch deletion or merging? Both are out of scope for this feature.
