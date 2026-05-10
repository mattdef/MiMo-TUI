# MiMo TUI

MiMo TUI is a Rust terminal client specialised for Xiaomi MiMo models. It uses an OpenAI-compatible chat completions API, streams responses into a keyboard-driven interface, and keeps the defaults centred on MiMo model IDs.

## Features

- Ratatui/crossterm terminal interface with streaming assistant responses.
- `ask` command for one-shot prompts outside the TUI.
- Searchable help overlay, slash-command registry, multiline input editor, and bounded transcript scrolling.
- Local session save/load/export, interactive `/config`, `/status`, `/models`, `/retry`, `/mode`, and `/context` commands.
- Configurable MiMo API key, base URL, model, temperature, and system prompt.
- Defaults for `https://api.xiaomimimo.com/v1` and `mimo-v2-flash`, with bundled model suggestions for `mimo-v2-flash`, `mimo-v2.5`, and `mimo-v2.5-pro`.
- `/models` tries to refresh the picker from the MiMo `/models` API and falls back to the bundled suggestions when the catalog cannot be loaded.

## Quickstart

```bash
export MIMO_API_KEY="your-api-key"
cargo run
```

One-shot prompt:

```bash
cargo run -- ask "Explain what makes MiMo models useful for coding workflows"
```

Check configuration:

```bash
cargo run -- doctor
```

## Configuration

Environment variables take precedence over `~/.config/mimo-tui/config.toml`. CLI flags take precedence over both.

```toml
api_key = "your-api-key"
base_url = "https://api.xiaomimimo.com/v1"
model = "mimo-v2-flash"
temperature = 0.2
system_prompt = "You are MiMo TUI, a concise terminal assistant."
```

Supported environment variables:

- `MIMO_API_KEY`
- `MIMO_BASE_URL`
- `MIMO_MODEL`
- `MIMO_TEMPERATURE`
- `MIMO_TUI_CONFIG`

## TUI controls

- `Enter`: send prompt
- `Shift+Enter` / `Ctrl+J`: insert a newline
- `F1` or `?` on an empty draft: open help
- `/help`: open command help
- `/clear`: clear conversation history
- `/exit` / `/quit` / `/q`: quit
- `/model [id]`: show or switch the current MiMo model
- `/models`: open the MiMo model picker and refresh it from the API when available
- `/save` / `/load` / `/sessions` / `/export`: manage local sessions
- `/config`: inspect or update local config
- `/status`: show runtime status
- `/retry`: resend the last prompt
- `/mode [chat|plan]`: switch between normal and planning behavior
- `/context`: inject a read-only workspace summary into the conversation
- `Ctrl+U`: clear prompt draft
- `Left` / `Right`: move the draft cursor
- `Home` / `End` or `Ctrl+A` / `Ctrl+E`: jump within the current line
- `Delete` / `Backspace`: delete around the cursor
- `Up` / `Down`: scroll conversation
- `PageUp` / `PageDown`: scroll by page
- `Ctrl+Home` / `Ctrl+End`: jump to transcript start/end
- `Esc`: close help or cancel an in-progress generation
- `Ctrl+C`: quit

## Configuration from inside the TUI

Examples:

```text
/config show
/config api-key YOUR_KEY
/config base-url https://api.xiaomimimo.com/v1
/config model mimo-v2-flash
/config temperature 0.2
```

The API key is written to `~/.config/mimo-tui/config.toml` and is never echoed back in the UI.

## Session files

By default, session JSON files and Markdown exports are stored under:

```text
~/.config/mimo-tui/sessions/
```

`/save` creates a new JSON snapshot, `/load` restores the most recent saved session when no path is provided, and `/export` writes a Markdown transcript.

## Architecture snapshot

- `src/main.rs`: CLI entrypoint and `ask` / `doctor` dispatcher
- `src/config.rs`: config loading, validation, and persistence helpers
- `src/client.rs`: OpenAI-compatible MiMo streaming client
- `src/tui.rs`: ratatui application shell, help overlay, commands, editor integration
- `src/tui/commands.rs`: slash-command registry and parsing
- `src/tui/input.rs`: multiline draft editor
- `src/tui/session_store.rs`: local session persistence/export
- `src/tui/project_context.rs`: read-only workspace summarization for planning mode
