# Plan: Link MiMo models to modes

## Objective

Make MiMo-TUI maintain a separate model selection for each interaction mode (`plan` and `agent`), automatically switch the active API model when the mode changes, and persist those selections in both config-backed preferences and saved sessions.

## Requirements Snapshot

- **R1:** Each `AppMode` (`Plan`, `Agent`) must have its own selected model.
- **R2:** Built-in defaults are Plan → `mimo-2.5` and Agent → `mimo-2.5-pro`.
- **R3:** Switching modes must update the active model to the selected model for the new mode.
- **R4:** `/models` and `/model` must update only the currently selected mode's model.
- **R5:** Mode-specific model selections must persist across sessions.
- **R6:** Preserve the existing API client contract where `MimoClient` reads the active request model from `AppConfig.model`.
- **R7:** Preserve existing config precedence: CLI > env > file > built-in. Existing legacy `model` config/session fields should remain backwards-compatible.
- **R8:** Avoid introducing a crate dependency cycle while making `AppConfig` store `HashMap<AppMode, String>`.

## Scope

- In scope: config structure/loading/persistence, `AppMode` hashability/type ownership, TUI mode switching, TUI model picker and `/model` command behavior, session save/load format, CLI `doctor`/`models` output, and tests.
- Out of scope: changing MiMo HTTP request shape, changing auto-routing internals beyond preserving `auto`, adding new modes, changing tool permissions, or changing conversation/message persistence beyond the model fields.

## Assumptions and Constraints

- `.opencode/task.md` is absent; this plan is based on the user request plus repository inspection.
- The current source paths differ slightly from the user's file list: TUI app code lives in `crates/mimo-tui/src/app.rs`, and command handlers live in `crates/mimo-tui/src/app/command_handlers.rs`.
- The repo currently uses v-prefixed model IDs (`mimo-v2.5`, `mimo-v2.5-pro`) in `KNOWN_MIMO_MODELS`, while the request names `mimo-2.5` and `mimo-2.5-pro`. Implement the requested literals only if they are confirmed valid API IDs; otherwise use the existing v-prefixed IDs consistently and document the correction in tests/comments.
- `AppConfig.model` is currently a public `String` field used throughout the workspace. To minimize churn and satisfy R6, treat it as the active model cache synchronized from `mode_models[current_mode]`, rather than replacing every use with a new getter in the first pass.
- `mimo-config` currently cannot depend on `mimo-state` because `mimo-state` already depends on `mimo-config`; implementing `HashMap<AppMode, String>` directly in `AppConfig` requires moving or re-exporting `AppMode` so the type is available without a cycle.

## Risks and Areas Requiring Care

- **Dependency cycle risk:** Do not add `mimo-state` as a dependency of `mimo-config`. Move `AppMode` to a lower-level crate or to `mimo-config` and re-export it from `mimo-state`.
- **Legacy data risk:** Existing `config.toml` files and saved sessions only have `model`; loading them must produce sensible `mode_models` without losing the old selection.
- **Stale active model risk:** Every path that changes `self.mode`, loads a session, applies a model picker selection, or queues/sends a request must keep `self.config.model` synchronized.
- **Persistence ambiguity:** Once `[mode_models]` exists, avoid continuing to write a conflicting legacy `model` key during model-picker saves.
- **HashMap serialization:** TOML/JSON object key order may not be stable. If deterministic output becomes important, use `BTreeMap` instead, but the requested design explicitly says `HashMap`.

## Core Concepts

The persistent source of truth should become a per-mode map:

```toml
[mode_models]
plan = "mimo-2.5"
agent = "mimo-2.5-pro"
```

At runtime:

- `AppConfig.mode_models` owns the selected model for each mode.
- `App.mode` owns the currently selected mode.
- `AppConfig.model` remains the active model cache used by `MimoClient` and UI display.
- `App::set_mode(mode)` updates both `self.mode` and `self.config.model` from `self.config.mode_models[mode]`.
- `/model <id>` and the `/models` picker call a config helper that updates only `mode_models[self.mode]`; if that mode is active, it also updates `config.model`.

## Sub-Tasks

### Sub-Task 1: Make `AppMode` usable by config without a crate cycle

