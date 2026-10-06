// Checks the Node SDK end to end against a fake mog: node sdk/node/test.js

"use strict";

const assert = require("node:assert");
const { PassThrough } = require("node:stream");
const { Plugin, Reader, status, edit, change, command, span } = require("./mog-plugin");

const toPlugin = new PassThrough();
const fromPlugin = new PassThrough();
const plugin = new Plugin({ input: toPlugin, output: fromPlugin });
plugin.command("count", { title: "Count", keys: ["alt+c"] }, async (context) => {
  const answer = await plugin.ask("editor/text", {});
  return [status(`${answer.text.split(" ").length} words in ${context.path}`)];
});
plugin.on("before_save", () => [change(0, 1, "")]);
plugin.provide("hover", ["md"], () => ({ text: "hi" }));
plugin.tool("shout", { description: "Shouts" }, (input) => input.text.toUpperCase());
plugin.onKey((key) => {
  if (key === "j") return [command("move_down")];
  if (key === "i") return { capture: { keys: ["esc"] } };
  return false;
});
let ticks = 0;
plugin.every(250, () => {
  ticks += 1;
  plugin.draw("eye", [[span("o", { fg: "red" }), "_o"]], { anchor: "cursor" });
}, "blink");
plugin.capture("all", ["ctrl+s"]);
plugin.run();

const replies = [];
const reader = new Reader((message) => {
  replies.push(message);
  // answer the plugin's question like mog would
  if (message.method === "editor/text") {
    send({ id: message.id, result: { text: "one two three" } });
  }
});
fromPlugin.on("data", (chunk) => reader.feed(chunk));

function send(message) {
  const body = Buffer.from(JSON.stringify({ jsonrpc: "2.0", ...message }));
  toPlugin.write(`Content-Length: ${body.length}\r\n\r\n`);
  toPlugin.write(body);
}

send({ id: 0, method: "initialize", params: { root: "/p", settings: { a: 1 } } });
send({ id: 1, method: "command", params: { command: "count", context: { path: "a.md" } } });
send({ id: 2, method: "before_save", params: { text: "xy" } });
send({ id: 3, method: "provide/hover", params: {} });
send({ id: 4, method: "command", params: { command: "nope", context: {} } });
send({ id: 5, method: "key", params: { key: "j", char: "j" } });
send({ id: 6, method: "key", params: { key: "i", char: "i" } });
send({ id: 7, method: "key", params: { key: "x", char: "x" } });
send({ method: "event", params: { kind: "timer", id: "blink" } });
send({ id: 8, method: "tool/call", params: { name: "shout", input: { text: "hi" } } });

setTimeout(() => {
  const byId = (id) => replies.find((reply) => reply.id === id && reply.method === undefined);
  const hello = byId(0).result;
  assert.strictEqual(hello.protocolVersion, 2);
  assert.deepStrictEqual(hello.events, ["before_save"]);
  assert.deepStrictEqual(hello.providers, { hover: ["md"] });
  assert.strictEqual(hello.tools[0].name, "shout");
  assert.deepStrictEqual(byId(8).result, { content: "HI" });
  assert.strictEqual(plugin.settings.a, 1);
  assert.deepStrictEqual(byId(1).result.actions, [status("3 words in a.md")]);
  assert.deepStrictEqual(byId(2).result.changes, [change(0, 1, "")]);
  assert.strictEqual(byId(3).result.text, "hi");
  assert.match(byId(4).error.message, /no command/);
  assert.deepStrictEqual(byId(5).result, { actions: [command("move_down")] });
  assert.deepStrictEqual(byId(6).result, { capture: { keys: ["esc"] } });
  assert.deepStrictEqual(byId(7).result, { handled: false });
  const notes = replies.filter((reply) => reply.method !== undefined && reply.id === undefined);
  assert.deepStrictEqual(notes[0].params, { id: "blink", every: 250 });
  assert.deepStrictEqual(notes[1].params, { keys: "all", except: ["ctrl+s"] });
  assert.strictEqual(notes[notes.length - 1].method, "draw");
  assert.strictEqual(ticks, 1);
  assert.deepStrictEqual(edit([], "a", 2), { type: "edit", changes: [], path: "a", version: 2 });
  console.log("node sdk ok");
  process.exit(0);
}, 300);
