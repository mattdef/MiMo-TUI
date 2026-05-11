# MiMo TUI

> A fast, keyboard-driven terminal client for [Xiaomi MiMo](https://www.xiaomimimo.com/) models — streaming responses, agentic tool execution, and session memory, all without leaving the terminal.

---

## What it does

MiMo-TUI turns your terminal into a capable AI coding workspace. Type a prompt, get a streamed response. Ask MiMo to read your files, run shell commands, apply patches, or review your git diff — it will ask for approval before touching anything, or run fully autonomously in `yolo` mode.

**Highlights:**

- 🖥️ **Full-screen TUI** with streaming responses, multiline editor, and slash-command palette
- 🔧 **Agentic tool loop** — file read/write, shell execution, git, search, and diagnostics
- 💾 **Session persistence** — save, restore, and export conversations as Markdown
- 🗂️ **Plan mode** — keep a live checklist alongside your conversation
- 🧠 **Memory** — persist notes across sessions with `/note` and `/recall`
- 🤖 **Auto model routing** — short prompts on `mimo-v2-flash`, heavy tasks on `mimo-v2.5-pro`
- 🔌 **Skills & MCP** — install reusable prompt skills and wire up MCP server definitions

---

## Quickstart

```bash
export MIMO_API_KEY="your-api-key"
cargo run
```

One-shot from the command line:

```bash
cargo run -- ask "What makes MiMo v2.5 good for coding?"
cargo run -- doctor    # show resolved config and API key status
cargo run -- models    # list available models
```

---

## Workspace layout

MiMo-TUI now follows a multi-crate workspace layout inspired by DeepSeek-TUI:

| Crate | Responsibility |
|---|---|
| `crates/mimo-cli` | CLI entrypoint and subcommand dispatcher |
| `crates/mimo-config` | Config loading, MiMo defaults, validation, and precedence |
| `crates/mimo-protocol` | Shared chat and tool schema types |
| `crates/mimo-client` | MiMo HTTP client, SSE streaming, model listing, and auto-routing |
| `crates/mimo-tools` | Tool specs, registry, file/shell/git/web/patch/diagnostics tools |
| `crates/mimo-state` | Session, task, memory, skill, MCP, and diagnostics persistence |
| `crates/mimo-agent` | Agent loop that executes tool calls around the MiMo client |
| `crates/mimo-core` | Shared bootstrap helpers and project summarization |
| `crates/mimo-tui-core` | Slash commands, keybindings, input buffer, and markdown helpers |
| `crates/mimo-tui` | Ratatui/Crossterm runtime and interactive UI |

The root `Cargo.toml` is now a virtual workspace manifest. `cargo run`, `cargo check`, and `cargo fmt --check` still work from the repository root.

For the full test suite across every crate, use:

```bash
cargo test --workspace
```

---

## Configuration

Create `~/.config/mimo-tui/config.toml` — or use environment variables:

```toml
api_key      = "your-api-key"
model        = "mimo-v2-flash"          # or mimo-v2.5, mimo-v2.5-pro
temperature  = 0.2
# base_url defaults to https://api.xiaomimimo.com/v1
```

| Env var | Purpose |
|---|---|
| `MIMO_API_KEY` | API key |
| `MIMO_MODEL` | Model ID |
| `MIMO_BASE_URL` | Override API endpoint |
| `MIMO_TEMPERATURE` | Sampling temperature |
| `MIMO_TUI_CONFIG` | Custom config file path |

Config can also be edited live inside the TUI with `/config`.

---

## Essential controls

| Key / Command | Action |
|---|---|
| `Enter` | Send prompt |
| `Shift+Enter` | New line in draft |
| `F1` / `?` | Open help |
| `Ctrl+K` | Command palette |
| `Ctrl+R` | Session picker |
| `Tab` | Slash-command completion |
| `@path` + `Tab` | Attach a file or directory |
| `Esc` | Cancel generation / close overlay |
| `/clear` `/save` `/load` | Manage conversation |
| `/mode [agent\|plan\|yolo]` | Switch execution mode |
| `/model [id\|auto]` | Change or auto-route model |
| `/plan` `/note` `/recall` | Checklist, memory, search |
| `/review` `/lsp` | Code review and diagnostics |

Type `/help` inside the TUI for the full command reference.
