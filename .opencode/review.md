# Code Review Summary

**Scope**: Model-mode linking — each `AppMode` (Plan/Agent) bound to its own model with per-mode persistence, mode-switch syncing, session persistence, and `/model` scoping to the active mode.
**Overall risk**: Medium
**Verdict**: Approve with comments

## Findings

### [P0] Blocking

_None._

### [P1] High

_None._

### [P2] Medium

- **Session-loaded `mode_models` leak into the config file on the next `/model` change**
  - **Location**: `crates/mimo-config/src/lib.rs:284-302` (`set_model_for_mode`), combined with `crates/mimo-tui/src/app.rs:3747` (`apply_loaded_session`)
  - **Why it matters**: Loading a saved session replaces the in-memory `self.config.mode_models` with the session's map (line 3747), but does **not** touch the config file. The file and in-memory state now diverge. The next time the user runs `/model X` (or picks a model), `set_model_for_mode` rebuilds the file's `mode_models` from `default_mode_models()` extended with `self.mode_models` (the session-contaminated in-memory map) and writes the whole map to disk. This silently overwrites the **non-target** mode's persisted model in the user's config file with a value that came from the session, and the change survives restarts.
  - **Evidence**: Trace the flow:
    1. User config file: `mode_models = { plan = "cheap", agent = "cheap" }`.
    2. User opens the session picker and loads a session saved with `mode_models = { plan = "mimo-v2.5-pro", agent = "mimo-v2.5-pro" }`.
    3. `apply_loaded_session` sets `self.config.mode_models = { plan: "mimo-v2.5-pro", agent: "mimo-v2.5-pro" }` (in memory only; file unchanged).
    4. User switches to Plan mode and runs `/model mimo-v2.5`.
    5. `set_model_for_mode(Plan, "mimo-v2.5")` builds `mode_models = defaults ∪ self.mode_models ∪ {Plan: "mimo-v2.5"}` = `{ plan: "mimo-v2.5", agent: "mimo-v2.5-pro" }` and writes that to the config file with `model = None`.
    6. The user's config file `agent` slot is now `"mimo-v2.5-pro"` — a value they never chose for their config. On restart, Agent mode uses the pro model.
    The harm is a silent, persistent mutation of a config value the user did not ask to change (potentially routing Agent requests to a different/costlier model after restart). No test covers this load-session-then-change-model sequence.
  - **Fix**: Make `set_model_for_mode` read-modify-write the **file's** own `mode_models` inside the `update_file_config` closure, so only the target mode changes on disk. Keep the in-memory update scoped to the target mode so runtime state (session-derived) is preserved without being flushed back to the file. To preserve the existing legacy-`model` migration behavior, seed the file map from `file_config.model` when `file_config.mode_models` is absent, e.g.:
    ```rust
    update_file_config(&self.config_path, |file_config| {
        let mut mode_models = match file_config.mode_models.take() {
            Some(mm) => mm,
            None => match file_config.model.as_ref().and_then(normalize_model_value) {
                Some(legacy) => HashMap::from([
                    (AppMode::Plan, legacy.clone()),
                    (AppMode::Agent, legacy),
                ]),
                None => Self::default_mode_models(),
            },
        };
        for mode in [AppMode::Plan, AppMode::Agent] {
            mode_models.entry(mode).or_insert_with(|| default_model_for_mode(mode).to_string());
        }
        mode_models.insert(mode, model.clone());
        file_config.mode_models = Some(mode_models);
        file_config.model = None;
    })?;
    self.mode_models.insert(mode, model.clone());
    self.model = model;
    ```
    Add a regression test: load a session whose `mode_models` differs from the file's, run `/model` for one mode, and assert the file's other-mode model is unchanged.

### [P3] Low

- **`AppConfig::set_model` is dead code and a latent footgun under the new model**
  - **Location**: `crates/mimo-config/src/lib.rs:304-313`
  - **Why it matters**: `set_model` has no callers anywhere in the workspace (verified via workspace-wide grep). It writes only the legacy `model` field and never touches `mode_models`. Under the new resolution rules in `resolve_mode_models` (lines 658-707), when `mode_models` is present in the file it takes precedence and the legacy `model` field is ignored. Because `set_model_for_mode` always writes `mode_models` and clears `model`, any future caller of `set_model` would see its change silently lost on the next reload. Keeping a broken-by-construction public method invites a subtle bug later.
  - **Evidence**: `grep` for `\.set_model\b|set_model\(` across `crates/**/*.rs` returns only the definition at line 304. `resolve_mode_models` prefers `file_mode_models` over `file_legacy_model` via the `if let ... else if` at lines 668-683.
  - **Fix**: Remove `set_model`, or have it delegate to `set_model_for_mode` for a sensible mode (e.g. the active/default mode) so its effect survives reload.

## Suggested Next Steps

- [x] Fix the P2 session-leak in `set_model_for_mode` (read-modify-write the file's own `mode_models`) and add a regression test.
- [x] Remove or repurpose `set_model` (P3).
- [x] Re-run `cargo test --workspace` and `cargo clippy --workspace -- -D warnings` after fixes.
