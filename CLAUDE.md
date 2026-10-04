# mog

A modeless terminal code editor written in Rust. Half joke, half real tool: it should look cool
(ambient silly widgets, an Obsidian-style graph view, little animated critters) and still be useful
out of the box (LSP, Copilot and Claude, mouse support like VS Code).

See `ROADMAP.md` for what is done and what is next. Update it whenever a roadmap item lands.

## Commands

- Build: `cargo build`
- Run: `cargo run -- [file or folder]`
- Check everything (must pass before every commit):
  - `cargo +nightly fmt --all --check`
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
  critters, etc. go here.
- `mog-lsp`: language server client.
- `mog-ai`: AI providers (Claude, Copilot) behind one trait.

Subsystems never touch the terminal. Everything flows through the event loop in `mog`
(`crates/mog/src/app.rs`), and only `mog-tui` draws.

## Where new things plug in

- Editing command: add a `Command` variant and its name in `mog-core/src/command.rs`, handle it
  in `Editor::execute`, bind it in `DEFAULT_BINDINGS` in `keymap.rs`.
- App level command (needs AI, UI): use a namespaced `Command::Custom` like
  `ai.explain` and handle `Outcome::Unhandled` in `App::execute_command`.
- Screen element (panel, palette, popup): implement `mog_tui::Layer` and push it in `App::new`.
  Layers get mouse events by hit testing and can animate via `tick`.
- Flair: implement `mog_flair::Flair` in `crates/mog-flair/src/builtin/` and add it to
  `builtin::all()`. Keep them decorative, they get no input.
- Language server feature: add the request to `mog-lsp/src/client.rs`, handle results in
  `crates/mog/src/lsp.rs`.
- AI backend: implement `mog_ai::AiProvider` and build it in `settings::ai_providers`.
- Config: add a field with a default in `mog-config`, and keep `examples/config.toml` valid (a
  test parses it).

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
- Tests too: `/// Tests for ...` on the `mod tests` and a one line `///` on each test.

### Imports

- Never write a multi-segment path inline (`crate::a::b`, `std::fs::read`). Add a `use` at the top
  and use the short name.
- If two imports clash, import the parent modules and qualify (`use a::b; use a::d;` then `b::c`
  and `d::c`), or rename with `as`.
