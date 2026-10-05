"""Vim motions for mog, as a plugin.

Normal, insert and visual modes, counts, operators with motions, registers, search and a few
ex commands, all worked out here from the text and sent to mog as edits and selections. It
shows that a plugin can take over the keyboard: it captures every key in normal mode and only
esc in insert mode, sets the cursor shape and draws the keys typed so far on the screen.

Not covered: the dot command, macros, marks, text objects and named registers.
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from mog_plugin import Plugin, change, command, edit, select, status  # noqa: E402

plugin = Plugin()

OPERATORS = "dcy<>"
# motions that take the char typed after them
FIND = "fFtT"
# keys with no char that vim uses, mapped to what they mean here
NAMED = {
    "esc": "<esc>",
    "enter": "+",
    "backspace": "h",
    "space": "l",
    "ctrl+r": "<redo>",
    "ctrl+d": "<pagedown>",
    "ctrl+u": "<pageup>",
}
MODE_COLORS = {"normal": "blue", "insert": "green", "visual": "purple", "visual line": "purple"}
INDENT = "    "


class Text:
    """The document text with helpers for lines, in char offsets."""

    def __init__(self, text, version):
        self.t = text
        self.n = len(text)
        self.version = version

    def line_start(self, p):
        return self.t.rfind("\n", 0, p) + 1

    def line_end(self, p):
        end = self.t.find("\n", p)
        return self.n if end < 0 else end

    def line_of(self, p):
        return self.t.count("\n", 0, p)

    def line_count(self):
        return self.t.count("\n") + 1

    def start_of_line(self, line):
        p = 0
        for _ in range(line):
            p = self.t.find("\n", p) + 1
        return p

    def last_char(self, p):
        """The last char of the line at p, where the normal mode cursor can go."""
        start, end = self.line_start(p), self.line_end(p)
        return max(start, end - 1)

    def first_blank(self, p):
        """The first char of the line at p that is not a space."""
        q = self.line_start(p)
        end = self.line_end(p)
        while q < end and self.t[q] in " \t":
            q += 1
        return q

    def column(self, p):
        return p - self.line_start(p)

    def at_column(self, line, column):
        start = self.start_of_line(line)
        end = self.line_end(start)
        return start + max(0, min(column, end - start - 1))


def kind(ch, big):
    """Which kind of char ch is for word motions: space, word or punctuation."""
    if ch.isspace():
        return 0
    if big or ch.isalnum() or ch == "_":
        return 1
    return 2


def next_word(text, p, big):
    t, n = text.t, text.n
    if p >= n:
        return n
    k = kind(t[p], big)
    if k:
        while p < n and kind(t[p], big) == k:
            p += 1
    while p < n and t[p].isspace():
        # an empty line counts as a word
        if t[p] == "\n" and p + 1 < n and t[p + 1] == "\n":
            return p + 1
        p += 1
    return p


def prev_word(text, p, big):
    t = text.t
    if p <= 0:
        return 0
    p -= 1
    while p > 0 and t[p].isspace():
        if t[p] == "\n" and t[p - 1] == "\n":
            return p
        p -= 1
    k = kind(t[p], big)
    while p > 0 and kind(t[p - 1], big) == k:
        p -= 1
    return p


def end_word(text, p, big):
    t, n = text.t, text.n
    p += 1
    while p < n and t[p].isspace():
        p += 1
    if p >= n:
        return max(n - 1, 0)
    k = kind(t[p], big)
    while p + 1 < n and kind(t[p + 1], big) == k:
        p += 1
    return p


def match_bracket(text, p):
    pairs = {"(": ")", "[": "]", "{": "}"}
    closing = {v: k for k, v in pairs.items()}
    t = text.t
    end = text.line_end(p)
    while p < end and t[p] not in pairs and t[p] not in closing:
        p += 1
    if p >= end:
        return None
    ch = t[p]
    forward = ch in pairs
    other = pairs.get(ch) or closing[ch]
    depth, step = 0, 1 if forward else -1
    while 0 <= p < text.n:
        if t[p] == ch:
            depth += 1
        elif t[p] == other:
            depth -= 1
            if depth == 0:
                return p
        p += step
    return None


class Vim:
    """The state of vim between keys."""

    def __init__(self):
        self.on = True
        self.mode = "normal"
        self.keys = ""
        # the yanked text and whether it is whole lines
        self.register = ("", False)
        # the column j and k aim for
        self.want = None
        # where visual mode started and where its cursor is
        self.anchor = 0
        self.cur = 0
        self.find = None
        self.search = None

    # showing the mode

    def capture(self):
        """The keys vim takes in this mode."""
        if not self.on:
            return {"keys": None}
        return {"keys": ["esc"] if self.mode == "insert" else "all"}

    def show(self):
        plugin.capture(self.capture()["keys"])
        if not self.on:
            plugin.cursor("default")
            plugin.segment("", id="mode")
            plugin.clear()
            return
        plugin.cursor("bar" if self.mode == "insert" else "block")
        plugin.segment(self.mode.upper(), color=MODE_COLORS[self.mode], id="mode")
        self.show_keys()

    def show_keys(self):
        if self.keys:
            plugin.draw(
                "keys",
                [f" {self.keys} "],
                anchor="bottom_right",
                x=1,
                flair=False,
                fg="accent",
                bg="raised",
                z=10,
            )
        else:
            plugin.clear("keys")

    def set_mode(self, mode):
        self.mode = mode
        self.keys = ""
        self.show()

    # handling keys

    def key(self, key, char, context):
        if not self.on:
            return False
        name = NAMED.get(key)
        if name is None and char is None:
            return False
        if name is None and ("ctrl+" in key or "alt+" in key):
            return False
        token = name or char
        if self.mode == "insert":
            if token == "<esc>":
                text = self.text()
                p = context["selection"]["head"]
                self.set_mode("normal")
                left = p - 1 if p > text.line_start(p) else p
                return [select([(left, left)])]
            return False
        if token == "<esc>":
            leaving = self.mode.startswith("visual")
            self.keys = ""
            if leaving:
                self.set_mode("normal")
                return [select([(self.cur, self.cur)])]
            self.show_keys()
            return []
        if token in ("<redo>", "<pagedown>", "<pageup>"):
            self.keys = ""
            self.show_keys()
            name = {"<redo>": "redo", "<pagedown>": "page_down", "<pageup>": "page_up"}[token]
            return [command(name)]
        self.keys += token
        parsed = self.parse(self.keys)
        if parsed == "more":
            self.show_keys()
            return []
        self.keys = ""
        self.show_keys()
        if parsed is None:
            return []
        if not self.mode.startswith("visual"):
            self.cur = context["selection"]["head"]
        return self.run(parsed, self.text())

    def text(self):
        answer = plugin.ask("editor/text", {})
        return Text(answer["text"], answer["version"])

    # reading what was typed

    def parse(self, keys):
        """Splits keys into (count, operator, motion or command, has count), "more" while
        unfinished, or None when it means nothing."""
        i, count1 = self.read_count(keys, 0)
        if i == len(keys):
            return "more"
        op = None
        if keys[i] in OPERATORS and not self.mode.startswith("visual"):
            op = keys[i]
            i += 1
            j, count2 = self.read_count(keys, i)
            if j == len(keys):
                return "more"
            if keys[j] == op:
                return (count1 * count2, op, "line", count1 > 1 or count2 > 1)
            count1 *= count2
            i = j
        rest = keys[i:]
        c = rest[0]
        if c in FIND or c == "r":
            if len(rest) < 2:
                return "more"
            return (count1, op, rest[:2], keys[:1].isdigit())
        if c == "g":
            if len(rest) < 2:
                return "more"
            return (count1, op, rest[:2], keys[:1].isdigit())
        return (count1, op, c, keys[:1].isdigit() and keys[:1] != "0")

    @staticmethod
    def read_count(keys, i):
        j = i
        while j < len(keys) and keys[j].isdigit() and not (j == i and keys[j] == "0"):
            j += 1
        return j, int(keys[i:j]) if j > i else 1

    # motions

    def motion(self, name, count, text, cur, op, has_count):
        """Returns (target, how) for a motion, how being exclusive, inclusive or linewise, or
        None when name is not a motion."""
        t = text
        if name in "hl" or name in ("<left>", "<right>"):
            if name == "h":
                return max(t.line_start(cur), cur - count), "exclusive"
            limit = t.line_end(cur) if op else t.last_char(cur)
            return min(limit, cur + count), "exclusive"
        if name in "jk+-":
            line = t.line_of(cur)
            target = line + count if name in "j+" else line - count
            target = max(0, min(t.line_count() - 1, target))
            if name in "+-":
                return t.first_blank(t.start_of_line(target)), "linewise"
            if self.want is None:
                self.want = t.column(cur)
            return t.at_column(target, self.want), "linewise"
        if name in "wW":
            p = cur
            for _ in range(count):
                p = next_word(t, p, name == "W")
            if op == "c" and cur < t.n and not t.t[cur].isspace():
                p = cur
                for _ in range(count):
                    p = end_word(t, p, name == "W")
                return p, "inclusive"
            if op and t.line_of(p) > t.line_of(cur) and p > t.line_end(cur):
                # dw on the last word of a line stops at the end of the line
                p = max(cur, t.line_end(cur))
            return p, "exclusive"
        if name in "bB":
            p = cur
            for _ in range(count):
                p = prev_word(t, p, name == "B")
            return p, "exclusive"
        if name in "eE":
            p = cur
            for _ in range(count):
                p = end_word(t, p, name == "E")
            return p, "inclusive"
        if name == "0":
            return t.line_start(cur), "exclusive"
        if name == "^":
            return t.first_blank(cur), "exclusive"
        if name == "$":
            line = min(t.line_of(cur) + count - 1, t.line_count() - 1)
            end = t.line_end(t.start_of_line(line))
            return (end if op else max(t.line_start(end), end - 1)), "inclusive"
        if name == "G":
            line = count - 1 if has_count else t.line_count() - 1
            line = max(0, min(line, t.line_count() - 1))
            return t.first_blank(t.start_of_line(line)), "linewise"
        if name == "gg":
            line = max(0, min(count - 1 if has_count else 0, t.line_count() - 1))
            return t.first_blank(t.start_of_line(line)), "linewise"
        if name == "ge":
            p = max(prev_word(t, cur, False) - 1, 0)
            return end_word(t, p, False) if p else 0, "inclusive"
        if name[:1] in FIND and len(name) == 2:
            self.find = name
            return self.find_char(name, count, t, cur)
        if name in ";,":
            if not self.find:
                return None
            key, ch = self.find
            if name == ",":
                key = key.swapcase()
            return self.find_char(key + ch, count, t, cur)
        if name in "{}":
            p = cur
            for _ in range(count):
                if name == "}":
                    q = t.t.find("\n\n", p + 1)
                    p = t.n if q < 0 else q + 1
                else:
                    q = t.t.rfind("\n\n", 0, max(p - 1, 0))
                    p = 0 if q < 0 else q + 1
            return p, "exclusive"
        if name == "%":
            p = match_bracket(t, cur)
            return (p, "inclusive") if p is not None else None
        return None

    @staticmethod
    def find_char(name, count, t, cur):
        key, ch = name[0], name[1]
        start, end = t.line_start(cur), t.line_end(cur)
        p = cur
        for _ in range(count):
            if key in "ft":
                q = t.t.find(ch, p + 1 + (key == "t"), end)
            else:
                q = t.t.rfind(ch, start, max(p - (key == "T"), start))
            if q < 0:
                return cur, "exclusive"
            p = q
        if key == "t":
            p -= 1
        elif key == "T":
            p += 1
        return p, "inclusive" if key in "ft" else "exclusive"

    # doing things

    def run(self, parsed, text):
        count, op, name, has_count = parsed
        cur = self.cur
        if name not in "jk":
            self.want = None
        if self.mode.startswith("visual"):
            return self.run_visual(count, name, has_count, text)
        if op and name == "line":
            line = text.line_of(cur)
            last = min(line + count - 1, text.line_count() - 1)
            start = text.start_of_line(line)
            return self.operate(op, start, text.start_of_line(last), "linewise", text)
        if name == "x":
            return self.operate("d", cur, min(cur + count, text.line_end(cur)), "exclusive", text)
        if name == "X":
            return self.operate("d", max(cur - count, text.line_start(cur)), cur, "exclusive", text)
        aliases = {"D": ("d", "$"), "C": ("c", "$"), "s": ("c", "l"), "Y": ("y", "line")}
        if name in aliases and not op:
            op, name = aliases[name]
            if name == "line":
                return self.run((count, op, "line", has_count), text)
        if name == "S" and not op:
            return self.run((count, "c", "line", has_count), text)
        moved = self.motion(name, count, text, cur, op, has_count)
        if moved is not None:
            target, how = moved
            if op:
                return self.operate(op, cur, target, how, text)
            return [select([(target, target)])]
        if op:
            return []
        return self.command(name, count, text, cur)

    def command(self, name, count, text, cur):
        t = text
        if name == "i":
            self.set_mode("insert")
            return []
        if name == "a":
            self.set_mode("insert")
            p = min(cur + 1, t.line_end(cur))
            return [select([(p, p)])]
        if name == "I":
            self.set_mode("insert")
            p = t.first_blank(cur)
            return [select([(p, p)])]
        if name == "A":
            self.set_mode("insert")
            p = t.line_end(cur)
            return [select([(p, p)])]
        if name in "oO":
            indent = t.t[t.line_start(cur) : t.first_blank(cur)]
            self.set_mode("insert")
            if name == "o":
                at = t.line_end(cur)
                return [
                    edit([change(at, at, "\n" + indent)], version=t.version),
                    select([(at + 1 + len(indent),) * 2]),
                ]
            at = t.line_start(cur)
            return [
                edit([change(at, at, indent + "\n")], version=t.version),
                select([(at + len(indent),) * 2]),
            ]
        if name in "vV":
            self.anchor = self.cur = cur
            self.set_mode("visual" if name == "v" else "visual line")
            return [self.visual_selection(t)]
        if name in "pP":
            return self.paste(name == "p", count, t, cur)
        if name == "u":
            return [command("undo")] * count
        if name[:1] == "r" and len(name) == 2:
            end = min(cur + count, t.line_end(cur))
            if end - cur < count:
                return []
            return [
                edit([change(cur, end, name[1] * count)], version=t.version),
                select([(end - 1, end - 1)]),
            ]
        if name == "J":
            changes = []
            p = cur
            for _ in range(max(count - 1, 1)):
                end = t.line_end(p)
                if end >= t.n:
                    break
                nxt = end + 1
                while nxt < t.n and t.t[nxt] in " \t":
                    nxt += 1
                changes.append(change(end, nxt, " "))
                p = nxt
            if not changes:
                return []
            return [edit(changes, version=t.version)]
        if name == "~":
            end = min(cur + count, t.line_end(cur))
            swapped = t.t[cur:end].swapcase()
            p = min(end, t.last_char(cur))
            return [edit([change(cur, end, swapped)], version=t.version), select([(p, p)])]
        if name == ":":
            return [command("plugin.vim.ex")]
        if name in "/?":
            return [command("plugin.vim.search", "forward" if name == "/" else "backward")]
        if name in "nN" and self.search:
            pattern, forward = self.search
            return self.find_text(pattern, forward == (name == "n"), t, cur)
        return []

    def operate(self, op, a, b, how, text):
        t = text
        start, end = min(a, b), max(a, b)
        if how == "inclusive":
            end = min(end + 1, t.n)
        if how == "linewise":
            start = t.line_start(start)
            end = min(t.line_end(end) + 1, t.n)
        piece = t.t[start:end]
        lines = how == "linewise"
        if lines and not piece.endswith("\n"):
            piece += "\n"
        if op == "y":
            self.register = (piece, lines)
            p = start if not lines else min(a, b)
            return [status(f"yanked {piece.count(chr(10)) if lines else len(piece)}"
                           f"{' lines' if lines else ' chars'}"), select([(p, p)])]
        if op in "<>":
            return self.shift(op, start, end, t)
        self.register = (piece, lines)
        if op == "c":
            if lines:
                indent = t.t[start : t.first_blank(start)]
                keep_end = end - 1 if t.t[end - 1 : end] == "\n" else end
                self.set_mode("insert")
                return [
                    edit([change(start, keep_end, indent)], version=t.version),
                    select([(start + len(indent),) * 2]),
                ]
            self.set_mode("insert")
            return [edit([change(start, end, "")], version=t.version), select([(start, start)])]
        if lines and end == t.n and start > 0 and not t.t.endswith("\n"):
            # deleting the last lines also takes the line break before them
            start -= 1
        after = t.t[:start] + t.t[end:]
        rest = Text(after, None)
        p = min(start, len(after))
        p = rest.first_blank(p) if lines else min(p, rest.last_char(p))
        return [edit([change(start, end, "")], version=t.version), select([(p, p)])]

    def shift(self, op, start, end, t):
        changes = []
        p = start
        while p < end and p <= t.n:
            line_end = t.line_end(p)
            if op == ">":
                if line_end > p:
                    changes.append(change(p, p, INDENT))
            else:
                q = p
                while q < line_end and q - p < len(INDENT) and t.t[q] == " ":
                    q += 1
                if q == p and t.t[p : p + 1] == "\t":
                    q = p + 1
                if q > p:
                    changes.append(change(p, q, ""))
            if line_end >= t.n:
                break
            p = line_end + 1
        return [edit(changes, version=t.version)] if changes else []

    def paste(self, after, count, t, cur):
        piece, lines = self.register
        if not piece:
            return [status("nothing yanked yet")]
        piece *= count
        if lines:
            if after:
                at = t.line_end(cur)
                if at >= t.n:
                    piece = "\n" + piece.rstrip("\n")
                    return [edit([change(at, at, piece)], version=t.version), select([(at + 1,) * 2])]
                at += 1
            else:
                at = t.line_start(cur)
            return [edit([change(at, at, piece)], version=t.version), select([(at, at)])]
        at = min(cur + 1, t.line_end(cur)) if after else cur
        end = at + len(piece) - 1
        return [edit([change(at, at, piece)], version=t.version), select([(end, end)])]

    def find_text(self, pattern, forward, t, cur):
        if forward:
            p = t.t.find(pattern, cur + 1)
            if p < 0:
                p = t.t.find(pattern)
        else:
            p = t.t.rfind(pattern, 0, cur)
            if p < 0:
                p = t.t.rfind(pattern)
        if p < 0:
            return [status(f"{pattern} not found")]
        return [select([(p, p)])]

    # visual mode

    def visual_selection(self, t):
        a, c = self.anchor, self.cur
        if self.mode == "visual line":
            lo, hi = min(a, c), max(a, c)
            start, end = t.line_start(lo), min(t.line_end(hi) + 1, t.n)
            return select([(start, end)] if c >= a else [(end, start)])
        if c >= a:
            return select([(a, min(c + 1, t.n))])
        return select([(min(a + 1, t.n), c)])

    def run_visual(self, count, name, has_count, t):
        if name == "o":
            self.anchor, self.cur = self.cur, self.anchor
            return [self.visual_selection(t)]
        if name in "vV":
            wanted = "visual" if name == "v" else "visual line"
            if self.mode == wanted:
                self.set_mode("normal")
                return [select([(self.cur, self.cur)])]
            self.set_mode(wanted)
            return [self.visual_selection(t)]
        how = "linewise" if self.mode == "visual line" else "inclusive"
        ops = {"d": "d", "x": "d", "y": "y", "c": "c", "s": "c", ">": ">", "<": "<"}
        if name in ops:
            lo, hi = min(self.anchor, self.cur), max(self.anchor, self.cur)
            self.set_mode("normal")
            return self.operate(ops[name], lo, hi, how, t)
        if name == "~":
            lo, hi = min(self.anchor, self.cur), max(self.anchor, self.cur) + 1
            self.set_mode("normal")
            return [edit([change(lo, hi, t.t[lo:hi].swapcase())], version=t.version),
                    select([(lo, lo)])]
        moved = self.motion(name, count, t, self.cur, None, has_count)
        if moved is None:
            return []
        self.cur = moved[0]
        return [self.visual_selection(t)]


vim = Vim()


@plugin.on_key
def on_key(key, char, context):
    actions = vim.key(key, char, context)
    if actions is False:
        return {"handled": False}
    # the next key may already be waiting, so it has to see the new capture at once
    return {"actions": actions, "capture": vim.capture()}


@plugin.command("toggle", title="Vim: Turn vim motions on or off")
def toggle(context, args):
    vim.on = not vim.on
    vim.mode = "normal"
    vim.keys = ""
    vim.show()
    return [status(f"vim motions {'on' if vim.on else 'off'}")]


@plugin.command("ex", title="Vim: Run an ex command like :w")
def ex(context, args):
    answer = plugin.ask("ui/prompt", {"title": ":", "hint": "w, q, wq, x, qa, a line number or a mog command"})
    if not answer:
        return []
    line = answer["text"].strip()
    simple = {
        "w": [command("save")],
        "q": [command("close_tab")],
        "q!": [command("close_tab")],
        "wq": [command("save"), command("close_tab")],
        "x": [command("save"), command("close_tab")],
        "qa": [command("quit")],
        "wa": [command("save")],
    }
    if line in simple:
        return simple[line]
    if line.isdigit():
        text = vim.text()
        target = max(0, min(int(line) - 1, text.line_count() - 1))
        p = text.first_blank(text.start_of_line(target))
        return [select([(p, p)])]
    return [command(line)]


@plugin.command("search", title="Vim: Search")
def search(context, args):
    forward = args != "backward"
    answer = plugin.ask("ui/prompt", {"title": "/" if forward else "?"})
    if not answer or not answer["text"]:
        return []
    vim.search = (answer["text"], forward)
    return vim.find_text(answer["text"], forward, vim.text(), context["selection"]["head"])


vim.show()
plugin.run()
