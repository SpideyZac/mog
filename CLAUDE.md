# mog

A modeless terminal code editor written in Rust. Half joke, half real tool: it should look cool
(ambient silly widgets, an Obsidian-style graph view, little animated critters) and still be useful
out of the box (LSP, Copilot and Claude, Lua plugins, mouse support like VS Code).

See `ROADMAP.md` for what is done and what is next. Update it whenever a roadmap item lands.

## Commands

- Build: `cargo build`
- Run: `cargo run -- [file]`
- Check everything (must pass before every commit):
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`

## Layout

Cargo workspace, one crate per concern under `crates/`:

- `mog`: the binary. CLI args, terminal setup and teardown, panic hook, event loop, glue.
- `mog-core`: editing model with no terminal deps. Documents (rope), selections, transactions,
  undo/redo, commands, keymap.
- `mog-config`: TOML config loading and defaults.
- `mog-tui`: ratatui rendering. A compositor of layers, the editor view, the status line, themes,
  mouse hit-testing.
- `mog-flair`: the fun stuff. The `Flair` trait and registry. New silly widgets, the graph view,
  critters, etc. go here (or in plugins).
- `mog-lsp`: language server client.
- `mog-ai`: AI providers (Claude, Copilot) behind one trait.
- `mog-plugin`: Lua plugin host (`mlua`).

Subsystems never touch the terminal. Everything flows through the app event channel in `mog`,
and only `mog-tui` draws.

## Git rules

- Conventional commits (`feat:`, `fix:`, `chore:`, `docs:`, `refactor:`, `test:`, scopes allowed
  like `feat(core):`).
- Super small commits. Every tiny fix or tiny feature gets its own commit.
- Never add a `Co-Authored-By` line or any AI attribution.
- Do not push unless asked.

## Code rules

The lints in the workspace `Cargo.toml` enforce most of these. Do not silence them.

### Comments

- Lowercase, little to no punctuation.
- Only where truly needed, and they explain why, not what.
- No sectioning comments (`// ---- vars ----` and the like).

### Docstrings

- Every file starts with a `//!` module docstring.
- Everything gets a `///` docstring: consts, statics, structs, every field (pub and private),
  enums, every variant, traits, functions, methods, type aliases.
- Proper grammar and capitalization. Use backticks and intra-doc links like [`Document`].
- Keep them short. No paragraphs explaining everything.
- ASCII only. No em dashes, smart quotes or other unicode.
- Add `# Panics`, `# Errors` and `# Safety` sections where they apply.

### Imports

- Never write a multi-segment path inline (`crate::a::b`, `std::fs::read`). Add a `use` at the top
  and use the short name.
- If two imports clash, import the parent modules and qualify (`use a::b; use a::d;` then `b::c`
  and `d::c`), or rename with `as`.