- **Status:** Pending
- **Objective:** Allow `crates/mimo-config` to store `HashMap<AppMode, String>` while preserving existing `mimo_state::AppMode` imports.
- **Related Requirements:** R1, R8
- **Dependencies and Preconditions:** Existing dependency graph: `mimo-state -> mimo-config`; `mimo-config` must not depend on `mimo-state`.
- **In Scope for This Sub-Task:**
  - `crates/mimo-config/src/lib.rs`: introduce the canonical `AppMode` type here, or in another lower-level crate already depended on by both config and state.
  - `crates/mimo-state/src/models.rs`: re-export the canonical `AppMode` so current callers can continue using `mimo_state::AppMode`.
  - Ensure `AppMode` derives or implements `Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`, `Hash`, `Serialize`, `Deserialize`, `Default`, and `Display`.
  - Preserve current deserialization compatibility: `"plan"` → `Plan`, `"agent"`/`"chat"` → `Agent`, unknown values → `Agent`.
- **Out of Scope for This Sub-Task:** Adding mode-model config behavior; this sub-task only resolves the shared type.
- **Instructions:**
  1. Prefer defining `AppMode` in `mimo-config` to avoid new dependencies and keep mode-model config self-contained.
  2. In `crates/mimo-state/src/models.rs`, replace the local enum definition with `pub use mimo_config::AppMode;` or an equivalent re-export.
  3. Keep existing `AppMode` tests either in `mimo-state` through the re-export or move them to `mimo-config`.
  4. Do not add a `mimo-config -> mimo-state` dependency.
- **Acceptance Criteria:** `AppConfig` can name `AppMode` directly, existing `mimo_state::AppMode` imports still compile, and `AppMode` can be used as a `HashMap` key.
- **Cautionary Points (Risks & Edge Cases):** Moving a public enum changes where serde/display tests should live; preserve serialized strings exactly.
- **Implementation Suggestions:** If moving the enum to `mimo-config` feels architecturally too config-specific, use `mimo-protocol` instead; both `mimo-config` and `mimo-state` already depend on it.
- **Testing Suggestions:** Run targeted tests for `mimo-config` and `mimo-state` after moving/re-exporting the type.
- **Done When:** The workspace compiles past all `AppMode` imports and no dependency cycle is introduced.

### Sub-Task 2: Add mode-specific model storage and helpers to config

- **Status:** Pending
- **Objective:** Make `AppConfig` load, expose, and persist per-mode model selections while preserving the active `model` field.
- **Related Requirements:** R1, R2, R6, R7
- **Dependencies and Preconditions:** Sub-Task 1 completed or an equivalent no-cycle type location chosen.
- **In Scope for This Sub-Task:**
  - `crates/mimo-config/src/lib.rs`.
  - Add `use std::collections::HashMap`.
  - Add `mode_models: Option<HashMap<AppMode, String>>` to `ConfigOverrides`.
  - Add `pub mode_models: HashMap<AppMode, String>` and, if following existing source-tracking patterns, `pub mode_models_source: ConfigValueSource` to `AppConfig`.
  - Add `mode_models: Option<HashMap<AppMode, String>>` to `FileConfig` with serde default/skip behavior.
  - Add built-in mode defaults via constants such as `DEFAULT_PLAN_MODEL` and `DEFAULT_AGENT_MODEL`; keep `DEFAULT_MODEL` as the default active/agent model if existing callers/tests still need it.
  - Add helper methods for mode-model behavior.
- **Out of Scope for This Sub-Task:** TUI command behavior and session persistence.
- **Instructions:**
  1. Build `mode_models` in `AppConfig::load` using the existing precedence model:
     - Start with built-in defaults for both `Plan` and `Agent`.
     - If a legacy file `model` exists and `mode_models` is absent, apply that legacy model to both mode entries for backwards compatibility.
     - Overlay file `mode_models` entries, filling any missing mode with the built-in default.
     - Overlay `MIMO_MODEL` and CLI `--model` as legacy/global runtime overrides; for backwards compatibility, apply them to both mode entries for the loaded runtime config.
     - Overlay `ConfigOverrides.mode_models` last when present.
  2. Set the active `AppConfig.model` after resolving the map. Use `Agent` as the initial active mode because both TUI startup and CLI `ask` currently start in agent behavior.
  3. Add helpers similar to:
     - `default_mode_models() -> HashMap<AppMode, String>`.
     - `model_for_mode(&self, mode: AppMode) -> &str`.
     - `sync_model_for_mode(&mut self, mode: AppMode)` to update the active `model` cache without file I/O.
     - `set_model_for_mode(&mut self, mode: AppMode, model: String) -> Result<()>` to update the map, persist `[mode_models]`, clear or ignore legacy `model`, and sync `self.model` when appropriate.
     - Optional `set_mode_models_for_session(&mut self, mode_models: HashMap<AppMode, String>, active_mode: AppMode)` for session load without writing to config.
  4. Keep `set_model` only as a backwards-compatible wrapper if needed by remaining call sites; prefer migrating TUI callers to `set_model_for_mode`.
  5. Ensure `known_mimo_models` includes the new default model IDs and still includes `auto` plus the current/custom model.
