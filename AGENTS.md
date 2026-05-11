# AGENTS.md

## Read this first

`.github/copilot-instructions.md` contains the canonical architecture overview, conventions, config details, and CLI reference. Read it before making changes.

## Commands

```bash
cargo check              # fast compile check
cargo fmt --check        # formatting verification (the only lint step)
cargo test --workspace   # full workspace suite
cargo run                # launch TUI
cargo run -- doctor      # print resolved config
cargo run -- ask "..."   # one-shot prompt via MiMo API
```

## Gotchas

- **Edition 2024** — the workspace uses `edition = "2024"`. Ensure the toolchain supports it. Do not downgrade to 2021.
- **Workspace layout** — this repository is now split across `crates/` (`mimo-cli`, `mimo-config`, `mimo-protocol`, `mimo-client`, `mimo-tools`, `mimo-state`, `mimo-agent`, `mimo-tui-core`, `mimo-tui`). Do not reintroduce a monolithic `src/` crate.
- **Tests exist** — use `cargo test --workspace` for the full suite. Do not assume there is no test coverage.
- **CI exists** — `.github/workflows/ci.yml` is present. Keep root cargo commands working for the whole workspace.
- **reqwest is rustls-only** — `default-features = false` with `rustls-tls`. Do not introduce native-tls.
- **Config precedence** — CLI > env > file > built-in. All new settings must thread through `ConfigOverrides` → `AppConfig::load`.
- **Base URL normalization** — `trim_end_matches('/')` in `crates/mimo-config/src/lib.rs`; `crates/mimo-client/src/lib.rs` appends `/chat/completions` relying on that normalization.
