// A small library for writing mog plugins in JavaScript, for Node 18 or newer, with no
// dependencies. Copy this file next to your plugin. See docs/plugins.md for the protocol.
//
//   const { Plugin, status } = require("./mog-plugin");
//   const plugin = new Plugin();
//   plugin.command("count", { title: "Words: Count" }, (context) => [
//     status(`${(context.text || "").split(/\s+/).filter(Boolean).length} words`),
//   ]);
//   plugin.run();

"use strict";

const PROTOCOL_VERSION = 2;

// events mog sends as requests and waits on, the rest are notifications
const REQUEST_EVENTS = new Set(["before_save"]);

/** An action that shows text in the status line. */
const status = (text) => ({ type: "status", text });

/** An action that shows a message with a level: info, warning or error. */
const notify = (text, level = "info") => ({ type: "notify", text, level });

/** An action that replaces the selection, or types at the cursor. */
const insert = (text) => ({ type: "insert", text });

/** One change for edit: replace chars start..end with text. */
const change = (start, end, text) => ({ start, end, text });

/** An action that changes one file in one undo step, refused if it changed since version. */
const edit = (changes, path, version) => {
  const action = { type: "edit", changes };
  if (path !== undefined) action.path = path;
  if (version !== undefined) action.version = version;
  return action;
};

/** An action that changes several files, all of them or none. Each edit is from edit(). */
const workspaceEdit = (edits) => ({
  type: "workspace_edit",
  edits: edits.map(({ type, ...rest }) => rest),
});

/** An action that opens a file, optionally at a line and column from 0. */
const openFile = (path, line, column) => ({ type: "open", path, line, column });

/** An action that runs a mog command by name. */
const command = (name, args = null) => ({ type: "command", name, args });

/** An action that sets the selections, a list of [anchor, head] pairs. */
const select = (selections, path) => ({
  type: "select",
  path,
  selections: selections.map(([anchor, head]) => ({ anchor, head })),
});

/** An action that saves a file, the focused one by default. */
const save = (path) => ({ type: "save", path });

/** An action that shows text in the output panel. */
const output = (text, title = "plugin") => ({ type: "output", title, text });

/** A piece of a widget row in one style. Colors are theme color names or hex codes. */
const span = (text, { fg, bg, bold, italic, underline } = {}) => {
  const piece = { text };
  if (fg !== undefined) piece.fg = fg;
  if (bg !== undefined) piece.bg = bg;
  if (bold) piece.bold = true;
  if (italic) piece.italic = true;
  if (underline) piece.underline = true;
  return piece;
};

/** Splits framed JSON-RPC messages out of a byte stream. */
class Reader {
  constructor(onMessage) {
    this.buffer = Buffer.alloc(0);
    this.onMessage = onMessage;
  }

  feed(chunk) {
    this.buffer = Buffer.concat([this.buffer, chunk]);
    for (;;) {
      const end = this.buffer.indexOf("\r\n\r\n");
      if (end < 0) return;
      const header = this.buffer.subarray(0, end).toString();
      const match = /content-length:\s*(\d+)/i.exec(header);
      if (!match) throw new Error("a message has no Content-Length");
      const length = Number(match[1]);
      if (this.buffer.length < end + 4 + length) return;
      const body = this.buffer.subarray(end + 4, end + 4 + length).toString("utf8");
      this.buffer = this.buffer.subarray(end + 4 + length);
      this.onMessage(JSON.parse(body));
    }
  }
}

/** A mog plugin. Register handlers, then call run(). Handlers may be async. */
class Plugin {
  constructor({ input = process.stdin, output = process.stdout } = {}) {
    this.input = input;
    this.output = output;
    this.commands = new Map();
    this.titles = [];
    this.events = new Map();
    this.providers = new Map();
    this.tools = new Map();
    this.pending = new Map();
    this.cancelled = new Set();
    this.keyHandler = null;
    this.timers = new Map();
    this.early = [];
    this.ready = false;
    this.nextId = 0;
    this.root = null;
    this.settings = {};
    this.mogVersion = null;
    this.capabilities = {};
  }

  /** Registers a command; the handler gets (context, args) and returns a list of actions. */
  command(name, { title = name, keys = [], menu = false } = {}, handler) {
    this.commands.set(name, handler);
    this.titles.push({ name, title, keys, menu });
    return this;
  }