- **Acceptance Criteria:** `AppConfig::load` always returns a map containing both `Plan` and `Agent`, and `config.model` equals the initial active mode's model.
- **Cautionary Points (Risks & Edge Cases):** Decide and test whether the requested non-v model IDs are correct. If they are not valid API IDs, use the existing `mimo-v2.5`/`mimo-v2.5-pro` IDs instead.
- **Implementation Suggestions:** Clear `file_config.model` when writing `mode_models` through `set_model_for_mode` to avoid future ambiguity in `config.toml`.
- **Testing Suggestions:** Add config tests for built-in defaults, file `[mode_models]`, legacy `model`, missing map entries, and CLI/env override behavior.
- **Done When:** Config loading and config-file writes support mode-specific models without breaking existing global-model configs.

### Sub-Task 3: Persist mode models in saved sessions

- **Status:** Pending
- **Objective:** Save and restore the full per-mode model map with sessions while keeping legacy sessions loadable.
- **Related Requirements:** R5, R7
- **Dependencies and Preconditions:** Sub-Tasks 1 and 2.
- **In Scope for This Sub-Task:**
  - `crates/mimo-state/src/session_store.rs`.
  - Test helper `AppConfig` literals in state-store tests.
- **Out of Scope for This Sub-Task:** TUI session-application behavior, which is handled in Sub-Task 4.
- **Instructions:**
  1. Add `#[serde(default)] pub mode_models: HashMap<AppMode, String>` to `SavedSession`.
  2. Keep the existing `model: String` field as the active/legacy model for compatibility and session list display.
  3. In `save_session`, set `mode_models` from `config.mode_models.clone()`; no signature change is needed because `save_session` already receives `&AppConfig`.
  4. In `list_sessions`, continue displaying `session.model` as the active saved model. Optionally fill it from `mode_models[session.mode]` if `model` is empty in future data.
  5. Add a small helper in this module or config to fill missing session map entries from defaults and to use legacy `model` for the saved active `mode` when `mode_models` is absent.
  6. Update session tests to include the new `AppConfig` fields and add a regression test that saved JSON includes `mode_models` and loads it back.
  7. Add a legacy-session test with no `mode_models` field to confirm deserialization still succeeds.
- **Acceptance Criteria:** Newly saved sessions include both the active `model` and `mode_models`; old sessions with only `model` and `mode` still load.
- **Cautionary Points (Risks & Edge Cases):** JSON object keys for enum-keyed maps should serialize as `"plan"` and `"agent"`; verify this with a test.
- **Implementation Suggestions:** Prefer centralizing fallback/default filling in `mimo-config` so config and session load use identical rules.
- **Testing Suggestions:** `cargo test -p mimo-state session` or the closest available targeted session tests.
- **Done When:** Session persistence round-trips per-mode model selections and legacy sessions remain compatible.

### Sub-Task 4: Synchronize TUI mode changes with the active model

- **Status:** Pending
- **Objective:** Make every TUI mode change update `self.config.model` from `self.config.mode_models` before rendering or sending requests.
- **Related Requirements:** R1, R3, R5, R6
- **Dependencies and Preconditions:** Sub-Tasks 1-3.
- **In Scope for This Sub-Task:**
  - `crates/mimo-tui/src/app.rs`.
  - TUI app tests in the same file.
