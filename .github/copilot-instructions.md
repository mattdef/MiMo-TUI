# Copilot Instructions

## Build, test, and verification commands

- `cargo check` — fast compile check for the whole crate
- `cargo fmt --check` — formatting check used as the closest thing to a lint step in this repo
- `cargo test` — run the full test suite
- `cargo test <test_name>` — run a single test or a filtered subset by name
- `cargo run` — launch the TUI
- `cargo run -- doctor` — print the resolved config, model, and API key status
- `cargo run -- ask "your prompt"` — exercise the MiMo API path without opening the TUI

## High-level architecture

- `src/main.rs` is the entrypoint and dispatcher. It parses CLI flags and subcommands, resolves configuration once via `AppConfig::load`, then routes into one-shot `ask`, `doctor`, or the full-screen TUI.
- `src/config.rs` is the source of truth for runtime settings and MiMo defaults. Configuration is merged in this order: CLI flags, environment variables, config file, then built-in defaults.
- `src/client.rs` owns the OpenAI-compatible MiMo HTTP client. It serializes `ChatMessage` values, posts to `{base_url}/chat/completions`, and parses streaming SSE responses line by line until `[DONE]`.
- `src/tui.rs` owns the interactive app state and rendering loop. It enters raw mode and the alternate screen, stores the whole conversation in `App`, and updates the in-progress assistant reply through Tokio channel events emitted by the async API task.

## Key conventions

- Keep MiMo-specific defaults centralized in `src/config.rs` (`DEFAULT_BASE_URL`, `DEFAULT_MODEL`, `DEFAULT_TEMPERATURE`, `DEFAULT_SYSTEM_PROMPT`). Do not duplicate those values in CLI, client, or TUI code.
- Preserve config precedence: CLI overrides environment, environment overrides config file, and the config file overrides built-in defaults. If you add a setting, wire it through `ConfigOverrides`, `FileConfig`, and `AppConfig::load` together.
- Base URLs are normalized with `trim_end_matches('/')` before requests are built. Keep that behavior so `src/client.rs` can safely append `/chat/completions`.
- Both CLI and TUI build requests from the same `ChatMessage` model. Reuse `ChatMessage::system`, `ChatMessage::user`, and `ChatMessage::assistant` instead of constructing ad hoc payloads.
- The TUI streaming flow depends on inserting an empty assistant message before spawning the request, saving its index in `assistant_index`, then mutating that same message as `StreamEvent::Delta` arrives. Preserve that pattern so streamed replies stay in a single conversation entry.
- Recoverable user-facing failures are surfaced in the TUI status line or assistant transcript instead of aborting the whole session. Follow the existing pattern in `submit` and `handle_stream_event` when adding new runtime errors.
- This codebase uses `anyhow::Result` with `Context` to propagate errors across module boundaries. When adding new fallible paths, attach context close to file I/O, HTTP calls, and channel boundaries.

## Project-specific configuration details

- The default config file is `~/.config/mimo-tui/config.toml`.
- `MIMO_TUI_CONFIG` overrides the config file location.
- `MIMO_API_KEY`, `MIMO_BASE_URL`, `MIMO_MODEL`, and `MIMO_TEMPERATURE` are the supported environment variables.
- The repository is currently a single binary crate; there is no separate library crate or workspace split yet.
