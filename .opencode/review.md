# Code Review Summary

**Scope**: Workspace instruction auto-discovery in `mimo-config`, plus CLI/TUI integration checks
**Overall risk**: Medium
**Verdict**: Approve with comments

## Findings

### [P0] Blocking

- None.

### [P1] High

- None.

### [P2] Medium

- **Unreadable instruction files now abort startup instead of being skipped**
  - **Location**: `crates/mimo-config/src/lib.rs:267-289`
  - **Why it matters**: Workspace instructions are optional/additive, but a permission error or transient read failure on `AGENTS.md`, `CLAUDE.md`, or `README.md` now makes `AppConfig::load()` fail. That blocks `doctor`, `ask`, and the TUI from starting in that workspace.
  - **Evidence**: `discover_workspace_instruction()` only skips `NotFound` and Unix symlink `ELOOP` cases; other `symlink_metadata`, `open`, and `read` errors are returned. `AppConfig::load()` propagates that through `resolve_system_prompt(...)` at `crates/mimo-config/src/lib.rs:125-129`, so a regular but unreadable `AGENTS.md` prevents fallback to `CLAUDE.md`, `README.md`, or the base prompt.
  - **Fix**: Treat I/O failures for auto-discovered instruction files as non-fatal discovery misses: skip to the next candidate (or no workspace instructions) and optionally surface a warning separately. Add a regression test with an unreadable higher-priority candidate and a readable fallback.

### [P3] Low

- None.

## Suggested Next Steps

- [ ] Decide whether auto-discovered workspace instructions should ever be allowed to block startup; if not, make candidate I/O failures skippable
- [ ] Add a regression test for unreadable candidates (and optionally a UTF-8-boundary truncation case)
- [ ] Re-run relevant validation after fixes