- **Out of Scope for This Sub-Task:** Model picker and `/model` command save behavior, which is handled in Sub-Task 5.
- **Instructions:**
  1. In `App::new`, take `config` as mutable locally, set `mode = AppMode::Agent`, then call `config.sync_model_for_mode(mode)` before storing it.
  2. Change `fn set_mode(&mut self, mode: AppMode)` so it sets `self.mode = mode` and then calls `self.config.sync_model_for_mode(mode)`.
  3. Update all mode-switching call sites to rely on `set_mode`: `/mode`, `toggle_mode`, command palette mode action, approval-key mode changes, and session load.
  4. Update mode-switch status strings to include the active model, for example `Mode switched to plan (model: ...)`, so users can see R3 happening.
  5. In `send_prompt`, ensure the active model is synchronized for `self.mode` before constructing `MimoClient`. This should be redundant after `set_mode`, but protects future call paths.
  6. In `enqueue_background_task` and `spawn_background_task`, ensure the stored task model and cloned config reflect the task's mode model.
  7. In `apply_loaded_session`, read the loaded `mode_models`, merge/fill defaults, update in-memory `self.config.mode_models`, then call `self.set_mode(mode)`. For legacy sessions with an empty map, insert the legacy `model` for the loaded `mode` before syncing.
  8. Remove the old direct assignment `self.config.model = model` from session load except as part of legacy fallback map construction.
  9. Update `config_summary` and `status_summary` to show both active model and per-mode model selections.
- **Acceptance Criteria:** Switching from Agent to Plan changes `self.config.model` to the plan model; switching back restores the agent model; sends use the active model.
- **Cautionary Points (Risks & Edge Cases):** Avoid file writes during `set_mode`; mode switches should only update runtime state, not persist config.
- **Implementation Suggestions:** Add a local helper such as `current_mode_model(&self) -> &str` only if it makes call sites clearer; avoid broad refactors.
- **Testing Suggestions:** Add/adjust TUI tests for F2/Ctrl+Tab mode toggles to assert both `app.mode` and `app.config.model` after each toggle.
- **Done When:** All runtime mode transitions visibly and internally sync the active model.

### Sub-Task 5: Update model picker and model commands to save the current mode only

- **Status:** Pending
- **Objective:** Ensure `/models`, `/model`, and related config-command paths update only the active mode's entry in `mode_models`.
- **Related Requirements:** R1, R4, R5
- **Dependencies and Preconditions:** Sub-Tasks 2 and 4.
- **In Scope for This Sub-Task:**
  - `crates/mimo-tui/src/app.rs`.
  - `crates/mimo-tui/src/app/command_handlers.rs`.
  - Optional but recommended: `crates/mimo-tui-core/src/commands.rs` command descriptions/help text.
- **Out of Scope for This Sub-Task:** Remote model catalog API behavior.
- **Instructions:**
  1. In `apply_selected_model`, replace `self.config.set_model(model.clone())?` with `self.config.set_model_for_mode(self.mode, model.clone())?`.
  2. Update the picker status to include the mode, for example `Plan model switched to ...`.
  3. In `refresh_model_picker_catalog`, use the current mode's active model (`self.config.model` after sync or `self.config.model_for_mode(self.mode)`) as the selected/highlighted model.
  4. In `render_model_picker_overlay`, consider changing the title/subtitle to `MiMo models — plan` or `Select a model for plan mode` so R4 is clear.
  5. In `handle_model_command(None)`, show the current mode's model and optionally both mode selections.
  6. In `handle_model_command(Some(model))`, validate with `normalize_model_name`, then call `set_model_for_mode(self.mode, model.clone())` and update status with the mode.
  7. In `handle_config_command(ConfigCommand::SetModel)`, either:
     - update it to save the current mode's model with the same helper, or
     - deprecate it in favor of `/model` and make the status explicitly say how to set mode-specific models.
     Recommended: use the same current-mode behavior to avoid any remaining global model mutation path.
  8. If help text is updated, change `/model` and `/models` descriptions to mention the current mode's model.
  9. Keep `auto` support unchanged: selecting `auto` for one mode should not force `auto` in the other mode.
- **Acceptance Criteria:** Selecting a model in Plan changes only `mode_models[Plan]`; selecting a model in Agent changes only `mode_models[Agent]`; the other mode's model remains unchanged.
- **Cautionary Points (Risks & Edge Cases):** Existing tests may write to `config.toml` in the workspace when using `PathBuf::from("config.toml")`; use temp config paths for any new tests that exercise persistence.
- **Implementation Suggestions:** Add an assertion in the model-picker test that the inactive mode still has its previous model after applying a selection.
- **Testing Suggestions:** Add tests for `/model mimo-...` in each mode and for picker application in each mode.
- **Done When:** There are no TUI paths left that call the old global `set_model` for interactive model selection.

