# Plan: Remove the legacy auto-approval mode and move tool permissions to config

## Objective

Remove the runtime auto-approval execution mode from MiMo-TUI and make tool permission behavior come only from `config.toml` plus a safe built-in default. Keep the agent/planning workflow modes, but stop using mode switches, keybindings, palette entries, or approval-prompt shortcuts to change permission behavior.

## Requirements Snapshot

- **R1:** Find and remove all product references to the legacy auto-approval mode.
- **R2:** Preserve only supported execution modes after removal: `agent` and `plan`.
- **R3:** Tool permissions must be configured through the config file, not through runtime mode changes, CLI flags, environment variables, palette actions, or approval-prompt shortcuts.
- **R4:** Keep safe defaults: mutating tools should not become auto-approved unless the config file explicitly asks for that behavior.
- **R5:** Update tests and docs to match the new mode and permission model.

## Scope

- In scope: config loading, CLI/TUI approval decisions, TUI mode UI, slash-command parsing, session/task mode serialization, tests, and README/docs.
- Out of scope: changing individual tool implementations, HTTP client behavior, protocol types, or adding per-tool permission rules beyond the existing global read-only/prompt/auto behavior.

## Assumptions and Constraints

- `.opencode/task.md` is absent; this plan is based on the user request and code exploration.
- Current permission behavior is not config-driven: `AppMode::Plan` maps to `ApprovalMode::ReadOnly` and `AppMode::Agent` maps to `ApprovalMode::Prompt` in `crates/mimo-tui/src/app.rs`; the removed legacy auto-approval mode mapped to `ApprovalMode::Auto`.
- Current per-tool risk classification stays in `crates/mimo-tools`: `ApprovalRequirement::Auto` for read/search/git/project-style tools and `ApprovalRequirement::Prompt` for mutating/network/shell tools.
- Default permission policy should be `prompt` for interactive TUI use; non-interactive/background flows cannot prompt and should deny prompt-required tools unless the config policy is `auto`.
- Because repo instructions say new settings normally thread through `ConfigOverrides`, add an internal override field if needed for consistency/tests, but do not expose permissions through CLI args or environment variables.

## Risks and Areas Requiring Care

- Removing the legacy auto-approval mode can break loading older saved sessions/tasks whose serialized `mode` was the removed value. Prefer a compatibility strategy that maps unknown legacy mode strings to `Agent` without re-emitting the removed mode.
- Decoupling plan mode from permission enforcement changes semantics: plan mode should remain a planning prompt/UI mode, while actual tool permission decisions come from config.
- Background tasks currently auto-approve mutating tools in `Agent`; after this change they must use the config policy instead.
- Avoid leaving UI affordances that imply permissions can be changed at runtime.

## Current Findings

### Product references found by case-insensitive search

- `README.md`: overview, mode docs, warning, keybinding table, `/mode` table.
- `crates/mimo-tui-core/src/keybindings.rs`: F2/Ctrl+Tab description.
- `crates/mimo-tui-core/src/commands.rs`: removed mode name, `/mode` usage, parser branch, parser tests.
- `crates/mimo-state/src/models.rs`: removed mode, display string.
- `crates/mimo-state/src/task_store.rs`: test fixture using the removed mode.
- `crates/mimo-tui/src/command_palette.rs`: auto-approval mode entry.
- `crates/mimo-tui/src/app.rs`: `ModeName` mapping, plan hints/system prompt, footer warning, approval overlay text, background task permission logic, approval-key shortcut, `mode_approval_mode`, `next_mode`, and tests.

### Current permission flow

- Tools declare `ApprovalRequirement` in `crates/mimo-tools/src/spec.rs` and overrides in `file.rs`, `shell.rs`, and `extra.rs`.
- `mimo-agent::run_agent_turn` asks its caller whether to approve prompt-required tools.
- `mimo-cli ask` auto-approves only `ApprovalRequirement::Auto` and denies `Prompt`.
- Foreground TUI uses `ApprovalMode` from current app mode: auto approves, denies read-only, or prompts.
- Background tasks currently approve prompt-required tools whenever mode is `Agent` or the removed auto mode.
- `crates/mimo-config/src/lib.rs` currently has no tool permission setting.

