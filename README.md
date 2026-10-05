# mog

**A terminal code editor that mogs other editors.**

![mog](docs/editor.png)

Normal keybindings. Full mouse support. Language servers, Claude and Copilot built in. Also a
tiny face that judges your compile errors, critters living in your whitespace, and chiptune
music that gets tense when the build breaks.

It's a joke. It's also a real editor you can get work done in.

```sh
mog .
```

## What's in it

**The editor**

- Modeless editing with the keys you already know, and a mouse that works like VS Code
  (drag select, double and triple click, right click menus)
- Tabs, splits, multiple cursors, find and replace in a file or the whole project, a built in
  terminal you can scroll and restart
- File explorer, fuzzy finder, command palette and a key list you can rebind from
- Tree-sitter highlighting for over thirty languages, including code inside Vue, HTML and
  Markdown blocks
- LSP: diagnostics with error lens, completion, hover, go to definition, references, rename,
  formatting, code actions, inlay hints, semantic colors, signature help, an outline and project
  wide symbol search
- Picks up where you left off: opening a folder brings back its files, split and cursors, undo
  history survives restarts, and unsaved work is kept on disk so a crash or a closed terminal
  does not lose it
- Git gutter, branch in the status line, file colors and inline blame, plus a source control
  panel with diffs, staging of whole files or single changes, and commits
- Claude chat, explain selection and ghost text. Copilot ghost text.
- Updates itself from GitHub releases and shows what changed
- Seven themes, a theme editor where you drag colors around, and a settings menu that writes
  to your config
- A serious mode that turns all the fun off, a reduced motion setting, and `NO_COLOR` support

**The rest**

- An Obsidian style graph of your project
- A marker for drawing anywhere on the screen, with a pen, a marker, an eraser and text
- An AI meter that guesses how AI generated your file is
- Your LAN IP, displayed like it got leaked
- The mogling, a little face with feelings about your errors
- A combo counter with sparks flying off the cursor
- Aura points, a RAM download and an FBI agent counter
- A $MOG stock ticker and a weather report for your codebase up in the tab bar
- A resource monitor and a fake live stream chat that roasts your errors, under the explorer
- Matrix rain when you go idle
- Sound effects, music and spectrum bars for whatever your computer is playing
- Discord Rich Presence

All of it can be switched off.

## Install

