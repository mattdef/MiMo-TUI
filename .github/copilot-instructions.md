# Copilot Instructions

## Build, test, and verification commands

- `cargo check` — fast compile check for the whole crate
- `cargo fmt --check` — formatting verification for the whole workspace
- `cargo clippy --workspace -- -D warnings` — lint step enforced by CI
- `cargo test --workspace` — run the full workspace test suite
- `cargo test <test_name>` — run a single test or a filtered subset by name
- `cargo run` — launch the TUI
- `cargo run -- doctor` — print the resolved config, permission policy, model, and API key status
- `cargo run -- ask "your prompt"` — exercise the MiMo API path without opening the TUI

## High-level architecture

- `crates/mimo-cli/src/main.rs` is the entrypoint and dispatcher. It parses CLI flags and subcommands, resolves configuration once via `AppConfig::load`, then routes into one-shot `ask`, `doctor`, or the full-screen TUI.
- `crates/mimo-config/src/lib.rs` is the source of truth for runtime settings and MiMo defaults. Configuration is merged in this order: CLI flags, environment variables, config file, then built-in defaults.
- `crates/mimo-protocol/src/lib.rs` holds shared transport types such as `ChatMessage`, `Role`, and tool schema metadata.
- `crates/mimo-client/src/lib.rs` owns the OpenAI-compatible MiMo HTTP client. It posts to `{base_url}/chat/completions`, parses streaming SSE responses line by line until `[DONE]`, and keeps model auto-routing out of the UI layer.
- `crates/mimo-agent/src/lib.rs` owns the tool-call execution loop around `MimoClient`.
- `crates/mimo-state/src/*.rs` owns persisted sessions, tasks, memory, skills, MCP definitions, and diagnostics snapshots.
- `crates/mimo-tui-core/src/*.rs` owns slash commands, keybinding metadata, the input buffer, and markdown helpers.
- `crates/mimo-tui/src/lib.rs` and `crates/mimo-tui/src/app.rs` own the interactive rendering loop and app state. The TUI enters raw mode and the alternate screen, stores the whole conversation in `App`, and updates the in-progress assistant reply through Tokio channel events emitted by the async API task.

## Key conventions

- Keep MiMo-specific defaults centralized in `crates/mimo-config/src/lib.rs` (`DEFAULT_BASE_URL`, `DEFAULT_MODEL`, `DEFAULT_TEMPERATURE`, `DEFAULT_SYSTEM_PROMPT`). Do not duplicate those values in CLI, client, or TUI code.
- Preserve config precedence: CLI overrides environment, environment overrides config file, and the config file overrides built-in defaults. If you add a setting, wire it through `ConfigOverrides`, `FileConfig`, and `AppConfig::load` together.
- Base URLs are normalized with `trim_end_matches('/')` before requests are built. Keep that behavior so `crates/mimo-client/src/lib.rs` can safely append `/chat/completions`.
- Both CLI and TUI build requests from the same `ChatMessage` model. Reuse `ChatMessage::system`, `ChatMessage::user`, and `ChatMessage::assistant` instead of constructing ad hoc payloads.
- The TUI streaming flow depends on inserting an empty assistant message before spawning the request, saving its index in `assistant_index`, then mutating that same message as `StreamEvent::Delta` arrives. Preserve that pattern so streamed replies stay in a single conversation entry.
- Recoverable user-facing failures are surfaced in the TUI status line or assistant transcript instead of aborting the whole session. Follow the existing pattern in `submit` and `handle_stream_event` when adding new runtime errors.
- This codebase uses `anyhow::Result` with `Context` to propagate errors across module boundaries. When adding new fallible paths, attach context close to file I/O, HTTP calls, and channel boundaries.

## Project-specific configuration details

- The default config file is `~/.config/mimo-tui/config.toml`.
- `MIMO_TUI_CONFIG` overrides the config file location.
- `MIMO_API_KEY`, `MIMO_BASE_URL`, `MIMO_MODEL`, and `MIMO_TEMPERATURE` are the supported environment variables. Tool permissions are configured only in `config.toml`.
- The repository is now a Cargo workspace with separate crates for CLI, config, protocol, client, tools, state, agent, TUI-core, and TUI runtime.
