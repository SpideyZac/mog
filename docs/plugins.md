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

## The quick way

```sh
mog plugin new hello                    # a python plugin, or --language node
mog plugin doctor                       # starts every plugin and shows what it offers
```

`mog plugin new` makes a folder in the plugins folder of your config with a `plugin.toml`, a
main file and a copy of the SDK. Restart mog or run `Plugins: Restart all plugins`, then run
`hello: Say hello` from the command palette.

The SDKs do the framing and dispatch so a plugin is just its handlers:

| language | SDK | example |
| --- | --- | --- |
| Python 3.8+ | [`sdk/python/mog_plugin.py`](../sdk/python/mog_plugin.py), one file, no dependencies | [`examples/plugins/todo`](../examples/plugins/todo) |
| Node 18+ | [`sdk/node/mog-plugin.js`](../sdk/node/mog-plugin.js), one file, no dependencies | `mog plugin new x --language node` |
| Rust | the `mog-plugin-sdk` crate in this repository | its crate docs |
| anything else | read on, it is a hundred lines | [`examples/plugins/words.py`](../examples/plugins/words.py) with no SDK |

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
with your permissions. Only install plugins you trust.

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

The plugin gets `MOG_PLUGIN_DIR` and `MOG_VERSION` in its environment and runs in the project
folder.

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
fit.

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

## When things go wrong

- A command that takes too long times out with a message, see [Running a command](#running-a-command).
- A plugin has 10 seconds to answer `initialize` or it is stopped.
- Messages over 64 MB stop the plugin, and only 1024 messages can wait in each direction.
- A plugin that crashes is started again after 1, 2, 4, 8 and 16 seconds. After the fifth crash
  in a row mog gives up until you run `Plugins: Restart all plugins`. A plugin that ran for a
  minute before crashing starts the count over.
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
