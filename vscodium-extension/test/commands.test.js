"use strict";
// Unit tests for the pure command logic (no vscode host needed).
// Run:  node --test    (from the extension folder)

const { test } = require("node:test");
const assert = require("node:assert");
const path = require("node:path");

const { parseCommand, opInfo, resolvePath, isContained, OPS } = require("../lib/commands");

test("parseCommand accepts a valid command and echoes id", () => {
  const c = parseCommand('{"id":"x1","op":"create_folder","args":{"path":"sub"}}');
  assert.strictEqual(c.id, "x1");
  assert.strictEqual(c.op, "create_folder");
  assert.deepStrictEqual(c.args, { path: "sub" });
});

test("parseCommand synthesizes an id when missing", () => {
  const c = parseCommand('{"op":"list_workspace_folders"}');
  assert.match(c.id, /^cmd-/);
  assert.deepStrictEqual(c.args, {});
});

test("parseCommand rejects invalid JSON", () => {
  assert.throws(() => parseCommand("{not json"), /invalid JSON/);
});

test("parseCommand tolerates a leading UTF-8 BOM", () => {
  // PowerShell's Set-Content -Encoding utf8, some editors, and the future
  // coordinator can prepend a BOM; JSON.parse rejects it, so we strip it.
  const c = parseCommand("﻿" + '{"op":"create_folder","args":{"path":"x"}}');
  assert.strictEqual(c.op, "create_folder");
});

test("parseCommand rejects a non-object top level", () => {
  assert.throws(() => parseCommand("[1,2,3]"), /must be a JSON object/);
});

test("parseCommand rejects an unknown op", () => {
  assert.throws(() => parseCommand('{"op":"rm_rf_everything"}'), /unknown or missing op/);
});

test("parseCommand rejects a missing op", () => {
  assert.throws(() => parseCommand('{"args":{}}'), /unknown or missing op/);
});

test("parseCommand rejects non-object args", () => {
  assert.throws(() => parseCommand('{"op":"create_folder","args":[1]}'), /args must be an object/);
});

test("opInfo reports workspace/containment requirements", () => {
  assert.strictEqual(opInfo("create_file").needsWorkspace, true);
  assert.strictEqual(opInfo("create_file").contained, true);
  assert.strictEqual(opInfo("add_workspace_folder").contained, false);
  assert.strictEqual(opInfo("nope"), undefined);
});

test("every advertised op has metadata", () => {
  for (const op of Object.keys(OPS)) {
    assert.ok(opInfo(op), "missing metadata for " + op);
  }
});

test("resolvePath joins relative paths onto the base", () => {
  const base = path.resolve("/tmp/ws");
  assert.strictEqual(resolvePath(base, "a/b"), path.resolve(base, "a/b"));
});

test("resolvePath uses an absolute path as-is", () => {
  const base = path.resolve("/tmp/ws");
  const abs = path.resolve("/somewhere/else");
  assert.strictEqual(resolvePath(base, abs), abs);
});

test("resolvePath requires a non-empty path", () => {
  assert.throws(() => resolvePath("/tmp/ws", ""), /path is required/);
  assert.throws(() => resolvePath("/tmp/ws", null), /path is required/);
});

test("isContained is true for a child and the base itself", () => {
  const base = path.resolve("/tmp/ws");
  assert.ok(isContained(base, path.resolve(base, "a/b")));
  assert.ok(isContained(base, base));
});

test("isContained is false for a parent-escape or sibling", () => {
  const base = path.resolve("/tmp/ws");
  assert.ok(!isContained(base, path.resolve(base, "../evil")));
  assert.ok(!isContained(base, path.resolve("/tmp/ws-sibling")));
  assert.ok(!isContained("", path.resolve("/anything")));
});

test("isContained blocks the classic prefix-not-boundary trick", () => {
  // /tmp/ws must NOT contain /tmp/wsX — only /tmp/ws + separator counts.
  const base = path.resolve("/tmp/ws");
  assert.ok(!isContained(base, path.resolve("/tmp/wsX/file")));
});