## Core Concepts

- **Execution mode:** user workflow mode (`agent` or `plan`) that affects UI/system-prompt behavior.
- **Permission policy:** config-file setting controlling tool approval behavior. Suggested TOML shape:

  ```toml
  permissions = "prompt"  # read_only | prompt | auto
  ```

- **Policy semantics:**
  - `read_only`: allow `ApprovalRequirement::Auto`; deny `ApprovalRequirement::Prompt`.
  - `prompt`: allow `Auto`; prompt in foreground TUI for `Prompt`; deny `Prompt` in non-interactive/background contexts because no prompt is available.
  - `auto`: allow both `Auto` and `Prompt`; dangerous and must be explicit in `config.toml`.

## Sub-Tasks

### Sub-Task 1: Add file-backed permission policy to config

- **Status:** Pending
- **Objective:** Make permissions load from `config.toml` with a safe default and no user-facing CLI/env overrides.
- **Related Requirements:** R3, R4
- **Dependencies and Preconditions:** Existing config precedence and file-loading behavior in `crates/mimo-config/src/lib.rs`.
- **In Scope for This Sub-Task:**
  - Add a public config type such as `PermissionMode { ReadOnly, Prompt, Auto }` with serde support.
  - Add `permissions: PermissionMode` and `permissions_source: ConfigValueSource` to `AppConfig`.
  - Add `permissions: Option<PermissionMode>` to `FileConfig`.
  - Default to `Prompt` when unset.
  - Add an internal `ConfigOverrides` field only if needed by project convention/tests; do not wire it to Clap or environment variables.
  - Add config tests for default, valid file values, and invalid values.
- **Out of Scope for This Sub-Task:** Per-tool or per-command permission rules.
- **Instructions:**
  1. Keep the TOML key simple: `permissions = "prompt"`.
  2. Accept `read_only`, `prompt`, and `auto`; do not accept the removed mode name as a config value.
  3. Track source as `File` or `Default` for normal user flows.
  4. Update all test `AppConfig { ... }` literals across crates with the new fields.
- **Acceptance Criteria:** Config loading exposes a permission policy, default is safe, and no CLI/env permission surface exists.
- **Testing Suggestions:** `cargo test -p mimo-config` plus compile-driven updates to all `AppConfig` literals.

### Sub-Task 2: Remove the removed mode from shared mode state and slash commands

- **Status:** Pending
- **Objective:** Leave only `Plan` and `Agent` as execution modes.
- **Related Requirements:** R1, R2, R5
- **Dependencies and Preconditions:** Sub-Task 1 can be done before or after this task.
- **In Scope for This Sub-Task:**
  - `crates/mimo-state/src/models.rs`: remove the legacy auto-approval mode and its `Display` branch.
  - Add a legacy-safe deserialization strategy if desired: unknown/removed serialized mode values should load as `Agent` while serialization only emits `plan` or `agent`.
  - `crates/mimo-tui-core/src/commands.rs`: remove the legacy auto-approval mode name, update `/mode [agent|plan]`, remove parser support for the removed value and numeric `3`, update tests.
  - `crates/mimo-state/src/task_store.rs`: replace the removed-mode test fixture with `Agent` or a compatibility-focused fixture that does not reintroduce product support.
- **Out of Scope for This Sub-Task:** TUI rendering and runtime permission decisions.
- **Instructions:**
  1. Keep `#[serde(alias = "chat")]` for `Agent` if still needed.
  2. Decide compatibility explicitly: either map unknown legacy modes to `Agent`, or document that old sessions/tasks with removed modes are no longer loadable.
  3. Ensure `/mode 1` and `/mode 2` behavior remains if numeric aliases are retained; `/mode 3` should be invalid.
- **Acceptance Criteria:** Shared state and command parsing no longer expose the removed mode.
- **Testing Suggestions:** `cargo test -p mimo-state`, `cargo test -p mimo-tui-core`.

### Sub-Task 3: Decouple TUI permissions from execution modes