### Sub-Task 6: Update CLI `doctor` and `models` commands

- **Status:** Pending
- **Objective:** Make CLI output reflect the active model and per-mode configured models.
- **Related Requirements:** R2, R5, R6, R7
- **Dependencies and Preconditions:** Sub-Task 2.
- **In Scope for This Sub-Task:**
  - `crates/mimo-cli/src/main.rs`.
  - CLI tests if any are added or affected.
- **Out of Scope for This Sub-Task:** Adding a CLI mode flag or changing `ask` semantics.
- **Instructions:**
  1. Update `AppConfig::load(ConfigOverrides { ... })` call sites to initialize the new `mode_models` override field to `None`.
  2. Leave `ask` using `config.model`; because config load initializes the active model as Agent, one-shot `ask` should use the Agent default/model.
  3. In `doctor`, keep the existing active `Model` line and add a per-mode summary, for example:
     - `Plan model      : ...`
     - `Agent model     : ...`
     Include source info if `mode_models_source` was added.
  4. In `list_models`, ensure local fallback suggestions include `auto`, known models, `config.model`, and both values from `config.mode_models`.
  5. In `list_sessions`, the existing `session.model` display can remain as the active saved model; optionally include `session.mode` before model if readability improves.
- **Acceptance Criteria:** `cargo run -- doctor` shows enough information to verify both mode-specific model selections, and `cargo run -- models` includes configured custom mode models when offline.
- **Cautionary Points (Risks & Edge Cases):** Do not add `--mode` or mode-specific CLI configuration unless explicitly requested.
- **Implementation Suggestions:** If list fallback logic becomes duplicated, add a config helper that returns known models plus all configured mode models.
- **Testing Suggestions:** Update existing CLI unit tests only if signatures change; otherwise rely on `cargo check` plus a manual `doctor` smoke test.
- **Done When:** CLI paths compile and expose per-mode model state without changing API request logic.

### Sub-Task 7: Update test fixtures and add regression coverage

- **Status:** Pending
- **Objective:** Ensure the new fields and behavior are covered across config, state, TUI, and CLI crates.
- **Related Requirements:** R1-R8
- **Dependencies and Preconditions:** Sub-Tasks 1-6.
- **In Scope for This Sub-Task:**
  - All `AppConfig { ... }` test literals found in state-store modules and `crates/mimo-tui/src/app.rs`.
  - Targeted tests for config/session/TUI behavior.
- **Out of Scope for This Sub-Task:** Broad unrelated refactors or snapshot-test rewrites.
- **Instructions:**
  1. Update all test `AppConfig` literals with `mode_models` and `mode_models_source` if added.
  2. Add config tests for:
     - default Plan/Agent model map;
     - legacy `model = "..."` fallback;
     - `[mode_models]` file config;
     - missing one mode entry fills from defaults;
     - `auto` accepted per mode.
  3. Add session tests for:
     - new sessions serialize/deserialize `mode_models`;
     - legacy sessions without `mode_models` still load;
     - loaded active mode uses the expected model.
  4. Add TUI tests for:
     - `set_mode`/F2/Ctrl+Tab sync `config.model`;
     - `/model` changes current mode only;
     - picker application changes current mode only;
     - session load restores mode map and active model.
  5. Update any assertions that expected the old single global default model.
- **Acceptance Criteria:** Tests demonstrate mode-model separation, mode-switch sync, and backwards compatibility.
- **Cautionary Points (Risks & Edge Cases):** Tests that mutate config should use temp directories to avoid writing `config.toml` in the repo root.
- **Implementation Suggestions:** Add small helpers in tests to create `mode_models` maps to avoid repeated boilerplate.
- **Testing Suggestions:** Run targeted crate tests first, then full workspace tests.
- **Done When:** The new behavior is protected by regression tests and all config literals compile.

### Sub-Task 8: Final integration and cleanup