Grab a build for Windows, macOS or Linux from
[releases](https://github.com/SpideyZac/mog/releases), unpack it and put `mog` somewhere on your
`PATH`.

mog updates itself. When a new release is out it downloads in the background, checks its
[minisign](https://jedisct1.github.io/minisign/) signature against the key built into mog and is
ready the next time you start mog, which then shows what changed. Turn that off in
[`[updates]`](#updates), or update by hand with `mog --update`.

To check a download yourself:

```sh
minisign -Vm mog-<version>-<target>.zip -P RWQEyQrj2l2VtRVkLbwHBkVxhMbDdbOGc7wHR8hjR27Ry+epMjmzedE0
```

### Building it yourself

You need stable Rust 1.90 or newer and a C compiler for the tree-sitter
grammars. On Linux you also need ALSA headers:

```sh
# debian, ubuntu
sudo apt install build-essential pkg-config libasound2-dev
# fedora
sudo dnf install gcc pkgconf-pkg-config alsa-lib-devel
```

Then:

```sh
git clone https://github.com/SpideyZac/mog
cd mog
cargo build --release
./target/release/mog [file or folder]
```

Or `cargo install --path crates/mog` to put it in `~/.cargo/bin`.

### Usage

```
mog [PATH]              open a file, or a folder in the explorer
mog --run <COMMAND>     run a command after starting, can be repeated
mog --keys              print every key binding and exit
mog --update            install the newest release and exit
mog --version
```

`--run` takes any command name from the [command list](#commands), like
`mog . --run terminal.toggle`.

## Language servers

mog starts a server when you open a file it handles, if the program is on your `PATH`. When it
is not, mog says how to install it and, where one command does it, offers to run that command in
the built in terminal. Run `Code: Restart language servers` once it is done.

Besides diagnostics, completion, hover, go to definition, references, rename, formatting and code
actions, mog shows inferred types at the end of lines (inlay hints), colors from the server on top
of the syntax colors (semantic tokens), the signature of the call you are typing, and an outline
of the file and project wide symbol search.

| name | program | extensions |
| --- | --- | --- |
| `rust` | `rust-analyzer` | `rs` |
| `python` | `pyright-langserver --stdio` | `py` |
| `typescript` | `typescript-language-server --stdio` | `ts` `tsx` `js` `jsx` |
| `go` | `gopls` | `go` |
| `c` | `clangd` | `c` `h` `cpp` `hpp` `cc` `cxx` `hh` |
| `lua` | `lua-language-server` | `lua` |
| `zig` | `zls` | `zig` |
| `java` | `jdtls` | `java` |
| `csharp` | `csharp-ls` | `cs` |
| `kotlin` | `kotlin-language-server` | `kt` `kts` |
| `swift` | `sourcekit-lsp` | `swift` |
| `ruby` | `ruby-lsp` | `rb` |
| `php` | `intelephense --stdio` | `php` |
| `scala` | `metals` | `scala` `sbt` |
| `haskell` | `haskell-language-server-wrapper --lsp` | `hs` |
| `ocaml` | `ocamllsp` | `ml` `mli` |
| `elixir` | `elixir-ls` | `ex` `exs` |
| `dart` | `dart language-server` | `dart` |
| `bash` | `bash-language-server start` | `sh` `bash` |
| `html` | `vscode-html-language-server --stdio` | `html` `htm` |
| `css` | `vscode-css-language-server --stdio` | `css` `scss` |
| `json` | `vscode-json-language-server --stdio` | `json` `jsonc` |
| `yaml` | `yaml-language-server --stdio` | `yml` `yaml` |
| `toml` | `taplo lsp stdio` | `toml` |
| `markdown` | `marksman` | `md` |
| `vue` | `vue-language-server --stdio` | `vue` |

Add more or change these under [`[lsp]`](#lsp) in the config.

## AI

**Claude.** Set `ANTHROPIC_API_KEY`, then turn on `ai.claude.enabled`. You get a chat panel
(`ctrl+l`), explain the selection (`alt+e`) and ghost text.

**Copilot.** Install GitHub's language server, turn on `ai.copilot.enabled`, then run
`Copilot: Sign in` from the command palette.

```sh
npm i -g @github/copilot-language-server
```

Ghost text shows up in gray when you stop typing. `tab` takes all of it, `ctrl+right` takes the
next word, `alt+]` and `alt+[` cycle suggestions, `esc` dismisses.

## Config

Everything is optional. A missing or empty file is a valid config. Unknown keys are an error, so
typos don't fail silently.

| platform | path |
| --- | --- |
| Linux | `~/.config/mog/config.toml` |
| macOS | `~/Library/Application Support/mog/config.toml` |
| Windows | `%APPDATA%\mog\config.toml` |

`Settings: Open config file` from the palette opens it. Saving it reloads it. The settings menu
(`ctrl+,`) edits the same file and keeps your comments.

There's a full example in [examples/config.toml](examples/config.toml).

### Environment variables

| variable | what it does |
| --- | --- |
| `MOG_CONFIG_DIR` | Use this folder instead of the platform config folder |
| `ANTHROPIC_API_KEY` | Claude API key. The name can be changed with `ai.claude.api_key_env` |
| `SHELL` | Shell for the built in terminal on macOS and Linux, unless `terminal.shell` is set |
| `PATH` | Where language servers and the Copilot server are looked up |

API keys are never read from or written to the config file.

### `[editor]`

| key | default | |
| --- | --- | --- |
| `tab_width` | `4` | Width of a tab stop in cells |
| `insert_spaces` | `true` | `tab` inserts spaces |
| `auto_close_brackets` | `true` | Typing `(`, `[`, `{` or a quote also types the closing one |
| `auto_complete` | `true` | Completions pop up while typing |
| `diagnostics_delay` | `500` | Milliseconds of no typing before language servers check the file |
| `restore_session` | `true` | Opening a folder brings back the files, split and cursors from last time |
| `persistent_undo` | `true` | Keep undo history for each file between runs, as long as the file did not change elsewhere |
| `kitty_keyboard` | `true` | Use the kitty keyboard protocol in terminals that have it, so every chord arrives as itself |
| `alt_gr` | `true` | Symbols typed with AltGr (`{` on QWERTZ, `@` on AZERTY) type text instead of running `ctrl+alt` shortcuts. Only matters on layouts with AltGr |

### `[ui]`

| key | default | |
| --- | --- | --- |
| `theme` | `"mog"` | `mog`, `synthwave`, `matrix`, `sunset`, `ocean`, `forest`, `paper` or one of your [`[themes]`](#themes) |
| `tabs` | `true` | Open files as tabs along the top |
| `line_numbers` | `true` | Line numbers |
| `relative_line_numbers` | `false` | Count line numbers from the cursor |
| `cursor_line` | `true` | Highlight the cursor line |
| `indent_guides` | `true` | Thin guides at each indent level |
| `rainbow_brackets` | `true` | Color nested brackets by depth |
| `syntax_highlighting` | `true` | Syntax colors |
| `semantic_highlighting` | `true` | Colors from the language server on top, so parameters, macros and types stand out |
| `inlay_hints` | `true` | Inferred types and parameter names from the language server, shown at the end of the line |
| `minimap` | `true` | Zoomed out view of the file on the right |
| `error_lens` | `true` | Write diagnostics at the end of their line |
| `git_gutter` | `true` | Mark changed lines next to the line numbers |
| `git_blame` | `true` | Show who last changed the cursor line |
| `explorer` | `true` | Open the file explorer when opening a folder |
| `icons` | `true` | File icons |
| `serious` | `false` | Serious mode: turns off every flair, sound and music at once |
| `reduced_motion` | `false` | Hide sparks, the combo counter, critters and matrix rain, and stop things shimmering |
| `opacity` | `100` | How solid backgrounds are, 0 to 100. Below 100 your terminal shows through mog, if the terminal itself is see through (Windows Terminal opacity or acrylic, kitty `background_opacity`, Alacritty `window.opacity`, ...) |

Setting [`NO_COLOR`](https://no-color.org) to anything turns every color off, whatever the theme.
Selections, the status line and matches are marked with reverse video and underlines instead.

### `[updates]`

| key | default | |
| --- | --- | --- |
| `check` | `true` | Look for a new release on GitHub when mog starts |
| `install` | `true` | Download and install it by itself, ready the next time mog starts |

Builds made with `cargo build` or `cargo run` are never replaced.

### `[themes]`

Make your own theme from a built in one. Every color is optional and missing ones come from
`base`. Pick it with `ui.theme` like any other.

```toml
[ui]
theme = "midnight"

[themes.midnight]
base = "ocean"
accent = "#ff5ccd"
bg = "#070b14"
```

The colors are `bg`, `panel`, `raised`, `select`, `fg`, `dim`, `accent`, `accent2`, `red`,
`orange`, `yellow`, `green`, `cyan`, `blue`, `purple` and `pink`.

Or run `Theme: Edit colors` (`theme.edit`) and drag them around. See [theme editor](#theme-editor).

### `[keys]`

Maps a chord to a command name. An empty string removes a default binding.

```toml
[keys]
"ctrl+d" = "select_all"
"ctrl+shift+k" = ""
```

Chords are modifiers joined with `+`: `ctrl`, `alt`, `shift`, then a key like `a`, `f5`, `enter`,
`tab`, `esc`, `backspace`, `delete`, `insert`, `home`, `end`, `pageup`, `pagedown`, `up`, `down`,
`left`, `right` or `space`. You can also rebind from the key list (`ctrl+k`, pick a command,
press the new keys).

### `[flair]`

| key | default | |
| --- | --- | --- |
| `enabled` | `true` | Master switch for all of it |
| `disabled` | `[]` | Ids of flairs to turn off |

| id | what |
| --- | --- |
| `splash` | Welcome screen on an empty buffer |
| `badge` | Rainbow `mog` badge in the corner |
| `pet` | The mogling |
| `quips` | One liners in the status line when something happens |
| `critters` | Little guys that walk through blank space |
| `combo` | Typing combo counter |
| `sparks` | Sparks off the cursor |
| `ai_meter` | How AI generated the file looks |
| `leaked_ip` | Your LAN IP |
| `aura` | Aura points |
| `download_ram` | A RAM download that never finishes |
| `fbi` | FBI agents watching you type |
| `session` | Session timer that eventually tells you to go outside |
| `screensaver` | Matrix rain when idle |
| `spectrum` | Spectrum bars for system audio |
| `stonks` | A $MOG stock ticker in the tab bar. Saving pumps it, errors dump it |
| `weather` | The weather in your codebase, in the tab bar |
| `resources` | CPU, RAM and GPU usage under the explorer. The GPU is mining $MOG |
| `stream` | A fake live stream chat under the explorer that reacts to your code |

### `[lsp]`

One table per server. Setting a built in server's name changes only what you set, so
`[lsp.python] command = "my-pyright"` keeps its args and extensions.

| key | default | |
| --- | --- | --- |
| `enabled` | `true` | Start this server |
| `command` | | Program to run |
| `args` | `[]` | Its arguments |
| `extensions` | `[]` | File extensions it handles, no dot |
| `language_id` | server name | Language id sent to the server |
| `settings` | | Table sent as initialization options and on `workspace/configuration` |

```toml
[lsp.nim]
command = "nimlangserver"
extensions = ["nim"]

[lsp.go]
enabled = false

[lsp.rust.settings]
check.command = "clippy"
```

#### Per project settings

A project can have its own language server settings in `.mog/config.toml` at its root, using
the same `[lsp.<name>]` tables. They change only what they set: `settings` tables merge key by
key, and anything left out comes from your global config. `Settings: Open project config`
(`config.open_project`) creates the file.

```toml
# .mog/config.toml
[lsp.rust.settings]
check.command = "clippy"
cargo.features = ["serde"]

[lsp.python]
command = "pylsp"

[lsp.go]
enabled = false
```

Since a project config can pick programs to run, mog asks before using one and asks again
whenever it changes. Saving it applies it right away. Project configs can also hold
[`[tasks]`](#tasks) and [`[debug]`](#debug) tables.

### `[tasks]`

`Tasks: Run task` (`ctrl+shift+b`) lists the project's tasks and runs the one you pick, `alt+b`
runs the last one again. mog finds these on its own:

| project | tasks |
| --- | --- |
| `Cargo.toml` | `build`, `check` (clippy), `test`, `run` |
| `go.mod` | `build`, `test`, `check` (vet) |
| `package.json` | every script, with npm, pnpm, yarn or bun |
| `Makefile` | `build` (make), `test` (make test) |
| `pyproject.toml`, `pytest.ini` | `test` (pytest) |

Add your own or replace those by name. The command runs through the shell.

| key | default | |
| --- | --- | --- |
| `command` | | The command line, like `cargo build --release` |
| `cwd` | project | Folder to run it in, relative to the project |

```toml
[tasks.lint]
command = "cargo clippy --all-targets"
```

The output shows in `Tasks: Show output`, and errors and warnings in it (rustc, gcc, clang, go,
TypeScript, mypy, ruff, eslint and Python tracebacks) go to the problems list (`alt+m`), even
for files that are not open.

### `[debug]`

mog debugs with any debugger that speaks the
[debug adapter protocol](https://microsoft.github.io/debug-adapter-protocol/) over stdio. `f5`
picks one by the extension of the focused file. Two work out of the box once installed:

| name | adapter | files | launches |
| --- | --- | --- | --- |
| `lldb` | `lldb-dap` (comes with LLVM) | Rust, C, C++, Zig, Swift | `target/debug/<folder name>`, after the `build` task |
| `python` | `python -m debugpy.adapter` (`pip install debugpy`) | Python | the focused file |

| key | default | |
| --- | --- | --- |
| `command` | | The adapter program |
| `args` | `[]` | Its arguments |
| `extensions` | `[]` | Files it is picked for, no dot |
| `request` | `"launch"` | `launch` to start the program or `attach` to join one |
| `arguments` | | The launch or attach arguments, which every adapter names its own way |
| `before` | `""` | A task to run first, like `build`. Debugging starts if it passes |

Strings in `arguments` can use `${root}`, `${rootName}` (the folder name), `${file}`,
`${fileDirname}`, `${fileBasenameNoExtension}` and `${exe}` (`.exe` on Windows).

```toml
# a C program built with make
[debug.lldb]
command = "lldb-dap"
extensions = ["c"]
before = "build"

[debug.lldb.arguments]
program = "${root}/build/app${exe}"
args = ["--verbose"]
cwd = "${root}"
```

`f9` sets a breakpoint on the cursor line. When the program stops, the line is marked, and the
debug panel (`ctrl+shift+d`) shows the call stack, the variables and what the program printed.
Click a frame to look at it.

### `[ai]`

| key | default | |
| --- | --- | --- |
| `ghost_text` | `true` | Master switch for ghost text from any provider |

`[ai.claude]`

| key | default | |
| --- | --- | --- |
| `enabled` | `false` | Use Claude |
| `chat` | `true` | Answer in the chat panel |
| `ghost_text` | `true` | Suggest ghost text |
| `model` | provider default | Model id, like `"claude-opus-5-5"` |
| `api_key_env` | `"ANTHROPIC_API_KEY"` | Environment variable holding the key |

`[ai.copilot]`

| key | default | |
| --- | --- | --- |
| `enabled` | `false` | Use Copilot |
| `chat` | `true` | Answer in the chat panel |
| `ghost_text` | `true` | Suggest ghost text |
| `command` | `"copilot-language-server"` | The Copilot language server |
| `args` | `["--stdio"]` | Its arguments |

### `[audio]`

| key | default | |
| --- | --- | --- |
| `sound_effects` | `false` | Sounds for typing, saving and errors |
| `music` | `false` | Background music that gets tense when there are errors |
| `volume` | `50` | 0 to 100 |

### `[terminal]`

| key | default | |
| --- | --- | --- |
| `shell` | `""` | Shell to run. Empty means PowerShell on Windows, `$SHELL` elsewhere |
| `args` | `[]` | Its arguments |

### `[discord]`

| key | default | |
| --- | --- | --- |
| `enabled` | `false` | Show what you're mogging on your Discord profile |
| `client_id` | `"1556366148948459571"` | The mog app, or your own from the [Discord developer portal](https://discord.com/developers/applications) |
| `large_image` | `"mog"` | Art asset key for the big picture |
| `show_file` | `true` | Show the file name. Turn off to keep it secret |

## Keys

`ctrl+k` lists everything and lets you rebind. `mog --keys` prints the same list. Where two
chords are listed, either works; the `alt` versions are there for terminals that eat the `ctrl`
ones.

Terminals that speak the [kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/)
(kitty, WezTerm, foot, Ghostty, Alacritty, recent iTerm2) send every chord as itself, and mog turns
it on when it is there. Older terminals send control keys as single bytes, so `ctrl+i` looks like
tab, `ctrl+shift+p` arrives as `ctrl+p` and `ctrl+,` does not arrive at all. In those terminals the
key list marks the chords that cannot get through with `(!)`, so you know to use the `alt` version
or rebind it. Windows always gets every chord. Turn the protocol off with `editor.kitty_keyboard`.

On layouts with AltGr (German, Swiss, French, Spanish, Italian, Nordic, Polish and most other
European ones), Windows sends AltGr as `ctrl+alt`, so `editor.alt_gr` turns those into typed
symbols. A few default chords use keys that are awkward or dead keys there, like `` ctrl+` ``,
`ctrl+\` and `ctrl+/`; rebind them from `ctrl+k`. On macOS, keep "Option as Meta" off in your
terminal (or on for the left Option only) so Option still types symbols.

### File

| keys | command | |
| --- | --- | --- |
| `ctrl+s` | `save` | Save |
| `ctrl+shift+s` | `file.save_as` | Save as |
| `ctrl+n` | `file.new` | New untitled file |
| `ctrl+p` | `finder.files` | Find a file |
| `ctrl+w` | `close_tab` | Close tab |
| `ctrl+pagedown` `alt+right` | `next_tab` | Next tab |
| `ctrl+pageup` `alt+left` | `prev_tab` | Previous tab |
| `ctrl+q` | `quit` | Quit |

### Edit

| keys | command | |
| --- | --- | --- |
| `ctrl+z` | `undo` | Undo |
| `ctrl+y` `ctrl+shift+z` | `redo` | Redo |
| `ctrl+c` | `copy` | Copy |
| `ctrl+x` | `cut` | Cut |
| `ctrl+v` | `paste` | Paste |
| `ctrl+a` | `select_all` | Select all |
| `ctrl+/` `alt+/` | `toggle_comment` | Toggle comment |
| `alt+shift+down` | `duplicate_line` | Duplicate line |
| `ctrl+shift+k` | `delete_line` | Delete line |
| `alt+up` | `move_line_up` | Move line up |
| `alt+down` | `move_line_down` | Move line down |
| `tab` | `insert_tab` | Indent or insert a tab |
| `shift+tab` | `outdent` | Outdent |
| `enter` | `insert_newline` | New line, keeping indent |
| `backspace` | `delete_backward` | |
| `ctrl+backspace` | `delete_word_backward` | |
| `delete` | `delete_forward` | |

### Cursors

| keys | command | |
| --- | --- | --- |
| `ctrl+d` | `select_next_occurrence` | Add a cursor on the next match |
| `ctrl+shift+l` | `select_all_occurrences` | Cursor on every match |
| `ctrl+alt+up` | `add_cursor_above` | Add cursor above |
| `ctrl+alt+down` | `add_cursor_below` | Add cursor below |
| `esc` | `ui.escape` | Back to one cursor, close popups |

### Moving and selecting

Add `shift` to any of these to select.

| keys | command |
| --- | --- |
| `up` `down` `left` `right` | `move_up` `move_down` `move_left` `move_right` |
| `ctrl+left` `ctrl+right` | `move_word_left` `move_word_right` |
| `home` `end` | `move_line_start` `move_line_end` |
| `pageup` `pagedown` | `move_page_up` `move_page_down` |
| `ctrl+home` `ctrl+end` | `move_doc_start` `move_doc_end` |
| `ctrl+up` `ctrl+down` | scroll one line |

The selecting versions are named `select_*`, like `select_word_left` and `select_doc_end`.

### Search and go to

| keys | command | |
| --- | --- | --- |
| `ctrl+f` | `search.find` | Find |
| `ctrl+h` | `search.replace` | Find and replace |
| `ctrl+shift+f` `alt+f` | `project_search.open` | Find in every file of the project |
| `ctrl+shift+h` | `project_search.replace` | Replace in every file of the project |
| `f3` | `search.next` | Next match |
| `shift+f3` | `search.prev` | Previous match |
| `ctrl+g` | `goto.prompt` | Go to line |
| `f12` | `lsp.definition` | Go to definition |
| `shift+f12` | `lsp.references` | Find references |
| `ctrl+shift+o` `alt+o` | `lsp.symbols` | Go to a symbol in the file, an outline |
| `ctrl+t` `alt+t` | `lsp.workspace_symbols` | Go to a symbol anywhere in the project |
| `f8` | `problems.next` | Next problem |
| `shift+f8` | `problems.prev` | Previous problem |

### Code

| keys | command | |
| --- | --- | --- |
| `ctrl+space` | `lsp.complete` | Complete |
| `alt+h` | `lsp.hover` | Hover info |
| `f2` | `lsp.rename` | Rename symbol |
| `alt+shift+f` | `lsp.format` | Format file |
| `ctrl+.` `alt+enter` | `lsp.actions` | Quick fixes and refactors |
| `ctrl+shift+space` | `lsp.signature` | Signature of the call, also shown after typing `(` or `,` |
| | `lsp.restart` | Restart language servers |

### View

| keys | command | |
| --- | --- | --- |
| `ctrl+b` | `explorer.toggle` | Toggle file explorer |
| `ctrl+shift+e` | `explorer.focus` | Focus file explorer |
| `ctrl+\` | `split.toggle` | Split editor |
| `alt+\` | `split.focus` | Focus other split |
| ``ctrl+` `` ``alt+` `` | `terminal.toggle` | Toggle terminal |
| `ctrl+shift+m` `alt+m` | `problems.list` | Problems |
| `alt+g` | `graph.toggle` | Project graph |
| `alt+d` | `annotate.toggle` | Draw on the screen |

### Git

| keys | command | |
| --- | --- | --- |
| `ctrl+shift+g` `alt+shift+g` | `git.panel` | Source control: changed files and their diffs |
| `alt+s` | `git.stage_hunk` | Stage the change at the cursor |
| `alt+shift+s` | `git.unstage_hunk` | Unstage the change at the cursor |
| `alt+shift+r` | `git.revert_hunk` | Put the change at the cursor back to the last commit |
| | `git.stage_file` | Stage the whole file |
| | `git.commit` | Commit what is staged |

### Tasks and debugging

| keys | command | |
| --- | --- | --- |
| `ctrl+shift+b` | `task.run` | Pick a task like build or test and run it |
| `alt+b` | `task.rerun` | Run the last task again |
| | `task.stop` | Stop the running task |
| | `task.output` | Show the task output |
| `f5` | `debug.start` | Start debugging, or continue when paused |
| `shift+f5` | `debug.stop` | Stop debugging |
| `f9` | `debug.toggle_breakpoint` | Toggle a breakpoint on the cursor line |
| `f10` | `debug.step_over` | Step over |
| `f11` | `debug.step_into` | Step into |
| `shift+f11` | `debug.step_out` | Step out |
| `f6` | `debug.pause` | Pause |
| `ctrl+shift+d` | `debug.panel` | Show or hide the debug panel |

### AI

| keys | command | |
| --- | --- | --- |
| `ctrl+l` | `ai.chat` | Toggle chat |
| `alt+e` | `ai.explain` | Explain the selection |
| `tab` | | Accept ghost text |
| `ctrl+right` | | Accept the next word of ghost text |
| `alt+]` `alt+[` | | Next and previous suggestion |
| `esc` | | Dismiss ghost text |

### Help and settings

| keys | command | |
| --- | --- | --- |
| `ctrl+shift+p` `alt+p` `f1` | `command_palette` | Command palette |
| `ctrl+k` | `help.keys` | Key bindings |
| `ctrl+,` `alt+,` | `settings.open` | Settings |

### Commands

These have no default keys. Run them from the palette, bind them in `[keys]`, or pass them to
`--run`.

| command | |
| --- | --- |
| `file.create` | Create a file in the project |
| `config.open` | Open the config file |
| `config.reload` | Reload the config |
| `config.open_project` | Open this project's `.mog/config.toml` |
| `theme.next` | Next theme |
| `theme.edit` | Make your own theme |
| `flair.toggle` | Toggle all flair |
| `annotate.clear` | Wipe the drawing |
| `search.toggle_case` | Toggle match case in the open search |
| `search.toggle_word` | Toggle whole word in the open search |
| `search.toggle_regex` | Toggle regex in the open search |
| `terminal.restart` | Start a fresh shell in the terminal |
| `audio.toggle_music` | Toggle music |
| `audio.toggle_effects` | Toggle sound effects |
| `copilot.sign_in` | Sign in to Copilot |
| `copilot.sign_out` | Sign out of Copilot |
| `copilot.status` | Show Copilot status |
| `help.release_notes` | What's new, from the GitHub release |
| `update.check` | Look for a new release |
| `update.install` | Install the new release |

### In panels

**Find and replace**

| keys | |
| --- | --- |
| `enter` `down` | Next match |
| `shift+enter` `up` | Previous match |
| `tab` | Switch between find and replace fields |
| `enter` in replace | Replace one |
| `alt+enter` `ctrl+enter` | Replace all |
| `alt+c` | Match case (`Aa`) |
| `alt+w` | Whole words only (`ab`) |
| `alt+r` | Regular expressions (`.*`), with `$1` or `${name}` in the replacement for groups |
| `ctrl+backspace` | Clear the field |
| click `Aa` `ab` `.*` | Toggle the same options |
| `esc` | Close |

**Source control** (`ctrl+shift+g`)

| keys | |
| --- | --- |
| `up` `down` | Pick a file, its diff shows on the right |
| `space` `s` `u` | Stage or unstage the picked file |
| `a` | Stage everything |
| `c` | Commit what is staged |
| `enter` | Open the file |
| `pageup` `pagedown` | Scroll the diff |
| `r` | Read the changes again |
| `esc` | Close |

**Find in project** (`ctrl+shift+f`, needs a folder open)

| keys | |
| --- | --- |
| type | Search as you type, every file the explorer would show |
| `tab` | Switch between find and replace fields |
| `up` `down` `pageup` `pagedown` | Pick a match |
| `enter` | Open the match |
| `alt+enter` `ctrl+enter` | Replace every match, after asking |
| `alt+c` | Match case (`Aa`) |
| `alt+w` | Whole words only (`ab`) |
| `alt+r` | Regular expressions (`.*`), with `$1` or `${name}` in the replacement for groups |
| `ctrl+backspace` | Clear the field |
| click `Aa` `ab` `.*` | Toggle the same options |
| `esc` | Close |

Replacing changes open files in the editor so you can undo it, and saves the others right away.

**File explorer**

| keys | |
| --- | --- |
| `up` `down` `pageup` `pagedown` `home` `end` | Move |
| `enter` `space` | Open file or toggle folder |
| `right` | Expand, or step into |
| `left` | Collapse, or go to parent |
| any letter | Jump to the next entry starting with it |
| `ctrl+n` `insert` | New file |
| `f2` | Rename |
| `delete` | Delete |
| `esc` | Back to the editor |

**Pickers** (finder, palette, problems, key list)

| keys | |
| --- | --- |
| type | Filter |
| `up` `down` `tab` `pageup` `pagedown` | Move |
| `enter` | Pick |
| `ctrl+backspace` | Clear the filter |
| `esc` | Close |

In the key list, `enter` on a command waits for new keys. `backspace` unbinds it, `esc` cancels.

**Completion menu**

| keys | |
| --- | --- |
| `up` `down` `pageup` `pagedown` | Move |
| `enter` `tab` | Accept |
| `esc` | Close |

**Settings menu**

| keys | |
| --- | --- |
| `up` `down` `tab` `pageup` `pagedown` | Move |
| `enter` `space` `right` | Toggle, or next value |
| `left` | Previous value |
| `esc` | Close |

**AI chat**

| keys | |
| --- | --- |
| `enter` | Send |
| `shift+enter` `alt+enter` | New line |
| `up` `down` `pageup` `pagedown` | Scroll |
| `ctrl+backspace` | Clear the input |
| `esc` | Back to the editor |

**Drawing** (`alt+d`)

| keys | |
| --- | --- |
| left drag | Draw with the current tool |
| right drag | Erase |
| `p` `m` `e` `t` | Pen, marker, eraser, text |
| `1` to `8` | Pick a color |
| `u` `ctrl+z` | Undo |
| `c` | Clear everything |
| `esc` | Stop typing text, then stop drawing |

The drawing stays on screen after you stop, until you clear it. The toolbar at the top is
clickable too.

**Theme editor** (`theme.edit`)

The editor behind it shows the theme live as you change it.

| keys | |
| --- | --- |
| click a color, `up` `down` | Pick the color to change |
| drag a swatch onto another | Swap the two colors |
| drag in the big field | Set hue (across) and lightness (down) |
| drag a slider | Set hue, saturation or lightness |
| `tab` `shift+tab` | Pick a slider |
| `left` `right` | Move the slider, `shift` for bigger steps |
| `#` | Type a hex color |
| `r` | Reset the color |
| `enter` | Save under a name in `[themes]` |
| `esc` | Throw away the changes |

**Project graph**

| keys | |
| --- | --- |
| type | Search for a file |
| `tab` `shift+tab` | Cycle matches |
| `enter` | Open the file |
| arrows | Pan |
| `+` `-` | Zoom |
| `0` | Fit everything |
| `esc` | Clear the search, then close |

**Terminal**

Every key goes to the shell except the ones bound to `terminal.toggle`, `command_palette` and
`quit`.

| keys | |
| --- | --- |
| wheel | Scroll back through the output, or scroll inside full screen programs like `less` |
| `shift+pageup` `shift+pagedown` | Scroll back a page |
| click `⟳ restart` | Start a fresh shell, same as `terminal.restart` |

### Mouse

Click to place the cursor, drag to select, shift click to extend, double click for a word, triple
click for a line, click the gutter to select lines. The wheel scrolls whatever is under it. Right
click opens a context menu. Tabs, the explorer, panels and popups are all clickable.

## License

Apache 2.0. See [LICENSE](LICENSE).