- **Status:** Pending
- **Objective:** Make foreground and background TUI tool approval decisions use config policy only.
- **Related Requirements:** R2, R3, R4
- **Dependencies and Preconditions:** Sub-Tasks 1 and 2.
- **In Scope for This Sub-Task:**
  - `crates/mimo-tui/src/tooling.rs`: replace or repurpose `ApprovalMode` as config-backed permission state; prefer using the config enum to avoid duplicate concepts.
  - `crates/mimo-tui/src/app.rs`: initialize tool runtime permission state from `self.config.permissions`, not `self.mode`.
  - Remove `mode_approval_mode` and stop `set_mode` from changing permissions.
  - Change foreground `run_agent_turn` approval closure to use `config.permissions`.
  - Change background task approval closure to use `config.permissions`; in `prompt` mode, deny prompt-required tools because no interactive prompt is available.
  - Update `config_summary`, `status_summary`, header text, and tool runtime summaries to show the configured permission policy and source.
- **Out of Scope for This Sub-Task:** Adding live config reload while the TUI is running.
- **Instructions:**
  1. Add a small helper for approval decisions so foreground, background, and tests share semantics.
  2. Foreground TUI behavior:
     - `read_only`: auto-required tools run; prompt-required tools are denied.
     - `prompt`: prompt-required tools open the existing approval overlay.
     - `auto`: prompt-required tools run without overlay.
  3. Background behavior:
     - `auto`: run prompt-required tools.
     - `prompt`/`read_only`: deny prompt-required tools and log/status that config does not permit unattended execution.
  4. `set_mode(AppMode::Plan|Agent)` should only update `self.mode` and status.
- **Acceptance Criteria:** Changing `/mode`, F2, Ctrl+Tab, or palette mode entries never changes approval behavior.
- **Testing Suggestions:** Add TUI unit tests proving mode toggles do not change permission policy and background approval follows config.

### Sub-Task 4: Remove TUI UI affordances for runtime permission switching

- **Status:** Pending
- **Objective:** Remove user-visible runtime entry points for the removed mode and for changing permissions outside config.
- **Related Requirements:** R1, R2, R3, R5
- **Dependencies and Preconditions:** Sub-Tasks 2 and 3.
- **In Scope for This Sub-Task:**
  - `crates/mimo-tui/src/app.rs`:
    - Remove mapping from `ModeName` to removed mode.
    - Remove footer safety warning branch.
    - Update plan panel hint and planning system prompt so they do not mention switching to the removed mode or claim mode enforces permissions.
    - Update approval overlay copy to only offer approve/deny and mention config-file permissions if useful.
    - Remove approval key branch that approves and switches to the removed mode.
    - Remove approval key branches that change approval behavior through `r`/`p`; keep only approve/deny unless there is a non-permission mode-change reason.
    - Change `next_mode` to toggle `Agent <-> Plan`.
  - `crates/mimo-tui/src/command_palette.rs`: remove the auto-approval mode entry and update agent/plan hints to avoid implying permissions are controlled by modes.
  - `crates/mimo-tui-core/src/keybindings.rs`: update F2/Ctrl+Tab and approval-prompt descriptions.
- **Out of Scope for This Sub-Task:** Permission config parsing already handled in Sub-Task 1.
- **Instructions:** Ensure every product string that mentioned the removed mode is rewritten or removed.
- **Acceptance Criteria:** UI offers only `agent` and `plan` modes, and the approval prompt cannot alter permission policy.
- **Testing Suggestions:** Update/remove affected TUI tests: safety-warning test, plan-panel test, Ctrl+Tab/F2 cycling tests, approval prompt tests if present.

### Sub-Task 5: Update CLI behavior and diagnostics for config-backed permissions

- **Status:** Pending
- **Objective:** Make non-interactive CLI tool approval use the same config policy.
- **Related Requirements:** R3, R4, R5
- **Dependencies and Preconditions:** Sub-Task 1.
- **In Scope for This Sub-Task:**
  - `crates/mimo-cli/src/main.rs`: pass config permission policy into `ask` approval decisions.
  - Update `should_auto_approve_non_interactive_cli` to account for policy.
  - Update warnings for denied tools to explain that non-interactive prompt-required tools need `permissions = "auto"` in `config.toml` to run unattended.
  - Update `doctor` output to print the resolved permissions policy and source.
