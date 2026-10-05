"""Checks the Python SDK against a scripted mog: python sdk/python/test_mog_plugin.py"""

import io
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from mog_plugin import Plugin, change, status  # noqa: E402


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


plugin.run()
replies = {m["id"]: m for m in unframe(out.getvalue()) if "method" not in m}
hello = replies[0]["result"]
assert hello["protocolVersion"] == 2
assert hello["events"] == ["before_save", "saved"]
assert hello["providers"] == {"hover": ["md"]}
assert plugin.settings == {"a": 1}
assert replies[1]["result"]["actions"] == [status("3 words in a")]
assert replies[2]["result"]["changes"] == [change(0, 1, "")]
assert replies[3]["result"]["text"] == "hi"
assert "no command" in replies[4]["error"]["message"]
assert 5 not in replies, "a cancelled request gets no answer"
assert saved == ["a"]
print("python sdk ok")