- **Status:** Pending
- **Objective:** Verify no stale global-model assumptions remain and the workspace passes standard checks.
- **Related Requirements:** R1-R8
- **Dependencies and Preconditions:** Sub-Tasks 1-7.
- **In Scope for This Sub-Task:** Final search, formatting, compile, lint, and workspace tests.
- **Out of Scope for This Sub-Task:** New features beyond mode-specific model selection.
- **Instructions:**
  1. Search for remaining `set_model(` call sites and confirm none are interactive TUI model-selection paths unless intentionally retained as compatibility wrappers.
  2. Search for direct `self.config.model =` assignments and confirm only config helpers or tightly controlled legacy/session fallback code remain.
  3. Search for `known_mimo_models(&config.model)` and similar single-model fallback usage; include mode map values where user-facing model catalogs are built.
  4. Confirm no `mimo-config -> mimo-state` dependency was added.
  5. Run the verification commands below.
- **Acceptance Criteria:** No stale global-only behavior remains in mode switching, model selection, session load, or CLI model display.
- **Cautionary Points (Risks & Edge Cases):** Clippy may flag large config constructors or unused compatibility helpers after call sites are migrated; clean those up.
- **Implementation Suggestions:** Keep the first PR focused on behavior and tests; defer cosmetic UI copy changes unless necessary for clarity.
- **Testing Suggestions:** Use the commands in Final Integration & Verification.
- **Done When:** The implementation is complete, tested, and ready for review.

## File-by-File Change Summary

- `crates/mimo-config/src/lib.rs`:
  - Add/rehome canonical `AppMode` if using config as the owner.
  - Add `HashMap<AppMode, String>` mode-model storage to `ConfigOverrides`, `FileConfig`, and `AppConfig`.
  - Add Plan/Agent default constants and mode-model helper methods.
  - Update `AppConfig::load`, config persistence, known-model catalog tests, and config tests.
- `crates/mimo-state/src/models.rs`:
  - Add `Hash` support if the enum remains here, or re-export the canonical `AppMode` if moved to avoid the config/state cycle.
  - Preserve serde/display compatibility.
- `crates/mimo-tui/src/app.rs`:
  - Sync active `config.model` on startup, mode switches, request send, background task enqueue/spawn, picker refresh, and session load.
  - Save sessions with `config.mode_models` through `session_store`.
  - Update status/config summaries and tests.
- `crates/mimo-state/src/session_store.rs`:
  - Add `SavedSession.mode_models` with serde default.
  - Save the map from config and load legacy sessions safely.
  - Add/adjust tests.
- `crates/mimo-tui/src/app/command_handlers.rs`:
  - Update `/model` and `/config model` behavior to write only the current mode's model.
  - Update status text.
- `crates/mimo-cli/src/main.rs`:
  - Populate new config override fields.
  - Print per-mode models in `doctor`.
  - Include configured mode models in offline `models` output.
- Additional compile-driven updates likely needed:
  - `crates/mimo-state/src/*_store.rs` test helpers with `AppConfig` literals.
  - Optional command help text in `crates/mimo-tui-core/src/commands.rs`.

## Final Integration & Verification

- **System-Wide Test:**
  1. Start with no model config and run `cargo run -- doctor`; confirm active model is the Agent default and both Plan/Agent model defaults are shown.
  2. Launch `cargo run`, confirm the header/landing model is the Agent model.
  3. Switch to Plan with F2/Ctrl+Tab or `/mode plan`; confirm the displayed model changes to the Plan model.
  4. Use `/model <id>` or `/models` in Plan; switch to Agent and confirm Agent model did not change; switch back to Plan and confirm the selected Plan model persisted.
  5. Save a session, change both mode models, load the session, and confirm both mode models and the active model are restored from the session.
  6. Test a legacy session/config containing only `model` and confirm it loads without error.
- **Validation Commands:**
  - `cargo fmt --check`
  - `cargo check`
  - `cargo clippy --workspace -- -D warnings`
  - `cargo test --workspace`
- **Completion Checklist:**
  - [ ] `AppConfig.mode_models` exists and contains both modes after load.
  - [ ] `AppConfig.model` is synchronized from the active mode before API requests.
  - [ ] Mode switches update the displayed and request model.
  - [ ] `/model` and `/models` update only the current mode.
  - [ ] Saved sessions include `mode_models` and old sessions still load.
  - [ ] CLI `doctor` shows per-mode models.
  - [ ] No config/state dependency cycle exists.
  - [ ] Workspace checks pass.

## Open Questions

- Are the required default IDs literally `mimo-2.5` and `mimo-2.5-pro`, or should they be the repo's existing `mimo-v2.5` and `mimo-v2.5-pro`? Confirm before implementation or make the change consistently in defaults, known-model lists, and tests.
