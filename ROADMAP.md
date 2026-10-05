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
- [ ] Kitty keyboard protocol with a fallback for older terminals
- [ ] Mark keys the terminal can't send in the key list
- [ ] Restore open tabs, splits and cursors between sessions
- [ ] Swap files to recover unsaved buffers after a crash
- [ ] Persistent undo history
- [ ] Run and build tasks with output in the problems list
- [ ] Debugger (DAP)

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
- [ ] Stage and unstage hunks
- [ ] Diff view
- [ ] Commit from the editor

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
- [ ] Inlay hints
- [ ] Signature help
- [ ] Document and workspace symbols (outline and go to symbol)
- [ ] Semantic tokens
- [ ] Suggest installing a language server that isn't on `PATH`

## AI

- [x] AI ghost text (Claude)
- [x] Copilot ghost text
- [x] Copilot sign in and status
- [x] Accept ghost text by word and cycle suggestions
- [x] Turn chat and ghost text on or off for each AI
- [ ] Copilot chat
- [x] Claude chat panel
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

- [ ] Homebrew
- [ ] winget
- [ ] AUR package
- [ ] `cargo-binstall` metadata

## Quality

- [ ] Snapshot tests for `mog-tui` with `TestBackend` and `insta`
- [ ] Scripted event loop tests (open, edit, save, quit)
- [ ] Benchmark a 100 MB file and a very long single line
- [ ] Make sure tree-sitter parsing is incremental and off the UI thread
- [ ] Profile idle CPU use of audio, flair ticks and the resource monitor

## Extensions

- [ ] Plugin API
