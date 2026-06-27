# Plan: Workspace Instruction Auto-Discovery

## Objective

Implement startup-time discovery of workspace instruction files for MiMo-TUI. When `AppConfig::load()` resolves the effective `system_prompt`, it should append the first supported workspace instruction file found at the workspace root, enforce a 32 KiB instruction-content cap, reject symlinked instruction files, and expose the contribution through `ConfigValueSource::WorkspaceInstructions` so `cargo run -- doctor` reports the source.

## Requirements Snapshot

- **R1:** Discover workspace instruction files from the workspace root only, in priority order: `AGENTS.md`, then `CLAUDE.md`, then `README.md` as a weak fallback.
- **R2:** Append discovered instruction content after the existing base prompt: a `system_prompt` from config file when present, otherwise `DEFAULT_SYSTEM_PROMPT`, using a clear separator.
- **R3:** Limit loaded workspace instruction content to 32 KiB and handle truncation safely.
- **R4:** Reject symlinked instruction files consistently with the existing `mimo-tools/src/file.rs` `O_NOFOLLOW` defense-in-depth pattern.
- **R5:** Add `ConfigValueSource::WorkspaceInstructions` and ensure `doctor` displays that source through the existing `system_prompt_source` output.
- **R6:** Preserve existing workspace layout, Edition 2024, root cargo commands, reqwest rustls-only setup, base URL normalization, and current CLI/TUI request flow.

## Scope

- Modify `crates/mimo-config/src/lib.rs` to add discovery constants/helpers, safe file reading, prompt merging, source tracking, and unit tests.
- Modify `crates/mimo-config/Cargo.toml` only if needed for `libc` and test-only `tempfile` dependencies.
- Review `crates/mimo-cli/src/main.rs` doctor output and adjust only if the new source is not displayed automatically.
- Do not change `crates/mimo-tui/src/app.rs` or `ask()` request construction unless compilation reveals a direct need; both already consume `config.system_prompt`.

## Assumptions and Constraints

- No `.opencode/task.md` exists; this plan is based on the user-provided requirements.
- Use `std::env::current_dir()` inside `mimo-config` for the workspace root. Do not add a `mimo-config -> mimo-tools` dependency just to call `default_workspace_root()`.
- Do not introduce a new user-facing workspace-root config setting in this change. If a configured workspace root is added later, it must be threaded through `ConfigOverrides -> AppConfig::load` and follow config precedence.
- Treat workspace instructions as an additive prompt contribution, not a replacement for config-file or built-in prompts.
- Because `system_prompt_source` is a single enum value, set it to `WorkspaceInstructions` when workspace content is appended; otherwise keep the existing `File` or `Default` source.
- For normal symlink candidates, “reject” means do not load the symlink target. Prefer skipping the rejected candidate and continuing to the next lower-priority file; still use `O_NOFOLLOW` on Unix to guard against race-time symlink replacement.

## Risks and Areas Requiring Care

- The repository root already contains `AGENTS.md`; after this change, running `cargo run -- doctor` from the repo root should show `workspace instructions` as the system prompt source.
- Avoid global `current_dir` mutations in tests where possible; test helper functions that accept an explicit root path.
- Do not use `Path::exists()` for discovery because it follows symlinks. Use `symlink_metadata()` to classify candidates.
- Do not read unbounded `README.md` content before truncating; read only up to the configured cap plus enough to detect truncation.
- Ensure byte truncation does not panic or create invalid prompt content for UTF-8 markdown files.
- Adding `libc` to `mimo-config` is acceptable because it is already a workspace dependency; do not alter reqwest features.

## Core Concepts

- **Base prompt:** The existing selected prompt from config file, or `DEFAULT_SYSTEM_PROMPT` when no non-empty config value exists.
- **Workspace instructions:** The first non-symlink, regular, non-empty supported instruction file in the workspace root by priority.
- **Effective prompt:** `base prompt + separator + workspace instruction block` when instructions are found; otherwise exactly the previous base prompt.
- **Source reporting:** `system_prompt_source` remains `File` or `Default` when no instruction file contributes. It becomes `WorkspaceInstructions` when an instruction block is appended.

## Sub-Tasks

### Sub-Task 1: Extend config source metadata and dependencies

- **Status:** Pending
- **Objective:** Add the new source variant and any crate dependencies required for no-follow file opening and tests.
- **Related Requirements:** R4, R5, R6
- **Dependencies and Preconditions:** None.
- **In Scope for This Sub-Task:**
  - `crates/mimo-config/src/lib.rs`
  - `crates/mimo-config/Cargo.toml`
