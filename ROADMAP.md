# Roadmap

## Foundation

- [x] Terminal setup, teardown and panic safety
- [x] Documents, selections, undo and redo
- [x] Editor view with line numbers and scrolling
- [x] Status line
- [x] Modeless keymap (save, quit, undo, clipboard, select all)
- [x] Mouse: click, drag select, shift click, double and triple click, wheel, gutter
- [x] Config file
- [x] Flair layer and registry
- [x] LSP client skeleton
- [x] AI provider skeleton

## Everyday editing

- [x] Syntax highlighting (tree-sitter, thirty two languages)
- [x] File tree
- [x] Keyboard focus for the file explorer
- [x] Key to show and hide the file explorer
- [x] Refresh the file explorer when files change on disk
- [x] Fuzzy file finder
- [x] Command palette
- [x] Key binding list (`ctrl+k` and `mog --keys`)
- [x] Change key bindings from the key list
- [x] Search and replace
- [x] Find and replace across the project
- [x] Go to line
- [x] Tabs
- [x] Auto indent and bracket pairs
- [x] Toggle comment, duplicate, move and delete lines, indent and outdent
- [x] Save as and new files
- [x] Problems list
- [x] Create, rename and delete files from the explorer
- [x] Right click menu
- [x] Multiple cursors
- [x] Splits
- [x] Built in terminal (`ctrl+backtick`)
- [x] Scroll back and restart the terminal
- [x] Kitty keyboard protocol with a fallback for older terminals
- [x] Mark keys the terminal can't send in the key list
- [x] Restore open tabs, splits and cursors between sessions
- [x] Swap files to recover unsaved buffers after a crash
- [x] Persistent undo history
- [x] Run and build tasks with output in the problems list
- [x] Debugger (DAP)

## Look and feel

- [x] Seven themes built from palettes
- [x] Custom themes and a theme editor
- [x] See through backgrounds with ui.opacity
- [x] Settings menu that saves to the config
- [x] Open and reload the config file from the editor
- [x] Settings for each language server
- [x] Minimap
- [x] Indent guides, rainbow brackets and matching bracket
- [x] Error lens
- [x] File icons
- [x] Serious mode preset that turns off all flair
- [x] Reduced motion setting for sparks, combo counter and matrix rain
- [x] Respect `NO_COLOR`

## Git

- [x] Changed line markers in the gutter
- [x] Branch in the status line
- [x] File status colors in the explorer
- [x] Inline blame for the cursor line
- [x] Stage and unstage hunks
- [x] Diff view
- [x] Commit from the editor

## Language servers

- [x] Diagnostics
- [x] Completion
- [x] Hover
- [x] Go to definition
- [x] Find references
- [x] Rename
- [x] Formatting
- [x] Code actions
- [x] Per project language server settings
- [x] Inlay hints
- [x] Signature help
- [x] Document and workspace symbols (outline and go to symbol)
- [x] Semantic tokens
- [x] Suggest installing a language server that isn't on `PATH`

## AI

- [x] Copilot ghost text
- [x] Copilot sign in and status
- [x] Accept ghost text by word and cycle suggestions
- [x] Turn chat and ghost text on or off for each AI
- [x] Claude chat panel
- [x] Stream chat answers, retry a busy API and keep long chats in budget
- [x] Keep secret files like `.env` away from every AI (`ai.exclude`)
- [x] Ask Claude about the selection

## Flair

- [x] Graph view of the project
- [x] Critters that wander around the screen
- [x] Typing combo counter and cursor sparks
- [x] Startup splash
- [x] Keep flair from covering the file explorer
- [x] AI meter
- [x] Leaked ip
- [x] The mogling, quips and a session timer
- [x] Matrix screensaver
- [x] Sound effects and music that gets tense when the build breaks
- [x] Spectrum bars for all desktop audio
- [x] Aura points, a RAM download and an FBI agent counter
- [x] Draw on the screen
- [x] Flair in the tab bar and under the explorer
- [x] $MOG stonks, codebase weather, a resource monitor and stream chat

## Social

- [x] Discord Rich Presence

## Updates

- [x] Auto update from GitHub releases
- [x] Release notes popup
- [x] Signed releases with a public key pinned in the binary

## Distribution

- [x] Homebrew
- [x] winget
- [x] AUR package
- [x] `cargo-binstall` metadata
- [x] Guide to releasing and publishing every package

## Quality

- [x] Snapshot tests for `mog-tui` with `TestBackend` and `insta`
- [x] Scripted event loop tests (open, edit, save, quit)
- [x] Benchmark a 100 MB file and a very long single line
- [x] Tree-sitter parsing off the UI thread
- [x] Incremental tree-sitter parsing and highlighting of only what changed
- [x] Profile idle CPU use of audio, flair ticks and the resource monitor
- [x] Property tests for transactions and undo
- [x] Crash safe state writes off the UI thread
- [x] CI checks the minimum Rust version, licenses and advisories, and releases wait for it

## Extensions

- [x] Plugin API, programs in any language over JSON-RPC
- [x] Plugin protocol 2: manifests and lazy start, events, before save, an editor API, status
  segments, decorations and diagnostics
- [x] Plugins provide completion, hover, formatting and code actions
- [x] Plugins provide go to definition, references and symbols, merged with language servers
- [x] Plugin timeouts, cancellation, crash restarts and logs
- [x] Plugin SDKs for Python, Node and Rust
- [x] `mog plugin` to make, install, list, check and remove plugins
- [x] Plugins draw widgets and flair on the screen, with animation, motion and clicks
- [x] Plugins take keys before the editor, enough for vim motions as a plugin
- [x] Plugins set the cursor shape and run timers
- [x] `mog plugin test` runs plugins against a fake editor with test scripts
- [x] Plugin answer times and memory in the plugin log and the resource monitor, a slow plugin
  warning, and stopping plugins that keep timing out
- [x] Install plugins from git at a version or from signed archives, `mog plugin outdated` and
  `mog plugin update`