- **Out of Scope for This Sub-Task:** Adding `--permissions` or environment variables.
- **Acceptance Criteria:** CLI behavior is config-driven and no new runtime permission flag exists.
- **Testing Suggestions:** Update `mimo-cli` unit tests for `read_only`, `prompt`, and `auto` policies.

### Sub-Task 6: Update docs and canonical project instructions

- **Status:** Pending
- **Objective:** Documentation matches the new mode and permission model.
- **Related Requirements:** R1, R3, R5
- **Dependencies and Preconditions:** Sub-Tasks 1-5 define final behavior.
- **In Scope for This Sub-Task:**
  - `README.md`: remove removed-mode overview/warning, update config example with `permissions = "prompt"`, document `read_only|prompt|auto`, update execution modes, controls, and `/mode [agent|plan]`.
  - `.github/copilot-instructions.md`: add the config-file-only permission setting to the configuration details if maintainers want canonical docs updated.
- **Out of Scope for This Sub-Task:** Changelog/release notes unless the project already has them.
- **Acceptance Criteria:** User docs no longer describe runtime auto-approval mode and clearly state permissions are file-configured.
- **Testing Suggestions:** Run a case-insensitive product grep after docs update.

### Sub-Task 7: Final cleanup and regression tests

- **Status:** Pending
- **Objective:** Verify the removed mode is gone from product code/docs and permissions are config-only.
- **Related Requirements:** R1-R5
- **Dependencies and Preconditions:** Sub-Tasks 1-6.
- **In Scope for This Sub-Task:**
  - Remove obsolete tests and add targeted replacements.
  - Verify no product references remain.
  - Run workspace validation.
- **Out of Scope for This Sub-Task:** Broad refactors unrelated to modes/permissions.
- **Instructions:**
  1. Run a case-insensitive search for the removed mode in product files; expected result should be none except temporary planning notes or explicit migration comments if the team accepts them.
  2. Run targeted tests first, then workspace checks.
  3. Manually smoke-test `/mode`, F2/Ctrl+Tab, approval overlay, `/config show`, `doctor`, and `ask` under each config permission value.
- **Acceptance Criteria:** Tests pass and no runtime UI/CLI path can switch permissions outside config.
- **Testing Suggestions:**
  - `cargo fmt --check`
  - `cargo check`
  - `cargo clippy --workspace -- -D warnings`
  - `cargo test --workspace`

## Final Integration & Verification

- **System-Wide Test:**
  1. With no `permissions` key, launch TUI and confirm header/status shows `prompt` permissions.
  2. Confirm `/mode` shows/switches only `agent` and `plan`.
  3. Confirm F2/Ctrl+Tab toggles only between `agent` and `plan` and does not alter permissions.
  4. Trigger a mutating tool in foreground TUI with `permissions = "prompt"`; approval overlay should offer only approve/deny.
  5. Set `permissions = "read_only"`; mutating tools should be denied.
  6. Set `permissions = "auto"`; mutating tools should run without approval.
  7. Run `cargo run -- doctor` and confirm permissions/source are shown.
  8. Run `cargo run -- ask "..."` with each permission value and confirm prompt-required tools are denied unless policy is `auto`.
- **Completion Checklist:**
  - [ ] Product references to the removed mode are gone.
  - [ ] `AppMode` has only `Plan` and `Agent`.
  - [ ] `/mode` accepts only `plan`/`agent` plus retained numeric aliases if any.
  - [ ] Permission policy loads from `config.toml` with safe default.
  - [ ] No CLI arg, env var, keybinding, palette action, or approval-prompt shortcut changes permissions.
  - [ ] Foreground TUI, background tasks, and CLI ask all use config-backed permission decisions.
  - [ ] README and tests are updated.

## Open Questions

- Should legacy saved sessions/tasks with the removed serialized mode be accepted and normalized to `agent`, or is strict failure acceptable? Recommendation: normalize to `agent` to avoid breaking user state, while never serializing or exposing the removed mode again.