- **Out of Scope for This Sub-Task:**
  - Prompt discovery logic.
  - CLI flags or config-file schema changes.
- **Instructions:**
  1. Add `WorkspaceInstructions` to `ConfigValueSource` in `crates/mimo-config/src/lib.rs`.
  2. Update the `fmt::Display` implementation so the variant prints a concise human-readable label such as `workspace instructions`.
  3. Add `libc.workspace = true` to `crates/mimo-config/Cargo.toml` if the Unix `O_NOFOLLOW` helper uses `libc::O_NOFOLLOW` directly.
  4. Add `[dev-dependencies] tempfile.workspace = true` to `crates/mimo-config/Cargo.toml` if tests use temporary directories.
- **Acceptance Criteria:**
  - `ConfigValueSource` exhaustive matches compile.
  - `ConfigValueSource::WorkspaceInstructions.to_string()` can be asserted in tests.
  - No workspace edition or reqwest dependency settings are changed.
- **Cautionary Points (Risks & Edge Cases):**
  - Keep the existing display strings for `Cli`, `Env`, `File`, and `Default` unchanged.
  - Do not add new environment variables or config TOML keys.
- **Implementation Suggestions:**
  - Place the new variant alongside the existing enum variants and update the match immediately to keep compiler errors obvious.
- **Testing Suggestions:**
  - Add or extend a small unit test in `mimo-config` for the display label.
  - Run `cargo check` after this sub-task if implementing incrementally.
- **Done When:**
  - The config crate can represent and display the workspace instruction source.

### Sub-Task 2: Add safe workspace instruction discovery helpers

- **Status:** Pending
- **Objective:** Implement root-only discovery, priority selection, symlink rejection, and 32 KiB bounded reading in `mimo-config`.
- **Related Requirements:** R1, R3, R4, R6
- **Dependencies and Preconditions:** Sub-Task 1 completed if `libc` is needed.
- **In Scope for This Sub-Task:**
  - New private constants in `crates/mimo-config/src/lib.rs`:
    - `WORKSPACE_INSTRUCTIONS_MAX_BYTES` set to `32 * 1024`.
    - A filename priority list for `AGENTS.md`, `CLAUDE.md`, `README.md`.
  - New private helper data structure for discovered instruction metadata, if useful.
  - New private helper functions for discovery, candidate classification, safe open/read, and truncation.
- **Out of Scope for This Sub-Task:**
  - Parent-directory walking.
  - Recursive workspace scanning.
  - Parsing markdown semantics.
- **Instructions:**
  1. Add a helper that accepts a workspace root path and checks only `root/AGENTS.md`, `root/CLAUDE.md`, and `root/README.md` in that order.
  2. Use `fs::symlink_metadata()` for each candidate:
     - missing file: continue to the next candidate;
     - symlink: reject it and continue to the next candidate;
     - directory or other non-regular file: ignore it and continue;
     - regular file: attempt to read it safely.
  3. For Unix builds, open the regular candidate using `OpenOptions` plus `O_NOFOLLOW`, matching the defense-in-depth pattern in `crates/mimo-tools/src/file.rs:312`.
  4. For non-Unix builds, still rely on `symlink_metadata()` to reject symlink candidates before opening.
  5. Read only up to `WORKSPACE_INSTRUCTIONS_MAX_BYTES + 1` bytes, so truncation can be detected without loading an entire large README.
  6. Convert the bounded bytes into prompt text safely. Prefer UTF-8-preserving truncation; if using lossy conversion, ensure tests cover the ASCII truncation contract.
  7. Treat empty or whitespace-only instruction content as no discovered instruction and continue to the next candidate.
  8. Return discovered metadata including at least filename/path, content, and whether truncation occurred.
- **Acceptance Criteria:**
  - Discovery chooses only one file: the first valid candidate by priority.
  - Symlinked candidates are never followed.
  - Loaded instruction content is capped at 32 KiB before prompt assembly.
  - Helper functions are testable without changing the process current directory.
- **Cautionary Points (Risks & Edge Cases):**
  - `Path::exists()` and `fs::metadata()` follow symlinks; avoid them for candidate classification.
  - If a file is replaced by a symlink between metadata and open on Unix, `O_NOFOLLOW` should cause open to fail instead of following it.
  - Be deliberate about unreadable regular files: include path context in any propagated error so startup failures are diagnosable.
- **Implementation Suggestions:**
  - Keep helpers private to `mimo-config` unless tests need `pub(crate)` visibility.
  - Include the source filename in the returned metadata so the prompt separator can say which file was used.