  /** Registers a handler for an event like "saved". For "before_save" it returns changes. */
  on(event, handler) {
    this.events.set(event, handler);
    return this;
  }

  /** Registers a provider like "completion" for file extensions, or every file with true. */
  provide(provider, languages, handler) {
    this.providers.set(provider, { handler, languages });
    return this;
  }

  /**
   * Registers a tool the AI chat may call while it answers. The handler gets the input object
   * and returns text, or any other JSON value which is sent as JSON. Throwing tells the model
   * the tool failed, with the message.
   */
  tool(name, { description = "", inputSchema } = {}, handler) {
    this.tools.set(name, {
      handler,
      tool: {
        name,
        description,
        input_schema: inputSchema || { type: "object", properties: {} },
      },
    });
    return this;
  }

  /**
   * Registers the handler for keys this plugin takes, see capture(). It gets (key, char,
   * context) and returns a list of actions, nothing when it used the key, false to let the
   * editor have it, or an object like { actions, capture }.
   */
  onKey(handler) {
    this.keyHandler = handler;
    return this;
  }

  /** Registers a handler mog calls every ms milliseconds. */
  every(ms, handler, id = "timer") {
    this.timers.set(id, handler);
    this.timer(id, ms);
    return this;
  }

  write(message) {
    const body = Buffer.from(JSON.stringify({ jsonrpc: "2.0", ...message }), "utf8");
    this.output.write(`Content-Length: ${body.length}\r\n\r\n`);
    this.output.write(body);
  }

