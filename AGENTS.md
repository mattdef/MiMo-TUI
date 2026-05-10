# AGENTS.md

## Read this first

`.github/copilot-instructions.md` contains the canonical architecture overview, conventions, config details, and CLI reference. Read it before making changes.

## Commands

```bash
cargo check              # fast compile check
cargo fmt --check        # formatting verification (the only lint step)
cargo test               # full suite (currently 0 tests)
cargo run                # launch TUI
cargo run -- doctor      # print resolved config
cargo run -- ask "..."   # one-shot prompt via MiMo API
```

## Gotchas

- **Edition 2024** — this crate uses `edition = "2024"`. Ensure the toolchain supports it. Do not downgrade to 2021.
- **No test suite** — there are 0 tests and 0 benchmarks. `cargo test` always passes. Do not look for test infrastructure that doesn't exist.
- **No CI** — there are no `.github/workflows/`, no pre-commit hooks, no Makefile.
- **Single binary crate** — no workspace, no library crate. Everything lives under `src/`.
- **reqwest is rustls-only** — `default-features = false` with `rustls-tls`. Do not introduce native-tls.
- **Config precedence** — CLI > env > file > built-in. All new settings must thread through `ConfigOverrides` → `AppConfig::load`.
- **Base URL normalization** — `trim_end_matches('/')` in `config.rs`; `client.rs` appends `/chat/completions` unsafely relying on that normalization.