- **Testing Suggestions:**
  - Unit test priority: create all three files in a tempdir and assert `AGENTS.md` is selected.
  - Unit test fallback: create only `CLAUDE.md`, then only `README.md`, and assert each can be selected.
  - Unit test truncation with ASCII content larger than 32 KiB.
  - Unix-only unit test symlink rejection using a symlinked `AGENTS.md`; assert the symlink target content is not loaded and a lower-priority regular file can be used.
- **Done When:**
  - Discovery is bounded, root-only, priority-aware, and symlink-safe.

### Sub-Task 3: Wire discovery into `AppConfig::load()` prompt resolution

- **Status:** Pending
- **Objective:** Merge discovered workspace instructions into the resolved system prompt while preserving existing config precedence for the base prompt.
- **Related Requirements:** R1, R2, R5, R6
- **Dependencies and Preconditions:** Sub-Task 2 completed.
- **In Scope for This Sub-Task:**
  - `crates/mimo-config/src/lib.rs`, specifically `AppConfig::load()` and nearby prompt helper functions.
- **Out of Scope for This Sub-Task:**
  - Changes to `mimo-client`, `mimo-agent`, or TUI message construction.
  - New CLI options for system prompts.
- **Instructions:**
  1. Keep the existing file/default base prompt behavior:
     - non-empty `file_config.system_prompt` remains the base prompt;
     - otherwise use `DEFAULT_SYSTEM_PROMPT`.
  2. Resolve the workspace root with `std::env::current_dir()` inside `AppConfig::load()`; fall back to `.` if matching the existing `default_workspace_root()` behavior is preferred over startup failure.
  3. Call the discovery helper after the base prompt is selected.
  4. If no instruction content is discovered, leave both `system_prompt` and `system_prompt_source` exactly as before.
  5. If instruction content is discovered, append it to the base prompt using a clear separator that includes the source filename, for example a heading-style block naming `AGENTS.md`, `CLAUDE.md`, or `README.md`.
  6. If the content was truncated, include a concise truncation note in the workspace instruction block.
  7. Set `system_prompt_source` to `ConfigValueSource::WorkspaceInstructions` when an instruction block is appended.
- **Acceptance Criteria:**
  - With no workspace instruction files, effective prompt output and source are unchanged.
  - With a workspace `AGENTS.md`, effective prompt contains the base prompt first, then a separator, then AGENTS content.
  - With a config-file `system_prompt` and workspace instructions, the config prompt remains first and workspace instructions are appended after it.
  - `base_url.trim_end_matches('/')` behavior remains untouched.
- **Cautionary Points (Risks & Edge Cases):**
  - Do not accidentally trim or rewrite the built-in default prompt.
  - Do not allow `README.md` to override `AGENTS.md` or `CLAUDE.md`.
  - Avoid duplicate separators when instruction content is empty.
- **Implementation Suggestions:**
  - Keep prompt assembly in a small helper such as `append_workspace_instructions(...)` so tests can assert the separator and ordering directly.
- **Testing Suggestions:**
  - Unit test prompt assembly with default base prompt.
  - Unit test prompt assembly with a custom config-style base prompt.
  - Unit test no-discovery path keeps the previous source.
- **Done When:**
  - `AppConfig::load()` produces the correct effective prompt and source for discovered and non-discovered cases.

### Sub-Task 4: Ensure `doctor` reports the new source

- **Status:** Pending
- **Objective:** Confirm the CLI `doctor` output displays `WorkspaceInstructions` through `system_prompt_source`.
- **Related Requirements:** R5, R6
- **Dependencies and Preconditions:** Sub-Task 1 and Sub-Task 3 completed.
- **In Scope for This Sub-Task:**
  - `crates/mimo-cli/src/main.rs:115-147`, especially the `System prompt` line.
- **Out of Scope for This Sub-Task:**
  - Printing full prompt contents.
  - Adding a new doctor section for full instruction-file paths unless needed for debugging.
- **Instructions:**
  1. Inspect the existing `doctor()` implementation. It already prints `config.system_prompt_source` via `Display`.
  2. If that remains true after adding the enum variant, no code change is required in `mimo-cli` beyond any formatting cleanup requested by `cargo fmt`.
  3. If the implementation changes during development, ensure the output still includes the source beside the system prompt preview.
- **Acceptance Criteria:**
  - From a directory containing a valid `AGENTS.md`, `cargo run -- doctor` shows the system prompt source as `workspace instructions`.
  - From a directory without instruction files, `doctor` still shows `default` or `config file` as before.
