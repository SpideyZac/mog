"""A small library for writing mog plugins in Python, with no dependencies.

Copy this file next to your plugin, or put its folder on PYTHONPATH. See docs/plugins.md for the
protocol it speaks.

    from mog_plugin import Plugin, status

    plugin = Plugin()

    @plugin.command("count", title="Words: Count", keys=["alt+shift+w"])
    def count(context, args):
        return [status(f"{len((context['text'] or '').split())} words")]

    @plugin.on("saved")
    def saved(event):
        plugin.log(f"saved {event['path']}")

    plugin.run()
"""

import json
import sys
import traceback

PROTOCOL_VERSION = 2

# events that mog sends as requests and waits on, the rest are notifications
REQUEST_EVENTS = {"before_save"}


class MogError(Exception):
    """Mog answered a request with an error, or went away."""


def status(text):
    """An action that shows text in the status line."""
    return {"type": "status", "text": text}


def notify(text, level="info"):
    """An action that shows a message with a level: info, warning or error."""
    return {"type": "notify", "text": text, "level": level}


def insert(text):
    """An action that replaces the selection, or types at the cursor."""
    return {"type": "insert", "text": text}


def change(start, end, text):
    """One change for edit: replace chars start..end with text."""
    return {"start": start, "end": end, "text": text}


def edit(changes, path=None, version=None):
    """An action that changes one file in one undo step. Pass the version you read the text at
    so mog refuses the edit if the file changed since."""
    action = {"type": "edit", "changes": list(changes)}
    if path is not None:
        action["path"] = path
    if version is not None:
        action["version"] = version
    return action


def workspace_edit(edits):
    """An action that changes several files, all of them or none. Each edit is from edit()."""
    return {
        "type": "workspace_edit",
        "edits": [{k: v for k, v in e.items() if k != "type"} for e in edits],
    }


def open_file(path, line=None, column=None):
    """An action that opens a file, optionally at a line and column from 0."""
    action = {"type": "open", "path": path}
    if line is not None:
        action["line"] = line
    if column is not None:
        action["column"] = column
    return action


def command(name, args=None):
    """An action that runs a mog command by name."""
    return {"type": "command", "name": name, "args": args}


def select(selections, path=None):
    """An action that sets the selections, a list of (anchor, head) pairs."""
    action = {
        "type": "select",
        "selections": [{"anchor": a, "head": h} for a, h in selections],
    }
    if path is not None:
        action["path"] = path
    return action


def save(path=None):
    """An action that saves a file, the focused one by default."""
    return {"type": "save", "path": path}


def output(text, title="plugin"):
    """An action that shows text in the output panel."""
    return {"type": "output", "title": title, "text": text}


def span(text, fg=None, bg=None, bold=False, italic=False, underline=False):
    """A piece of a widget row in one style. Colors are theme color names or hex codes."""
    piece = {"text": text}
    for key, value in (("fg", fg), ("bg", bg)):
        if value is not None:
            piece[key] = value
    for key, value in (("bold", bold), ("italic", italic), ("underline", underline)):
        if value:
            piece[key] = True
    return piece


