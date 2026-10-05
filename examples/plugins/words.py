"""An example mog plugin: counts words, shouts the selection and stamps the date.

Add it to your config:

    [plugins.words]
    command = "python"
    args = ["/path/to/mog/examples/plugins/words.py"]

It only uses the standard library. See docs/plugins.md for the protocol.
"""

import datetime
import json
import sys


def read():
    """Reads one framed message from mog, or None when mog closes the pipe."""
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            if length is not None:
                break
            continue
        name, _, value = line.decode().partition(":")
        if name.strip().lower() == "content-length":
            length = int(value.strip())
    return json.loads(sys.stdin.buffer.read(length))


def write(message):
    """Sends one framed message to mog."""
    body = json.dumps(dict(message, jsonrpc="2.0")).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()


def word_count(path):
    """Counts the words in the file at path."""
    try:
        with open(path, encoding="utf-8") as file:
            return len(file.read().split())
    except OSError:
        return None


def run(command, context):
    """Runs a command and returns the actions for mog."""
    text = context.get("text") or ""
    if command == "count":
        return [{"type": "status", "text": f"{len(text.split())} words, nice"}]
    if command == "shout":
        selection = context["selection"]
        start, end = sorted((selection["anchor"], selection["head"]))
        if start == end:
            return [{"type": "status", "text": "select something to shout first"}]
        return [{"type": "edit", "changes": [
            {"start": start, "end": end, "text": text[start:end].upper()},
        ]}]
    if command == "date":
        return [{"type": "insert", "text": datetime.date.today().isoformat()}]
    raise ValueError(f"no command called {command}")


def main():
    """Answers mog until it goes away."""
    while (message := read()) is not None:
        method = message.get("method")
        params = message.get("params") or {}
        if method == "initialize":
            write({"id": message["id"], "result": {"commands": [
                {"name": "count", "title": "Words: Count words", "keys": ["alt+shift+w"]},
                {"name": "shout", "title": "Words: SHOUT the selection"},
                {"name": "date", "title": "Words: Insert today's date"},
            ]}})
        elif method == "command":
            try:
                actions = run(params["command"], params["context"])
                write({"id": message["id"], "result": {"actions": actions}})
            except Exception as error:  # noqa: BLE001, a plugin should answer, not crash
                write({"id": message["id"], "error": {"code": -32000, "message": str(error)}})
        elif method == "event" and params.get("path"):
            count = word_count(params["path"])
            if count is not None:
                write({"method": "segment", "params": {"text": f"{count}w"}})
        elif method == "shutdown":
            break


if __name__ == "__main__":
    main()