- **Cautionary Points (Risks & Edge Cases):**
  - The doctor preview prints only the first line of the effective prompt; this is acceptable and should not be expanded in this task.
  - Do not change `ask()` request construction; it already sends `config.system_prompt` as the system message.
- **Implementation Suggestions:**
  - Prefer relying on `ConfigValueSource` `Display` over adding CLI-specific source string logic.
- **Testing Suggestions:**
  - Manual check: run `cargo run -- doctor` from the repo root; because this repo has `AGENTS.md`, it should report `workspace instructions`.
- **Done When:**
  - Doctor output reflects the new source without broader CLI behavior changes.

### Sub-Task 5: Add tests and run workspace validation

- **Status:** Pending
- **Objective:** Cover discovery behavior, prompt assembly, source reporting, truncation, and symlink rejection without destabilizing the workspace.
- **Related Requirements:** R1, R2, R3, R4, R5, R6
- **Dependencies and Preconditions:** Sub-Tasks 1-4 completed.
- **In Scope for This Sub-Task:**
  - Unit tests in `crates/mimo-config/src/lib.rs`.
  - Existing root cargo validation commands.
- **Out of Scope for This Sub-Task:**
  - End-to-end tests that require a live MiMo API key.
  - Snapshot tests for full doctor output unless such infrastructure already exists.
- **Instructions:**
  1. Add unit tests for discovery priority and fallback behavior.
  2. Add unit tests for no-file behavior returning no instructions.
  3. Add unit tests for prompt assembly order and separator presence.
  4. Add a unit test for `ConfigValueSource::WorkspaceInstructions` display text.
  5. Add a truncation test proving only 32 KiB of instruction content is included before any separator/metadata overhead.
  6. Add a Unix-only symlink rejection test. If `AGENTS.md` is a symlink and `CLAUDE.md` is a regular file, assert the symlink target content is not used and the regular fallback can be selected.
  7. Avoid tests that mutate the process current directory; test helper functions with explicit temporary roots instead.
- **Acceptance Criteria:**
  - `cargo test -p mimo-config` passes.
  - `cargo test --workspace` passes.
  - `cargo fmt --check` passes.
  - `cargo clippy --workspace -- -D warnings` passes.
- **Cautionary Points (Risks & Edge Cases):**
  - Tempdir-based tests should not depend on the real repository `AGENTS.md`.
  - Symlink tests should be gated with `#[cfg(unix)]` unless a reliable Windows test path is added.
  - If lossy UTF-8 conversion is used, keep truncation assertions byte-oriented for ASCII fixtures to avoid ambiguous character counts.
- **Implementation Suggestions:**
  - Reuse the existing test module in `mimo-config`; expand its `use super::{...}` list as needed.
  - Keep helper functions small enough that tests can exercise behavior directly without constructing a full `AppConfig`.
- **Testing Suggestions:**
  - Run, in order:
    1. `cargo test -p mimo-config`
    2. `cargo fmt --check`
    3. `cargo check`
    4. `cargo clippy --workspace -- -D warnings`
    5. `cargo test --workspace`
  - Manual smoke test: `cargo run -- doctor` from `/mnt/Data/Dev/rust/MiMo-TUI` should show `System prompt` with source `workspace instructions`.
- **Done When:**
  - Automated tests and manual doctor smoke test validate the requested behavior.

## Final Integration & Verification

- **System-Wide Test:**
  1. Create or use a workspace containing `AGENTS.md`; run `cargo run -- doctor` and verify `System prompt` reports `workspace instructions`.
  2. Temporarily test a workspace with only `CLAUDE.md`, then only `README.md`, and verify fallback behavior.
  3. Temporarily test a workspace with no supported files and verify the source remains `default` or `config file`.
  4. Confirm `cargo run -- ask "test prompt"` still builds a request successfully when credentials are configured; no live API assertion is required without an API key.
- **Completion Checklist:**
  - [ ] `ConfigValueSource::WorkspaceInstructions` exists and displays clearly.
  - [ ] Discovery searches only the workspace root and uses the required priority order.
  - [ ] Symlinked candidates are rejected and not followed.
  - [ ] Instruction content is capped at 32 KiB.
  - [ ] Effective prompt appends instructions after the base prompt with a clear separator.
  - [ ] Doctor displays the new source when workspace instructions are appended.
  - [ ] No changes violate Edition 2024, workspace layout, reqwest rustls-only, or base URL normalization constraints.
  - [ ] `cargo fmt --check`, `cargo check`, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace` pass.

## Open Questions

- None blocking. If maintainers prefer symlink candidates to fail startup instead of being skipped, update Sub-Task 2 tests and behavior consistently before implementation.
