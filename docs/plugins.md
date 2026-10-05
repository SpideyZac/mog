# Writing a mog plugin

A mog plugin is any program that reads and writes JSON-RPC 2.0 messages on stdin and stdout,
framed like the language server protocol:

```
Content-Length: 52\r\n
\r\n
{"jsonrpc":"2.0","id":1,"method":"initialize",...}
```

Plugins run as their own processes, so they can be written in anything, and a plugin that crashes
or hangs never takes mog down. [`examples/plugins/words.py`](../examples/plugins/words.py) is a
complete plugin in about a hundred lines of Python with no dependencies.

## Turning one on

Plugins live in your global config only. A project can never add one, since a plugin runs code.

```toml
[plugins.words]
command = "python"
args = ["/path/to/words.py"]
enabled = true
```

mog starts every enabled plugin when it starts, in the project folder. If one stops, mog says why
in the status line, using the last line it printed to stderr. Print debugging output to stderr,
never to stdout.

## Starting up

mog sends `initialize` first:

```json
{ "protocolVersion": 1, "mogVersion": "0.3.0", "root": "/home/me/project" }
```

Answer with the commands the plugin adds:

```json
{ "commands": [
  { "name": "count", "title": "Words: Count words", "keys": ["alt+shift+w"] }
] }
```

Each command shows up in the command palette and the key list as `plugin.<plugin>.<name>`, like
`plugin.words.count`, so users can bind it in `[keys]`. `keys` are only a suggestion: they are
bound when nothing else uses them and the user did not bind them themselves.

## Running a command

When the user runs a command, mog sends the request `command`:

```json
{
  "command": "count",
  "context": {
    "root": "/home/me/project",
    "path": "/home/me/project/notes.md",
    "language": "md",
    "text": "the whole file",
    "selection": { "anchor": 0, "head": 5 },
    "line": 0,
    "column": 5,
    "modified": false
  }
}
```

`path` and `language` are `null` for an untitled file, and `text` is `null` for files over 8 MB.
Offsets count chars (Unicode scalar values), not bytes, and lines and columns start at 0.

Answer with what mog should do, or with a JSON-RPC error whose `message` is shown to the user:

```json
{ "actions": [ { "type": "status", "text": "3 words" } ] }
```

## Actions

| action | does |
| --- | --- |
| `{ "type": "status", "text": "..." }` | Shows a message in the status line |
| `{ "type": "insert", "text": "..." }` | Replaces the selection, or types at the cursor |
| `{ "type": "edit", "changes": [{ "start": 0, "end": 5, "text": "..." }], "path": "..." }` | Replaces char ranges, in one undo step. `path` picks an open file, the focused one when left out. Changes must not overlap |
| `{ "type": "open", "path": "...", "line": 3 }` | Opens a file, relative to the project, at a line |
| `{ "type": "command", "name": "save" }` | Runs any mog command by name, see `mog --keys` and the command list in the readme |

## Talking first

A plugin does not have to wait to be asked. These notifications can be sent at any time, like
after background work finishes:

| method | params | does |
| --- | --- | --- |
| `actions` | `{ "actions": [...] }` | Does the actions |
| `status` | `{ "text": "..." }` | Shows a message in the status line |
| `segment` | `{ "text": "..." }` | Puts a short text in the status line until replaced, empty removes it |

## Asking mog

A plugin can send requests to mog at any time, even while it is handling a `command`, and gets a
normal JSON-RPC response. mog keeps reading while it waits, so read the answer by its `id` and
put other messages aside for later, like `ask` in the example does.

| method | params | answers |
| --- | --- | --- |
| `editor/context` | `{}` | The same context a `command` gets |
| `editor/text` | `{ "path": "..." }` | `{ "text": "..." }` of an open file, from disk if it is not open, the focused file when `path` is left out |
| `actions` | `{ "actions": [...] }` | `{}` once the actions are done |
| `ui/pick` | `{ "title": "...", "items": ["a", "b"] }` | `{ "index": 1, "item": "b" }`, or `null` if the user closed the list |
| `ui/prompt` | `{ "title": "...", "text": "start", "hint": "..." }` | `{ "text": "what was typed" }`, or `null` if the user closed it or left it empty |

Only one `ui/pick` or `ui/prompt` can be open at a time, and none while another popup is open.
A second one gets an error, try again later.

## Hearing about things

mog sends the notification `event` when something happens:

```json
{ "kind": "opened", "path": "/home/me/project/notes.md" }
```

| kind | when |
| --- | --- |
| `opened` | A different file got focus. `path` is `null` for an untitled one |
| `saved` | A file was saved |

## Stopping

When mog quits it sends the notification `shutdown` and closes stdin. Exit then. mog kills
plugins that are still running a moment later.

## Versioning

`protocolVersion` goes up only when something a plugin relies on changes. New actions, events
and context fields can appear without it changing, so ignore what you do not know.
