"""An example mog plugin built on the Python SDK, using protocol 2.

It marks TODO and FIXME notes with diagnostics and a note after the line, counts them in the
status line, lists them, offers a code action to remove one, completes the keywords, explains
them on hover and trims trailing spaces before every save.

Install it by copying this folder into the plugins folder of your mog config, or point a config
entry at it:

    mog plugin install /path/to/mog/examples/plugins/todo

See docs/plugins.md for the protocol.
"""

import re

from mog_plugin import Plugin, change, edit, open_file, status

NOTE = re.compile(r"\b(TODO|FIXME)\b:?\s*(.*)")

plugin = Plugin()


def notes(text):
    """Returns (line, start offset, kind, message) for every note in text."""
    found = []
    offset = 0
    for number, line in enumerate(text.splitlines(keepends=True)):
        match = NOTE.search(line)
        if match:
            found.append((number, offset + match.start(), match.group(1), match.group(2).strip()))
        offset += len(line)
    return found


def text_of(path):
    """Returns the text of an open file and its version."""
    answer = plugin.ask("editor/text", {"path": path})
    return answer["text"], answer.get("version")


def mark(path):
    """Updates the diagnostics, decorations and count for the file at path."""
    if not path:
        return
    text, _ = text_of(path)
    found = notes(text)
    plugin.diagnostics(
        path,
        [
            {
                "start": start,
                "end": start + len(kind),
                "severity": "warning" if kind == "FIXME" else "info",
                "message": message or kind.lower(),
            }
            for _, start, kind, message in found
        ],
    )
    plugin.decorations(
        path,
        [{"line": line, "text": f"⚑ {kind.lower()}", "color": "yellow"} for line, _, kind, _ in found],
    )
    count = len(found)
    plugin.segment(
        f"{count} todo{'s' if count != 1 else ''}" if count else "",
        color="yellow",
        command="plugin.todo.list",
    )


@plugin.on("opened")
def opened(event):
    mark(event.get("path"))


@plugin.on("changed")
def changed(event):
    mark(event.get("path"))


@plugin.on("before_save")
def before_save(event):
    changes = []
    offset = 0
    for line in event["text"].splitlines(keepends=True):
        body = line.rstrip("\r\n")
        trimmed = body.rstrip(" \t")
        if len(trimmed) != len(body):
            changes.append(change(offset + len(trimmed), offset + len(body), ""))
        offset += len(line)
    return changes


@plugin.command("list", title="Todo: List the notes in this file", keys=["alt+shift+t"], menu=True)
def list_notes(context, args):
    found = notes(context.get("text") or "")
    if not found:
        return [status("no notes here, nice")]
    picked = plugin.ask(
        "ui/pick",
        {
            "title": "notes",
            "items": [
                {"label": message or kind, "detail": kind, "hint": f"line {line + 1}"}
                for line, _, kind, message in found
            ],
        },
    )
    if picked is None:
        return []
    line = found[picked["index"]][0]
    return [open_file(context["path"], line=line)]


@plugin.provide("code_actions")
def code_actions(params):
    path = params.get("path")
    text, version = text_of(path)
    lines = text.splitlines(keepends=True)
    line = params["line"]
    if line >= len(lines) or not NOTE.search(lines[line]):
        return {"actions": []}
    start = sum(len(before) for before in lines[:line])
    return {
        "actions": [
            {
                "title": "Remove this note",
                "actions": [edit([change(start, start + len(lines[line]), "")], path, version)],
            }
        ]
    }


@plugin.provide("completion")
def completion(params):
    prefix = params.get("prefix", "")
    items = [
        {"label": word, "insert": word + ": ", "detail": "a note", "kind": "keyword"}
        for word in ("TODO", "FIXME")
        if prefix and word.startswith(prefix.upper())
    ]
    return {"items": items}


@plugin.provide("hover")
def hover(params):
    text, _ = text_of(params.get("path"))
    lines = text.splitlines()
    line = params["line"]
    if line < len(lines) and NOTE.search(lines[line]):
        return {"text": "a note to come back to, list them with alt+shift+t"}
    return {}


if __name__ == "__main__":
    plugin.run()