  /** Asks mog something, like "ui/pick" or "editor/text", and resolves with the answer. */
  ask(method, params = {}) {
    const id = `ask-${++this.nextId}`;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.write({ id, method, params });
    });
  }

  /** Sends mog a notification. Ones sent before mog said hello go out right after. */
  notify(method, params = {}) {
    if (this.ready) this.write({ method, params });
    else this.early.push({ method, params });
  }

  /** Asks mog to do actions now. */
  actions(actions) {
    this.notify("actions", { actions });
  }

  /** Shows text in the status line. */
  status(text) {
    this.notify("status", { text });
  }

  /** Adds a line to this plugin's log, shown by plugins.log. */
  log(text) {
    this.notify("log", { text: String(text) });
  }

  /** Puts a short text in the status line, an empty text removes it. */
  segment(text, { color = null, command = null, id, bg, bold, side } = {}) {
    this.notify("segment", { text, color, command, id, bg, bold, side });
  }

  /**
   * Shows a notification in the top right corner, or changes the one with this id. progress
   * is a percentage or true for busy, buttons a list of { title, command }.
   */
  toast(id, title, { text = "", level = "info", progress, timeout, buttons } = {}) {
    this.notify("toast", { id, title, text, level, progress, timeout, buttons });
  }

  /** Removes the notification with this id. */
  closeToast(id) {
    this.notify("toast", { id, done: true });
  }

  /** Shows or fills a panel docked on the right, at the bottom or in the sidebar (side is
   * "right", "bottom" or "sidebar"), see the docs for options like icon, size and focus. */
  panel(id, { title, lines, side = "right", ...options } = {}) {
    this.notify("panel", { ...options, id, title, lines, side });
  }

  /** Paints cells over the editor as flair, [x, y, char, fg, bg] each. */
  canvas(id = "canvas", { cells = [], rows, clear = true, still = false } = {}) {
    this.notify("canvas", { id, cells, rows, clear, still });
  }

  /** Shows progress of long work in the status line. */
  progress(id, title, { percentage = null, done = false } = {}) {
    this.notify("progress", { id, title, percentage, done });
  }

  /** Replaces this plugin's diagnostics for a file. */
  diagnostics(path, diagnostics) {
    this.notify("diagnostics", { path, diagnostics });
  }

  /** Replaces the text this plugin shows after lines of a file. */
  decorations(path, decorations) {
    this.notify("decorations", { path, decorations });
  }

  /**
   * Puts a widget on the screen, or replaces the one with this id. lines is a list of rows,
   * each a string or a list of span(). Options: frames, fps, anchor, x, y, flair,
   * transparent, clickable, fg, bg, z, motion and restart.
   */
  draw(id, lines, options = {}) {
    const params = { id, anchor: "top_left", x: 0, y: 0, ...options };
    if (params.frames === undefined) params.lines = lines || [];
    this.notify("draw", params);
  }

  /** Removes the widget with this id, or every widget of this plugin. */
  clear(id) {
    this.notify("clear", id === undefined ? {} : { id });
  }

  /** Sets the cursor shape: default, block, bar or underline. */
  cursor(shape = "default", blink = false) {
    this.notify("cursor", { shape, blink });
  }

  /** Takes keys before the editor: "all", a list like ["esc"], or null for none. */
  capture(keys, except = []) {
    this.notify("capture", except.length ? { keys, except } : { keys });
  }

  /** Starts a timer that sends the "timer" event every ms milliseconds, 0 stops it. */
  timer(id, ms) {
    this.notify("timer", { id, every: ms });
  }

  hello(params) {
    this.root = params.root;
    this.settings = params.settings || {};
    this.mogVersion = params.mogVersion;
    this.capabilities = params.capabilities || {};
    const providers = {};
    for (const [name, { languages }] of this.providers) providers[name] = languages;
    return {
      protocolVersion: PROTOCOL_VERSION,
      commands: this.titles,
      events: [...this.events.keys()],
      providers,
      tools: [...this.tools.values()].map((entry) => entry.tool),
    };
  }

  async answer(method, params) {
    if (method === "initialize") return this.hello(params);
    if (method === "key") {
      if (!this.keyHandler) return { handled: false };
      const answer = await this.keyHandler(params.key, params.char, params);
      if (answer === false) return { handled: false };
      if (answer && !Array.isArray(answer)) return answer;
      return { actions: answer || [] };
    }
    if (method === "command") {
      const handler = this.commands.get(params.command);
      if (!handler) throw new Error(`no command called ${params.command}`);
      return { actions: (await handler(params.context || {}, params.args)) || [] };
    }
    if (REQUEST_EVENTS.has(method)) {
      const handler = this.events.get(method);
      return { changes: (handler && (await handler(params))) || [] };
    }
    if (method === "tool/call") {
      const entry = this.tools.get(params.name);
      if (!entry) throw new Error(`no tool called ${params.name}`);
      const result = await entry.handler(params.input || {});
      return { content: typeof result === "string" ? result : JSON.stringify(result) };
    }
    if (method.startsWith("provide/")) {
      const entry = this.providers.get(method.slice("provide/".length));
      if (!entry) throw new Error(`no provider for ${method}`);
      return (await entry.handler(params)) || {};
    }
    throw new Error(`unknown request ${method}`);
  }

  async receive(message) {
    const { id, method } = message;
    const params = message.params || {};
    if (method === undefined) {
      const waiting = this.pending.get(id);
      if (!waiting) return;
      this.pending.delete(id);
      if (message.error) waiting.reject(new Error(message.error.message || "mog said no"));
      else waiting.resolve(message.result);
      return;
    }
    if (id !== undefined) {
      let reply;
      try {
        reply = { id, result: await this.answer(method, params) };
      } catch (error) {
        process.stderr.write(`${error.stack || error}\n`);
        reply = { id, error: { code: -32000, message: String(error.message || error) } };
      }
      if (!this.cancelled.delete(id)) this.write(reply);
      if (method === "initialize") {
        this.ready = true;
        for (const early of this.early) this.write(early);
        this.early = [];
      }
      return;
    }
    if (method === "event") {
      const timer = params.kind === "timer" && this.timers.get(params.id);
      const handler = timer ? () => timer() : this.events.get(params.kind);
      if (handler) {
        try {
          await handler(params);
        } catch (error) {
          process.stderr.write(`${error.stack || error}\n`);
        }
      }
    } else if (method === "$/cancelRequest") {
      this.cancelled.add(params.id);
    } else if (method === "shutdown") {
      process.exit(0);
    }
  }

  /** Answers mog until it says shutdown or goes away. */
  run() {
    const reader = new Reader((message) => {
      this.receive(message).catch((error) => process.stderr.write(`${error.stack || error}\n`));
    });
    this.input.on("data", (chunk) => reader.feed(chunk));
    this.input.on("end", () => process.exit(0));
  }
}

module.exports = {
  PROTOCOL_VERSION,
  Plugin,
  Reader,
  status,
  notify,
  insert,
  change,
  edit,
  workspaceEdit,
  openFile,
  command,
  select,
  save,
  output,
  span,
};
