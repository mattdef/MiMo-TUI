# MiMo TUI

MiMo TUI is a Rust terminal client specialised for Xiaomi MiMo models. It uses an OpenAI-compatible chat completions API, streams responses into a keyboard-driven interface, and keeps the defaults centred on MiMo model IDs.

## Features

- Ratatui/crossterm terminal interface with streaming assistant responses.
- `ask`, `doctor`, `models`, and `sessions` CLI commands outside the TUI.
- Searchable help overlay, slash-command registry, slash-menu completion, command palette, multiline input editor, and bounded transcript scrolling.
- Draft stash/history recovery (`Ctrl+S`, `Alt+R`), last-message pager (`l`), and interactive model/session pickers.
- Local session save/load/export with persisted plan checklist and attached workspace context.
- Interactive `/config`, `/status`, `/models`, `/retry`, `/mode`, `/plan`, and `/context` commands.
- Configurable MiMo API key, base URL, model, temperature, and system prompt, with doctor output that shows the winning config source for each value.
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

List live models or local fallbacks:

```bash
cargo run -- models
```

List saved sessions:

```bash
cargo run -- sessions
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
- `/plan [show|add <text>|done <n>|undo <n>|remove <n>|clear]`: manage the local planning checklist
- `/context`: inject a read-only workspace summary into the conversation
- `Tab`: complete a slash command from the slash menu
- `@path` then `Tab`: attach a file or directory to the next prompt
- `Ctrl+K`: open the command palette
- `Ctrl+R`: open the session picker
- `Ctrl+S`: stash the current draft
- `Alt+R`: reopen a cleared or recent draft
- `l`: open the latest message in a pager
- `Ctrl+U`: clear prompt draft and keep it in history
- `Left` / `Right`: move the draft cursor
- `Home` / `End` or `Ctrl+A` / `Ctrl+E`: jump within the current line
- `Delete` / `Backspace`: delete around the cursor, or remove the latest attachment when the draft is empty
- `Up` / `Down`: scroll conversation or move inside open menus
- `PageUp` / `PageDown`: scroll by page
- `Ctrl+Home` / `Ctrl+End`: jump to transcript start/end
- `Esc`: close help or cancel an in-progress generation
- `Ctrl+C` / `Ctrl+D` on an empty draft: quit

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

## Attached workspace context

Use `@path` in the draft and press `Tab` to attach a local file or directory to the next request. Attachments are shown above the prompt editor, serialized into saved sessions, exported in Markdown, and removed with `Backspace` when the draft is empty.

## Session files

By default, session JSON files and Markdown exports are stored under:

```text
~/.config/mimo-tui/sessions/
```

`/save` creates a new JSON snapshot, `/load` restores the most recent saved session when no path is provided, `/sessions` opens an interactive picker, and `/export` writes a Markdown transcript.

## Plan mode

`/mode plan` now opens a dedicated plan side panel. Use `/plan add <text>` to create checklist items, then `/plan done <n>` or `/plan undo <n>` as work progresses. The checklist is included in saved sessions and added back to the request context while plan mode is active.

## Architecture snapshot

- `src/main.rs`: CLI entrypoint and `ask` / `doctor` / `models` / `sessions` dispatcher
- `src/config.rs`: config loading, precedence tracking, validation, and persistence helpers
- `src/client.rs`: OpenAI-compatible MiMo streaming client
- `src/tui.rs`: ratatui shell bootstrap and event loop
- `src/tui/app.rs`: TUI state machine, overlays, commands, and render flow
- `src/tui/commands.rs`: slash-command registry and parsing
- `src/tui/slash_menu.rs`: slash-command suggestions and completion
- `src/tui/command_palette.rs`: palette actions and filtering
- `src/tui/keybindings.rs`: user-facing keybinding catalog and footer hint
- `src/tui/input.rs`: multiline draft editor
- `src/tui/attachments.rs`: `@path` attachment parsing and request-context injection
- `src/tui/session_store.rs`: local session persistence/export
- `src/tui/project_context.rs`: read-only workspace summarization
- `src/tui/tooling.rs`: future-facing tool/approval state skeleton
