# mog

A terminal code editor that mogs other editors.

mog is a little bit of a joke. It has silly stuff on screen for no reason. It is also a real
editor you can get work done in.

- Modeless editing with normal keybindings and full mouse support
- Language servers
- Copilot and Claude
- Lua plugins

It is early. See [ROADMAP.md](ROADMAP.md) for where things are at.

## Building

You need a recent stable Rust toolchain.

```
cargo build --release
./target/release/mog [file]
```

## License

Apache 2.0. See [LICENSE](LICENSE).
