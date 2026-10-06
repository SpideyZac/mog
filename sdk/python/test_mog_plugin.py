"""Checks the Python SDK against a scripted mog: python sdk/python/test_mog_plugin.py"""

import io
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from mog_plugin import Plugin, change, command, span, status  # noqa: E402


def frame(message):
    body = json.dumps(dict(message, jsonrpc="2.0")).encode()
    return b"Content-Length: %d\r\n\r\n" % len(body) + body


def unframe(data):
    messages = []
    while data:
        header, _, rest = data.partition(b"\r\n\r\n")
        length = int(header.split(b":")[1])
        messages.append(json.loads(rest[:length]))
        data = rest[length:]
    return messages


script = b"".join(
    frame(message)
    for message in [
        {"id": 0, "method": "initialize", "params": {"root": "/p", "settings": {"a": 1}}},
        {"id": 1, "method": "command", "params": {"command": "count", "context": {"path": "a"}}},
        # the answer to the question the count command asks
        {"id": "ask-1", "result": {"text": "one two three"}},
        {"id": 2, "method": "before_save", "params": {"text": "xy"}},
        {"id": 3, "method": "provide/hover", "params": {}},
        {"id": 4, "method": "command", "params": {"command": "nope", "context": {}}},
        {"method": "$/cancelRequest", "params": {"id": 5}},
        {"id": 5, "method": "provide/hover", "params": {}},
        {"method": "event", "params": {"kind": "saved", "path": "a"}},
        {"id": 6, "method": "key", "params": {"key": "j", "char": "j"}},
        {"id": 7, "method": "key", "params": {"key": "i", "char": "i"}},
        {"id": 8, "method": "key", "params": {"key": "x", "char": "x"}},
        {"method": "event", "params": {"kind": "timer", "id": "blink"}},
        {"id": 9, "method": "tool/call", "params": {"name": "shout", "input": {"text": "hi"}}},
        {"id": 10, "method": "tool/call", "params": {"name": "nope", "input": {}}},
        {"method": "shutdown"},
    ]
)
out = io.BytesIO()
plugin = Plugin(stdin=io.BytesIO(script), stdout=out)
saved = []


@plugin.command("count", title="Count", keys=["alt+c"])
def count(context, args):
    text = plugin.ask("editor/text", {})["text"]
    return [status(f"{len(text.split())} words in {context['path']}")]


@plugin.on("before_save")
def before_save(event):
    return [change(0, 1, "")]


@plugin.on("saved")
def on_saved(event):
    saved.append(event["path"])


@plugin.provide("hover", ["md"])
def hover(params):
    return {"text": "hi"}


@plugin.on_key
def on_key(key, char, context):
    if key == "j":
        return [command("move_down")]
    if key == "i":
        return {"capture": {"keys": ["esc"]}}
    return False


ticks = []


@plugin.tool("shout", "Shouts the text", {"type": "object", "properties": {"text": {"type": "string"}}})
def shout(tool_input):
    return tool_input["text"].upper()



@plugin.every(500, id="blink")
def blink():
    ticks.append(1)
    plugin.draw("eye", [[span("o", fg="red", bold=True), "_o"]], anchor="cursor", y=1)


# sent before mog said hello, so it waits until after the answer
plugin.capture("all", except_keys=["ctrl+s"])

plugin.run()
messages = unframe(out.getvalue())
notes = [m for m in messages if "method" in m]
assert "id" in messages[0], "the hello answer goes first"
assert notes[0] == {
    "jsonrpc": "2.0",
    "method": "timer",
    "params": {"id": "blink", "every": 500},
}
assert notes[1]["params"] == {"keys": "all", "except": ["ctrl+s"]}
assert notes[-1]["method"] == "draw"
assert notes[-1]["params"]["lines"] == [[{"text": "o", "fg": "red", "bold": True}, "_o"]]
assert ticks == [1]
replies = {m["id"]: m for m in unframe(out.getvalue()) if "method" not in m}
hello = replies[0]["result"]
assert hello["protocolVersion"] == 2
assert hello["events"] == ["before_save", "saved"]
assert hello["providers"] == {"hover": ["md"]}
assert hello["tools"][0]["name"] == "shout"
assert hello["tools"][0]["input_schema"]["properties"]["text"]["type"] == "string"
assert plugin.settings == {"a": 1}
assert replies[1]["result"]["actions"] == [status("3 words in a")]
assert replies[2]["result"]["changes"] == [change(0, 1, "")]
assert replies[3]["result"]["text"] == "hi"
assert "no command" in replies[4]["error"]["message"]
assert 5 not in replies, "a cancelled request gets no answer"
assert saved == ["a"]
assert replies[6]["result"] == {"actions": [command("move_down")]}
assert replies[7]["result"] == {"capture": {"keys": ["esc"]}}
assert replies[8]["result"] == {"handled": False}
assert replies[9]["result"] == {"content": "HI"}
assert "no tool" in replies[10]["error"]["message"]
print("python sdk ok")
