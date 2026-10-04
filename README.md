# mog

A terminal code editor that mogs other editors.

mog is a little bit of a joke. It has silly stuff on screen for no reason. It is also a real
editor you can get work done in.

## The real editor part

- Modeless editing with normal keybindings and full mouse support, right click menus included
- Tabs, splits, multiple cursors, find and replace, auto closing brackets
- A file explorer, fuzzy file finder, command palette and a searchable key binding list
- Tree-sitter syntax highlighting for twelve languages
- Language servers: diagnostics with error lens, completion, hover, go to definition,
  references, rename, formatting and code actions
- Git: changed lines in the gutter, branch in the status line, file colors and inline blame
- Claude: a chat panel, explain the selection and ghost text suggestions
- A settings menu (`ctrl+,`) where almost everything can be switched off, and seven themes

## The silly part

- An Obsidian style graph of your project (`alt+g`)
- An AI meter that guesses how AI generated your file is
- Your LAN ip, displayed like it got leaked
- The mogling, a tiny face that has feelings about your errors
- A typing combo counter with sparks flying out of the cursor
- Critters that walk through the blank parts of your code
- Matrix rain when you go idle
- Sound effects and chiptune music that gets tense when the build is broken

## Keys worth knowing

| keys | what |
| --- | --- |
| `ctrl+p` | find a file |
| `ctrl+shift+p` / `f1` | command palette |
| `ctrl+k` | every key binding |
| `ctrl+,` | settings |
| `ctrl+b` | file explorer |
| `ctrl+l` | AI chat |
| `alt+g` | project graph |
| `ctrl+\` | split |
| `ctrl+d` | add a cursor on the next match |

Run `mog --keys` to print them all.

## Building

You need a recent stable Rust toolchain and a C compiler for the tree-sitter grammars.

```sh
cargo build --release
./target/release/mog [file or folder]
```

See [examples/config.toml](examples/config.toml) for the config and [ROADMAP.md](ROADMAP.md)
for where things are at.

## License

Apache 2.0. See [LICENSE](LICENSE).
