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
- Accept changed screen snapshots: `cargo insta review` (or `INSTA_UPDATE=always cargo test`)
- Licenses and advisories: `cargo deny check`
- Plugin SDK tests: `python3 sdk/python/test_mog_plugin.py` and `node sdk/node/test.js`
- Timings, by hand: `cargo test --release -p mog-tui --test large_files -- --ignored --nocapture`
  and `cargo test --release -p mog frame_cost -- --ignored --nocapture`

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
- `mog-dap`: debug adapter client.
- `mog-plugin`: the plugin host, JSON-RPC over stdio, and plugin manifests.
- `mog-plugin-sdk`: the Rust SDK for writing plugins. The Python and Node SDKs are in `sdk/`.
- `mog-plugin-test`: a fake editor that runs a plugin and test scripts, behind `mog plugin test`.

Subsystems never touch the terminal. Everything flows through the event loop in `mog`
(`crates/mog/src/app.rs`), and only `mog-tui` draws. The app's handling of each subsystem lives
in a child module of `app`, like `app/language.rs` for language servers or `app/plugin_host.rs`
for plugins.

## Where new things plug in

- Editing command: add a `Command` variant and its name in `mog-core/src/command.rs`, handle it
  in `Editor::execute`, bind it in `DEFAULT_BINDINGS` in `keymap.rs`.
- App level command (needs AI, UI): use a namespaced `Command::Custom` like
  `ai.explain`, handle it in `App::execute_custom` (`crates/mog/src/app/custom.rs`) and list it
  in `crates/mog/src/commands.rs` for the palette.
- Sidebar view or button: `crates/mog-tui/src/sidebar.rs` has the bar of buttons (`buttons`), the
  source control view and the drag handle, `SidebarView` and `SidebarState` in `ui.rs` name the
  views, and plugins add one with a `panel` on the `sidebar` side.
- Screen element (panel, palette, popup): implement `mog_tui::Layer` and push it in `App::new`.
  Layers get mouse events by hit testing and can animate via `tick`.
- Flair: implement `mog_flair::Flair` in `crates/mog-flair/src/builtin/` and add it to
  `builtin::all()`. Keep them decorative, they get no input.
- Language server feature: add the request to `mog-lsp/src/client.rs`, handle results in
  `crates/mog/src/lsp.rs`.
- AI backend: implement `mog_ai::AiProvider` and build it in `settings::ai_providers`.
- Config: add a field with a default in `mog-config`, and keep `examples/config.toml` valid (a
  test parses it).
- Plugin protocol: `mog-plugin` speaks it, `crates/mog/src/plugins.rs` runs the plugins and
  `crates/mog/src/app/plugin_host.rs` answers them. Keep `docs/plugins.md`,
  `docs/plugin-protocol.schema.json`, the SDKs in `sdk/` and `crates/mog-plugin-sdk`, and the
  examples in `examples/plugins` in step with it. The SDKs are not copied into plugins: mog
  ships them (`crates/mog/src/sdk.rs`) and puts them on `PYTHONPATH` and `NODE_PATH`.
- Plugin screen and keys: `mog_tui` draws widgets (`PluginWidgets`), notifications (`Toasts`),
  panels (`PluginPanels`) and the flair canvas (`PluginCanvases`), and
  `crates/mog/src/app/plugin_host/screen.rs` handles `draw`, `capture`, `cursor`, `timer`,
  `toast`, `panel` and `canvas`.
- Plugin contributions (themes, keys, highlight queries, chat tools) are applied in
  `crates/mog/src/app/plugin_host/contributions.rs`, workspace edits in `plugin_host/edits.rs`.
- `mog plugin` is `crates/mog/src/plugin_cli.rs`, with installs and updates in
  `plugin_cli/source.rs` and the doctor in `plugin_cli/doctor.rs`. Plugins are not sandboxed, so
  keep saying so wherever installing one is explained.
- Releases: `docs/releasing.md`, templates in `packaging/`, workflows in `.github/workflows`.
- Debugger: `mog-dap` is the client, `crates/mog/src/debug.rs` runs sessions, built in adapters
  are in `mog-config/src/tasks.rs`. Tasks and their output parsing live in
  `crates/mog/src/tasks.rs` and `mog-core/src/problems.rs`.

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