class Plugin:
    """A mog plugin. Register handlers with the decorators, then call run()."""

    def __init__(self, stdin=None, stdout=None):
        self._in = stdin or sys.stdin.buffer
        self._out = stdout or sys.stdout.buffer
        self._commands = {}
        self._titles = []
        self._events = {}
        self._providers = {}
        self._waiting = []
        self._key = None
        self._timers = {}
        self._early = []
        self._ready = False
        self._next_id = 0
        self._cancelled = set()
        self.root = None
        self.settings = {}
        self.mog_version = None
        self.capabilities = {}

    # registering

    def command(self, name, title=None, keys=None, menu=False):
        """Registers a command. The handler gets (context, args) and returns a list of actions
        or None. Raising an exception shows its message to the user."""

        def register(handler):
            self._commands[name] = handler
            self._titles.append(
                {"name": name, "title": title or name, "keys": keys or [], "menu": menu}
            )
            return handler

        return register

    def on(self, event):
        """Registers a handler for an event like "saved" or "changed". It gets the event's
        params. For "before_save" it returns a list of changes to make before the file is
        written, or None."""

        def register(handler):
            self._events[event] = handler
            return handler

        return register

    def provide(self, provider, languages=True):
        """Registers a provider like "completion", "hover", "formatting" or "code_actions" for
        file extensions in languages, or every file when it is True. The handler gets the
        request params and returns the result dict."""

        def register(handler):
            self._providers[provider] = (handler, languages)
            return handler

        return register

    def on_key(self, handler):
        """Registers the handler for keys this plugin takes, see capture(). It gets (key, char,
        context): the key like "shift+g", the char it types or None, and where the cursor is.
        It returns a list of actions, None when it used the key and has nothing to do, False to
        let the editor have the key, or a dict like {"actions": [...], "capture": {...}}."""
        self._key = handler
        return handler

    def every(self, ms, id="timer"):
        """Registers a handler mog calls every ms milliseconds, with no arguments."""

        def register(handler):
            self._timers[id] = handler
            self.timer(id, ms)
            return handler

        return register

    # talking to mog

    def _read(self):
        length = None
        while True:
            line = self._in.readline()
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
        return json.loads(self._in.read(length))

    def _write(self, message):
        body = json.dumps(dict(message, jsonrpc="2.0")).encode()
        self._out.write(b"Content-Length: %d\r\n\r\n" % len(body))
        self._out.write(body)
        self._out.flush()

    def _next_message(self):
        return self._waiting.pop(0) if self._waiting else self._read()

    def ask(self, method, params=None):
        """Asks mog something, like "ui/pick" or "editor/text", and waits for the answer.
        Messages that arrive meanwhile are handled afterwards."""
        self._next_id += 1
        question = f"ask-{self._next_id}"
        self._write({"id": question, "method": method, "params": params or {}})
        while (message := self._read()) is not None:
            if message.get("id") == question and "method" not in message:
                if "error" in message:
                    raise MogError(message["error"].get("message", "mog said no"))
                return message.get("result")
            if message.get("method") == "$/cancelRequest":
                self._cancelled.add(message.get("params", {}).get("id"))
                continue
            self._waiting.append(message)
        raise MogError("mog went away")

    def notify(self, method, params=None):
        """Sends mog a notification, like "segment" or "diagnostics". Ones sent before mog
        said hello go out right after."""
        message = {"method": method, "params": params or {}}
        if self._ready:
            self._write(message)
        else:
            self._early.append(message)

    def actions(self, actions):
        """Asks mog to do actions now, without waiting for a command."""
        self.notify("actions", {"actions": list(actions)})

    def status(self, text):
        """Shows text in the status line."""
        self.notify("status", {"text": text})

    def log(self, text):
        """Adds a line to this plugin's log, shown by the plugins.log command."""
        self.notify("log", {"text": str(text)})

    def segment(self, text, color=None, command=None, id=None):
        """Puts a short text in the status line until replaced, an empty text removes it. Give
        an id to have more than one. Clicking it runs command."""
        params = {"text": text, "color": color, "command": command}
        if id is not None:
            params["id"] = id
        self.notify("segment", params)

    def progress(self, id, title, percentage=None, done=False):
        """Shows progress of long work in the status line."""
        self.notify(
            "progress", {"id": id, "title": title, "percentage": percentage, "done": done}
        )

    def diagnostics(self, path, diagnostics):
        """Replaces this plugin's diagnostics for a file. Each is a dict with start and end
        char offsets, or line and column, plus severity and message."""
        self.notify("diagnostics", {"path": path, "diagnostics": list(diagnostics)})

    def decorations(self, path, decorations):
        """Replaces the text this plugin shows after lines of a file. Each is a dict with line,
        text and an optional color."""
        self.notify("decorations", {"path": path, "decorations": list(decorations)})

    def draw(self, id, lines=None, frames=None, anchor="top_left", x=0, y=0, **options):
        """Puts a widget on the screen, or replaces the one with this id. lines is a list of
        rows, each a string or a list of span(). frames is a list of such lists to cycle
        through at fps. Other options: fps, flair, transparent, clickable, fg, bg, z, motion
        (a dict with dx, dy and edge), restart. Anchors are top_left, top_right, bottom_left,
        bottom_right, center, cursor and screen."""
        params = {"id": id, "anchor": anchor, "x": x, "y": y}
        if frames is not None:
            params["frames"] = frames
        else:
            params["lines"] = lines or []
        params.update(options)
        self.notify("draw", params)

    def clear(self, id=None):
        """Removes the widget with this id, or every widget of this plugin."""
        self.notify("clear", {} if id is None else {"id": id})

    def cursor(self, shape="default", blink=False):
        """Sets the cursor shape: default, block, bar or underline."""
        self.notify("cursor", {"shape": shape, "blink": blink})

    def capture(self, keys=None, except_keys=None):
        """Takes keys before the editor does, handing them to on_key(). keys is "all", a list
        like ["esc"], or None to take none. except_keys are left alone when taking all."""
        params = {"keys": keys}
        if except_keys:
            params["except"] = list(except_keys)
        self.notify("capture", params)

    def timer(self, id, ms):
        """Starts a timer that sends the "timer" event every ms milliseconds, 0 stops it."""
        self.notify("timer", {"id": id, "every": ms})

    # running

    def _hello(self, params):
        self.root = params.get("root")
        self.settings = params.get("settings") or {}
        self.mog_version = params.get("mogVersion")
        self.capabilities = params.get("capabilities") or {}
        return {
            "protocolVersion": PROTOCOL_VERSION,
            "commands": self._titles,
            "events": sorted(self._events),
            "providers": {name: languages for name, (_, languages) in self._providers.items()},
        }

    def _answer(self, message):
        method = message["method"]
        params = message.get("params") or {}
        if method == "initialize":
            return self._hello(params)
        if method == "key":
            if self._key is None:
                return {"handled": False}
            answer = self._key(params.get("key"), params.get("char"), params)
            if answer is False:
                return {"handled": False}
            if isinstance(answer, dict):
                return answer
            return {"actions": answer or []}
        if method == "command":
            handler = self._commands.get(params.get("command"))
            if handler is None:
                raise ValueError(f"no command called {params.get('command')}")
            return {"actions": handler(params.get("context") or {}, params.get("args")) or []}
        if method in REQUEST_EVENTS:
            handler = self._events.get(method)
            changes = handler(params) if handler else None
            return {"changes": changes or []}
        if method.startswith("provide/"):
            entry = self._providers.get(method[len("provide/"):])
            if entry is None:
                raise ValueError(f"no provider for {method}")
            return entry[0](params) or {}
        raise ValueError(f"unknown request {method}")

    def run(self):
        """Answers mog until it says shutdown or goes away."""
        while (message := self._next_message()) is not None:
            method = message.get("method")
            if method is None:
                continue
            if "id" in message:
                try:
                    result = self._answer(message)
                    reply = {"id": message["id"], "result": result}
                except Exception as error:  # noqa: BLE001, a plugin should answer, not crash
                    print(traceback.format_exc(), file=sys.stderr)
                    reply = {"id": message["id"], "error": {"code": -32000, "message": str(error)}}
                if message["id"] in self._cancelled:
                    self._cancelled.discard(message["id"])
                else:
                    self._write(reply)
                if method == "initialize":
                    self._ready = True
                    for early in self._early:
                        self._write(early)
                    self._early = []
            elif method == "event":
                params = message.get("params") or {}
                handler = self._events.get(params.get("kind"))
                if params.get("kind") == "timer" and params.get("id") in self._timers:
                    timer = self._timers[params["id"]]
                    handler = lambda _params: timer()  # noqa: E731
                if handler:
                    try:
                        handler(params)
                    except Exception:  # noqa: BLE001
                        print(traceback.format_exc(), file=sys.stderr)
            elif method == "$/cancelRequest":
                self._cancelled.add((message.get("params") or {}).get("id"))
            elif method == "shutdown":
                break
