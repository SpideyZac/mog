# Writing a mog plugin

A mog plugin is any program that reads and writes JSON-RPC 2.0 messages on stdin and stdout,
framed like the language server protocol:

```
Content-Length: 52\r\n
\r\n
{"jsonrpc":"2.0","id":1,"method":"initialize",...}
```

Plugins run as their own processes, so they can be written in anything, and a plugin that
crashes, hangs or floods mog with messages never takes the editor down. This page describes
protocol version 2. Plugins written for version 1 keep working unchanged.

## Plugins are not sandboxed

> **A plugin is a program running as you, with nothing holding it back.** mog does not sandbox
> plugins or limit what they do. Installing one is the same as running a script someone sent
> you.

Any plugin, including one that only claims to count words, can:

- read, change and delete **any file your user account can**, not just the project, like your
  SSH keys, browser profiles and other projects
- read **every environment variable** mog was started with, which often holds API keys and
  tokens like `ANTHROPIC_API_KEY`, `GITHUB_TOKEN` or cloud credentials
- see the full text of every open file and every `changed` edit, including secret files that
  `ai.exclude` keeps away from the AI, and every key it captures
- use the network, start other programs, and keep doing all of this in the background for as
  long as mog runs

The protocol on this page only limits what a plugin can ask *mog* to do. Nothing limits what the
program does by itself. So:

- Read a plugin's code before installing it, and only install plugins from people you trust.
- Pin a plugin to a version you read, see [Installing and turning plugins on](#installing-and-turning-plugins-on),
  since an update can change what it does.
- A project can never add a plugin, only your own config can, so opening a repository never runs
  one.
- `mog plugin install` says all of this and asks before it installs anything.

## The quick way

```sh
mog plugin new hello                    # a python plugin, or --language node
mog plugin doctor                       # starts every plugin and shows what it offers
mog plugin test hello                   # runs its tests against a fake editor
```

`mog plugin new` makes a folder in the plugins folder of your config with a `plugin.toml` and a
main file. Restart mog or run `Plugins: Restart all plugins`, then run
`hello: Say hello` from the command palette.

The SDKs do the framing and dispatch so a plugin is just its handlers:

| language | SDK | example |
| --- | --- | --- |
| Python 3.8+ | [`sdk/python/mog_plugin.py`](../sdk/python/mog_plugin.py), one file, no dependencies | [`examples/plugins/todo`](../examples/plugins/todo) |
| Node 18+ | [`sdk/node/mog-plugin.js`](../sdk/node/mog-plugin.js), one file, no dependencies | `mog plugin new x --language node` |
| Rust | the `mog-plugin-sdk` crate in this repository | its crate docs |
| anything else | read on, it is a hundred lines | [`examples/plugins/words.py`](../examples/plugins/words.py) with no SDK |

The Python and Node SDKs ship inside mog. mog writes the ones that match it to a folder it names
in `MOG_SDK_DIR` and adds that to `PYTHONPATH` and `NODE_PATH`, so `from mog_plugin import ...`
and `require("mog-plugin")` work with nothing next to the plugin. A copy of the SDK in the plugin
folder still wins, to pin an older one.

Two bigger examples show how far a plugin can go:
[`examples/plugins/vim`](../examples/plugins/vim) takes over the keyboard to add vim motions, and
[`examples/plugins/aquarium`](../examples/plugins/aquarium) fills the editor with fish as flair.

```python
from mog_plugin import Plugin, status

plugin = Plugin()

@plugin.command("count", title="Words: Count", keys=["alt+shift+w"])
def count(context, args):
    return [status(f"{len((context['text'] or '').split())} words")]

@plugin.on("saved")
def saved(event):
    plugin.log(f"saved {event['path']}")

plugin.run()
```

## Installing and turning plugins on

Plugins live in your global config only. A project can never add one, since a plugin runs code
with your permissions and no sandbox, see [Plugins are not sandboxed](#plugins-are-not-sandboxed).
Only install plugins you trust.

There are two ways to have one:

1. **A plugin folder** with a `plugin.toml`, in the `plugins` folder next to your config
   (`~/.config/mog/plugins` on Linux, see `mog plugin list` for yours). mog finds these on its
   own. `mog plugin install <folder or git url>` copies one there and `mog plugin remove
   <name>` deletes it.
2. **A config entry** that names a program:

```toml
[plugins.words]
command = "python3"
args = ["/path/to/words.py"]
```

A config entry with the same name as a plugin folder changes it:

```toml
[plugins.todo]
enabled = false                 # turn it off
command = "python"              # run another program, keeping the manifest args
path = "/somewhere/else/todo"   # use a plugin folder that is not in the plugins folder
timeout = 60                    # seconds a command may take, 30 by default
settings = { loud = true }      # handed to the plugin in initialize
```

`Plugins: Show plugins and their logs` (`plugins.log`) shows each plugin, whether it runs and the
last 500 lines it printed to stderr. Print debugging output to stderr, never to stdout.

## The manifest

`plugin.toml` says what a plugin is before it runs, so mog can list its commands right away and
start it only when it is needed:

```toml
name = "todo"                     # namespaces its commands as plugin.todo.<command>
version = "1.0.0"
description = "Marks TODO and FIXME notes."
protocol = 2
command = "python3"
command_windows = "python"        # optional, the program on windows
args = ["${plugin_dir}/todo.py"]  # ${plugin_dir} is this folder
activation = ["language:md", "command"]
timeout = 10

[[commands]]
name = "list"
title = "Todo: List the notes in this file"
keys = ["alt+shift+t"]
menu = true                       # also in the editor's right click menu
```

`activation` says when the plugin starts:

| activation | starts the plugin |
| --- | --- |
| `startup` | when mog starts, also what an empty list means |
| `command` | the first time one of its commands runs, which waits for it |
| `language:<extension>` | the first time a file with that extension gets focus |

The plugin gets `MOG_PLUGIN_DIR`, `MOG_VERSION` and `MOG_SDK_DIR` in its environment, with the
SDKs on `PYTHONPATH` and `NODE_PATH`, and runs in the project folder.

## Testing plugins

`mog plugin test` runs a plugin against a fake editor, so it can be tested without starting mog:

```sh
mog plugin test todo                       # every .toml in the plugin's tests folder
mog plugin test ./my-plugin tests/a.toml   # a plugin folder, and only this script
```

The fake editor keeps files in memory and answers everything a plugin asks the way mog does:
`editor/text` and the rest, `ui/pick` and `ui/prompt` with answers the test gives, and actions
like `edit` and `select` change its files. A test script lists files to open and then steps,
each doing one thing and saying what should happen. File paths are relative to the script.

```toml
settings = { loud = true }   # handed to the plugin instead of the config's

[[files]]
path = "notes.txt"
text = "first
TODO: water the mog
"   # read from disk when left out

[[steps]]
name = "marks the note when the file opens"
open = "notes.txt"
expect.notifications = [{ method = "segment", params = { text = "1 todo" } }]

[[steps]]
pick = 0                     # the answer to the next ui/pick, false closes the list
command = "list"
expect.result = { actions = [{ type = "open", line = 1 }] }
```

A step first sets things up, then does at most one thing:

| key | does |
| --- | --- |
| `open` | Opens or focuses a file, with `text` or from disk, sending `opened` |
| `text` | Replaces the text of the focused file, sending `changed`, or the text for `open` |
| `select` | Selects `[anchor, head]` |
| `pick`, `prompt` | Answers the next `ui/pick` with an index or `ui/prompt` with text, `false` closes it |
| `command` | Runs a command of the plugin with `args` and does its actions |
| `event` | Sends an event with `params`, if the plugin listens for it (`click` and `timer` always go) |
| `provide` | Asks for a feature like `completion` at the cursor, with extra `params` |
| `request` | Sends any request with `params` |
| `keys` | Sends keys like `["j", "shift+g"]` as `key` requests and does the actions |
| `before_save` | Asks the plugin to tidy the focused file, applies its changes and saves |
| `wait` | Milliseconds to wait for what is expected, 2000 by default |

and `expect` says what should happen. Each is checked once the step is done, waiting up to
`wait` for the plugin to get there:

| expect | passes when |
| --- | --- |
| `result` | The answer has at least this: extra keys are fine, and list items must appear in order with others allowed in between |
| `error` | The step failed with an error containing this |
| `status` | The status line contains this |
| `text` | The focused file holds exactly this |
| `selection` | The main selection is `[anchor, head]` |
| `notifications`, `requests` | The plugin sent these during the step, in order, each `{ method, params }` with `params` matched like `result` |
| `log` | Something the plugin printed or logged during the step contains this |
| `commands` | The plugin asked mog to run these commands during the step |
| `saved` | The plugin saved a file during the step, or did not for `false` |
| `output` | The output panel contains this |

It prints each step and what went wrong, and fails if any step did.
[`examples/plugins/todo/tests`](../examples/plugins/todo/tests) tests the todo example.

Rust tests can use the fake editor directly: the `mog-plugin-test` crate has `FakeHost`, which
starts a plugin or talks to one over in-memory pipes, and `run_script`. A test can open files,
run commands, send events and keys, and look at everything the plugin did.

## Starting up

mog sends `initialize` first:

```json
{
  "protocolVersion": 2,
  "mogVersion": "1.0.0",
  "root": "/home/me/project",
  "settings": { "loud": true },
  "capabilities": {
    "events": ["opened", "closed", "saved", "changed", "..."],
    "requests": ["editor/context", "editor/text", "..."],
    "notifications": ["status", "draw", "capture", "..."],
    "actions": ["status", "edit", "..."],
    "providers": ["completion", "hover", "formatting", "code_actions"]
  }
}
```

`capabilities` lists everything this mog understands, so a plugin can check before relying on
something newer. Answer within 10 seconds with what the plugin offers:

```json
{
  "protocolVersion": 2,
  "commands": [
    { "name": "count", "title": "Words: Count words", "keys": ["alt+shift+w"], "menu": false }
  ],
  "events": ["saved", "changed"],
  "providers": { "completion": ["md"], "hover": true }
}
```

- `protocolVersion` is the version the plugin speaks, 1 when left out. mog speaks 1 and 2.
- `commands` show up in the palette and the key list as `plugin.<plugin>.<name>`, like
  `plugin.words.count`, so users can bind them in `[keys]`. `keys` are only a suggestion: they
  are bound when nothing else uses them and the user did not bind them. Names cannot have
  spaces or colons.
- `events` are the [events](#hearing-about-things) to send. Leaving it out means `opened` and
  `saved`, as in protocol 1.
- `providers` are the [features](#providing-features) the plugin adds, each for a list of file
  extensions or `true` for every file.

## Running a command

When the user runs a command, mog sends the request `command`:

```json
{
  "command": "count",
  "args": null,
  "context": {
    "root": "/home/me/project",
    "path": "/home/me/project/notes.md",
    "language": "md",
    "text": "the whole file",
    "length": 14,
    "version": 7,
    "selection": { "anchor": 0, "head": 5 },
    "selections": [{ "anchor": 0, "head": 5 }],
    "line": 0,
    "column": 5,
    "modified": false
  }
}
```

`path` and `language` are `null` for an untitled file, and `text` is `null` for files over 8 MB,
read those in parts with `editor/text`. Offsets count chars (Unicode scalar values), not bytes,
and lines and columns start at 0. `version` goes up on every edit. `selections` has the main
selection first, then any other cursors.

`args` is what an action or key binding passed. A binding like
`"alt+1" = "plugin.snippets.insert:greeting"` passes the text after the colon, `"greeting"`.

Answer with what mog should do, or with a JSON-RPC error whose `message` is shown to the user:

```json
{ "actions": [ { "type": "status", "text": "3 words" } ] }
```

A command has 30 seconds to answer, or what its manifest or config says. After that mog tells
the user, sends `$/cancelRequest` with the request `id`, and drops any late answer.

## Actions

| action | does |
| --- | --- |
| `{ "type": "status", "text": "..." }` | Shows a message in the status line |
| `{ "type": "notify", "text": "...", "level": "warning" }` | Shows a message marked `info`, `warning` or `error` |
| `{ "type": "insert", "text": "..." }` | Replaces the selection, or types at the cursor |
| `{ "type": "edit", "changes": [...], "path": "...", "version": 7 }` | Replaces char ranges in one undo step, see below |
| `{ "type": "workspace_edit", "edits": [{ "path": "...", "version": 7, "changes": [...] }] }` | Edits several files, all of them or none |
| `{ "type": "open", "path": "...", "line": 3, "column": 0 }` | Opens a file, relative to the project, at a line |
| `{ "type": "select", "selections": [{ "anchor": 0, "head": 5 }], "path": "..." }` | Sets the selections, the first being the main cursor |
| `{ "type": "save", "path": "..." }` | Saves a file, the focused one when `path` is left out |
| `{ "type": "command", "name": "save", "args": null }` | Runs any mog command by name, see `mog --keys`, with `args` for plugin commands |
| `{ "type": "output", "title": "...", "text": "..." }` | Shows text in the output panel |

An edit's `changes` are `{ "start": 0, "end": 5, "text": "..." }` in char offsets of the text as
it is before the edit. They must not overlap or run past the end. `path` picks a file, the
focused one when left out. With `version`, mog refuses the edit if the file changed since that
version, so an edit worked out from old text never lands in the wrong place.

Open files change in the editor, one undo step each. Files that are not open are changed and
saved on disk. A `workspace_edit` checks every part first and changes nothing if one does not
fit. Parts for the same file are merged into one edit, so their changes are all in offsets of the
text before the edit and must not overlap each other. Files on disk are written to temp files
first and only replace the real ones once every one of them was written, so a full disk or a
missing folder leaves every file as it was.

Actions run in order and stop at the first one that fails. A list with a broken action, like a
missing field or an unknown type, is refused whole with an error that says which one, so
nothing half happens.

## Talking first

A plugin does not have to wait to be asked. These notifications can be sent at any time:

| method | params | does |
| --- | --- | --- |
| `actions` | `{ "actions": [...] }` | Does the actions |
| `status` | `{ "text": "..." }` | Shows a message in the status line |
| `notify` | `{ "text": "...", "level": "info" }` | Shows a message and keeps it in the plugin log |
| `log` | `{ "text": "..." }` | Adds a line to the plugin log only |
| `segment` | `{ "text": "...", "color": "green", "command": "...", "id": "..." }` | Puts a short text in the status line until replaced, empty removes it. Clicking it runs `command`. Each `id` is its own segment |
| `progress` | `{ "id": "...", "title": "...", "percentage": 40, "done": false }` | Shows progress of long work in the status line, `done` removes it |
| `diagnostics` | `{ "path": "...", "diagnostics": [...] }` | Replaces this plugin's problems for an open file, see below |
| `decorations` | `{ "path": "...", "decorations": [{ "line": 3, "text": "...", "color": "dim" }] }` | Replaces the text this plugin shows after lines of a file, empty removes them |
| `output` | `{ "title": "...", "text": "..." }` | Shows text in the output panel |
| `draw` | a widget, see [Drawing on the screen](#drawing-on-the-screen) | Puts a widget on the screen or replaces the one with the same `id` |
| `clear` | `{ "id": "..." }` | Removes a widget, or every widget of the plugin when `id` is left out |
| `cursor` | `{ "shape": "block", "blink": false }` | Sets the cursor shape, see [The cursor](#the-cursor) |
| `capture` | `{ "keys": "all", "except": ["ctrl+s"] }` | Takes keys before the editor, see [Taking keys](#taking-keys) |
| `timer` | `{ "id": "...", "every": 500 }` | Sends the `timer` event every so many milliseconds, 0 stops it, see [Timers](#timers) |

A diagnostic is `{ "start": 0, "end": 4, "severity": "warning", "message": "..." }` in char
offsets, or `{ "line": 2, "column": 0, "end_line": 2, "end_column": 4, ... }`. Severities are
`error`, `warning`, `info` and `hint`. They show with the language server's in the gutter, the
problems list and error lens.

Colors are theme color names (`bg`, `panel`, `raised`, `select`, `fg`, `dim`, `accent`,
`accent2`, `red`, `orange`, `yellow`, `green`, `cyan`, `blue`, `purple`, `pink`) or hex codes
like `#ff5ccd`.

## Asking mog

A plugin can send requests to mog at any time, even while it is handling a `command`, and gets a
normal JSON-RPC response. mog keeps reading while it waits, so read the answer by its `id` and
put other messages aside for later, like the SDKs do.

| method | params | answers |
| --- | --- | --- |
| `editor/context` | `{}` | The same context a `command` gets |
| `editor/text` | `{ "path": "...", "start": 0, "end": 100 }` | `{ "text", "version", "length" }` of an open file or the chars `start..end` of it, from disk if it is not open (with a `null` version), the focused file when `path` is left out |
| `editor/documents` | `{}` | `{ "documents": [{ "path", "name", "language", "version", "modified", "active" }] }` |
| `editor/diagnostics` | `{ "path": "..." }` | `{ "diagnostics": [{ "start", "end", "line", "column", "severity", "message" }] }` from every source |
| `editor/select` | `{ "path": "...", "selections": [...] }` | `{}` once set, opening the file if needed |
| `editor/save` | `{ "path": "..." }` | `{}` once saved |
| `actions` | `{ "actions": [...] }` | `{}` once the actions are done, or an error saying what failed |
| `ui/pick` | `{ "title": "...", "items": ["a", { "label": "b", "detail": "...", "hint": "..." }] }` | `{ "index": 1, "item": "b" }`, or `null` if the user closed the list |
| `ui/prompt` | `{ "title": "...", "text": "start", "hint": "..." }` | `{ "text": "what was typed" }`, or `null` if the user closed it or left it empty |
| `ui/layout` | `{}` | Where things are on screen, see [Drawing on the screen](#drawing-on-the-screen) |

Only one `ui/pick` or `ui/prompt` can be open at a time, and none while another popup is open. A
second one gets an error, try again later. If the plugin stops while one is open, mog closes it.

## Hearing about things

mog sends the notification `event` for each event the plugin listed in `events`:

```json
{ "kind": "saved", "path": "/home/me/project/notes.md", "language": "md" }
```

| kind | when | params |
| --- | --- | --- |
| `opened` | A different file got focus | `path` (`null` for an untitled one), `language` |
| `closed` | A file was closed | `path` |
| `saved` | A file was saved | `path`, `language` |
| `changed` | Typing paused after a file changed | `path`, `version`, and `changes` to apply in order since the last `changed` for that file, or `text` when mog cannot say (the first time, or after many edits). Files over 8 MB never get `text`, read them with `editor/text` |
| `selection` | The cursor rested somewhere new | `path`, `selections` |
| `diagnostics` | A language server sent new problems for a file | `path`, `errors`, `warnings` |
| `config_changed` | The config was reloaded or changed in the settings menu | |
| `idle` | Nobody touched mog for a while | |
| `task_finished` | A task like build or test finished | `name`, `code`, `success` |
| `git_changed` | The git status or branch changed | `branch` |
| `before_save` | A file is about to be saved, see below | `path`, `language`, `version`, `text` |

Two more events come without asking for them in `events`, since a plugin only gets them for
things it set up itself: `click` when the user clicks one of its
[widgets](#drawing-on-the-screen), with the widget `id`, the `x` and `y` of the click inside it
and the `button` (`left`, `right` or `middle`), and `timer` with the `id` of one of its
[timers](#timers).

`before_save` is a request, not a notification. Answer with `{ "changes": [...] }` in char
offsets of the `text` it sent, like a formatter would, or `{ "changes": [] }`. mog waits up to 2
seconds for every plugin that listens, applies all their changes together as one undo step, then
saves. Changes from different plugins must not overlap. If the user keeps typing meanwhile, the
changes are dropped and the file is saved as it is.

## Providing features

A plugin that lists a provider gets requests for it, for files with the listed extensions. mog
asks every plugin that provides a feature at once, waits up to 1.5 seconds and merges what came
in with the language server's answer. Every request has `path`, `language`, `version`, `offset`,
`line` and `column` for the cursor.

| provider | request | params | answer |
| --- | --- | --- | --- |
| `completion` | `provide/completion` | `prefix` (the word before the cursor), `manual` | `{ "items": [{ "label", "insert", "detail", "kind", "filter" }] }`. `kind` is one of `function`, `variable`, `field`, `type`, `module`, `keyword`, `constant` |
| `hover` | `provide/hover` | `selection` | `{ "text": "..." }`, shown under the language server's |
| `formatting` | `provide/formatting` | `text`, `tab_size`, `insert_spaces` | `{ "changes": [...] }` against `text`. A formatter plugin goes before the language server |
| `code_actions` | `provide/code_actions` | `selection` | `{ "actions": [{ "title": "...", "actions": [...] }] }`, listed with the language server's code actions. Picking one does its actions |

Completion works in files with no language server at all, so a plugin can add completion for
any kind of file.

## Drawing on the screen

A plugin can put widgets anywhere on the screen: a box of styled text that can animate, drift on
its own and be clicked. Flair, little critters, a mode indicator or a which-key popup are all
widgets. The notification `draw` puts one up, or replaces the one with the same `id`:

```json
{
  "id": "fish",
  "anchor": "top_left",
  "x": 10,
  "y": 3,
  "frames": [["><>"], ["><o"]],
  "fps": 2,
  "transparent": true,
  "clickable": true,
  "motion": { "dx": 3, "dy": 0, "edge": "wrap" }
}
```

| field | default | means |
| --- | --- | --- |
| `id` | `widget` | Which widget of this plugin it is |
| `lines` | | The rows, each a string or a list of spans like `{ "text": "hi", "fg": "red", "bg": "panel", "bold": true, "italic": false, "underline": false }` |
| `frames` | | A list of `lines` to cycle through instead, at `fps` frames a second |
| `anchor` | `top_left` | What `x` and `y` count from: `top_left`, `top_right`, `bottom_left` and `bottom_right` of the editor area, counting inward from that corner, `center` of the editor area, `cursor` for the text cursor, or `screen` for the top left of the whole screen |
| `x`, `y` | 0 | Cells from the anchor, can be negative |
| `flair` | `true` | Decoration, hidden with the rest of the flair. Set it to `false` for widgets that are part of how the plugin works, like a mode indicator |
| `transparent` | `false` | Spaces without a background let what is underneath show through |
| `clickable` | `false` | Clicks on it go to the plugin as the `click` event instead of to what is underneath |
| `fg`, `bg` | | The default text color, and a background that fills the whole box |
| `z` | 0 | Widgets with a higher `z` are drawn on top |
| `motion` | | Drifts `dx` and `dy` cells a second, and at the edge of the editor area (the screen for `screen` widgets) either bounces back (`bounce`) or comes in on the other side (`wrap`) |
| `restart` | `false` | Starts the animation and motion over. Otherwise they keep going when a widget is replaced, so moving or changing one does not make it jump |

A `draw` with no `lines` or `frames` removes the widget, and so does `clear`. A plugin can have 64
widgets, each up to 400 cells wide and tall with up to 64 frames. Widgets anchored to the editor
area are cut off at its edges and ones anchored to the cursor hide while the cursor is off screen.
mog draws them above the text and the built in flair and below popups like the palette.

mog animates and moves widgets itself, so a plugin only sends a widget again when it changes.
Flair widgets follow the user's flair settings: they hide in serious mode and when flair is off,
`flair.disabled = ["plugin.<name>"]` turns off one plugin's flair (it is listed in the settings
menu once it drew something), `"plugins"` turns off all of it, and reduced motion holds
animations on their first frame and hides widgets that drift.

The request `ui/layout` answers where things are, to place widgets with:

```json
{
  "screen": { "x": 0, "y": 0, "width": 120, "height": 40 },
  "editor": { "x": 31, "y": 1, "width": 89, "height": 38 },
  "split": null,
  "status": { "x": 0, "y": 39, "width": 120, "height": 1 },
  "cursor": { "x": 36, "y": 4 },
  "flair": true,
  "reduced_motion": false,
  "theme": "mog"
}
```

## Taking keys

A plugin can take keys before the editor does, which is enough to build modal editing like vim
on top of mog. The notification `capture` says which:

| `capture` params | takes |
| --- | --- |
| `{ "keys": "all" }` | Every key |
| `{ "keys": "all", "except": ["ctrl+s", "ctrl+q"] }` | Every key but these |
| `{ "keys": ["esc", "ctrl+space"] }` | Only these |
| `{ "keys": null }` | Nothing, the editor gets every key again |

Keys are named like in `[keys]`: `j`, `shift+g`, `ctrl+r`, `esc`, `enter`, `space`. Keys are only
taken while the text has focus and no popup, prompt or drawing is open, so the palette, the
explorer, the terminal and search keep working.

For each key it takes, mog sends the request `key` with the key and where the cursor is (the
[context](#running-a-command) without `text`, plus `lines`, the number of lines):

```json
{ "key": "shift+g", "char": "G", "path": "...", "version": 7, "selection": { "anchor": 4, "head": 4 }, "...": "..." }
```

`char` is the char the key types, or `null` for keys like `esc` or `ctrl+r`. Answer with what to
do:

```json
{ "actions": [{ "type": "select", "selections": [{ "anchor": 9, "head": 9 }] }], "capture": { "keys": ["esc"] } }
```

- `actions` are [actions](#actions), like `select`, `edit` or `command` with any mog command
  such as `move_word_right` or `undo`.
- `{ "handled": false }` hands the key to the editor as if the plugin never took it, so a plugin
  can take every key and still let `ctrl+s` through.
- `capture` changes which keys the plugin takes, before the next key is looked at. Change modes
  here rather than with a separate `capture` notification, so a key typed right after `i` already
  goes to the text.

Keys stay in order: while a plugin decides about one, later keys wait, then go to the plugin or
the editor depending on what it answered. A plugin has a second to answer a key, after that the
key is dropped. Ask mog things like `editor/text` while answering, but do not wait on the user
there: run a plugin command instead, with a `command` action, and ask from that command.

## The cursor

The notification `cursor` sets the shape of the text cursor, `default` (what the terminal is set
up to show), `block`, `bar` or `underline`, and whether it `blink`s. It goes back to normal when
the plugin stops and when mog quits.

## Timers

The notification `timer` with an `id` and `every` in milliseconds makes mog send the `timer`
event with that `id` every so often, for things that change on their own. `every` of 0 stops
it. Ticks come at most every 30 milliseconds, a plugin can have 16 timers, and a plugin that is
slow to read skips ticks rather than getting a pile of them. For moving and animated widgets, let
mog do it with `frames` and `motion` instead.

## When things go wrong

- A command that takes too long times out with a message, see [Running a command](#running-a-command).
- A plugin has 10 seconds to answer `initialize` or it is stopped.
- Messages over 64 MB stop the plugin, and only 1024 messages can wait in each direction.
- A plugin that crashes is started again after 1, 2, 4, 8 and 16 seconds. After the fifth crash
  in a row mog gives up until you run `Plugins: Restart all plugins` or one of its commands. A
  plugin that ran for a minute before crashing starts the count over. Commands run while it waits
  to restart wait with it, and opening a file of its language does not start it early.
- Reloading the config restarts plugins whose settings changed and starts new ones.
- Commands run while a plugin is starting wait for it, and fail with a message if it stops.

## Stopping

When mog quits it sends the notification `shutdown` and closes stdin. Exit then. mog kills
plugins that are still running a moment later.

## Versioning

`protocolVersion` goes up only when something a plugin relies on changes. New actions, events,
requests and fields can appear without it changing, so ignore what you do not know, and check
`capabilities` before using something new. A [JSON schema](plugin-protocol.schema.json) of the
messages is next to this page.

What changed in protocol 2:

- `initialize` has `settings` and `capabilities`, and its answer can list `events` and
  `providers`.
- Plugins can have a `plugin.toml` manifest and start lazily.
- A broken action or change is an error instead of being skipped, and edits can carry a
  `version`.
- Requests from mog time out and are cancelled with `$/cancelRequest`.
- `command` has `args`, and the context has `version`, `length` and `selections`.
- New actions: `notify`, `workspace_edit`, `select`, `save`, `output`. New notifications:
  `notify`, `log`, `progress`, `diagnostics`, `decorations`, `output`. New requests:
  `editor/documents`, `editor/diagnostics`, `editor/select`, `editor/save`, and `editor/text`
  can read part of a file.
- New events and `before_save`, and providers for completion, hover, formatting and code actions.

Added later without changing the protocol version, check `capabilities.notifications` and
`capabilities.requests` for them:

- `draw`, `clear`, `ui/layout` and the `click` event for [drawing on the screen](#drawing-on-the-screen).
- `capture` and the `key` request for [taking keys](#taking-keys).
- `cursor` and `timer` with the `timer` event.
