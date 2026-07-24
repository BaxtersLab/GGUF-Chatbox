"use strict";
// Pure command logic for the A6 VSCodium tool bridge.
// Deliberately imports NOTHING from `vscode` so it runs under plain Node and is
// unit-testable without an extension host. extension.js owns the side effects
// (the actual vscode.workspace.fs / updateWorkspaceFolders calls); this module
// only parses, validates, and resolves paths.

const path = require("path");

// The tool surface A6 can call. Kept small on purpose (thin executor first):
// workspace-focused file/folder ops that map 1:1 onto stable vscode APIs.
const OPS = Object.freeze({
  create_folder: { needsWorkspace: true, contained: true },
  create_file: { needsWorkspace: true, contained: true },
  read_file: { needsWorkspace: true, contained: true },
  list_dir: { needsWorkspace: true, contained: true },
  add_workspace_folder: { needsWorkspace: false, contained: false },
  remove_workspace_folder: { needsWorkspace: false, contained: false },
  list_workspace_folders: { needsWorkspace: false, contained: false },
});

/**
 * Parse+validate a raw command payload (the text of an inbox .json file).
 * Returns a normalized { id, op, args }. Throws Error on anything malformed —
 * callers turn the throw into an outbox error result.
 */
function parseCommand(text) {
  let obj;
  // Tolerate a leading UTF-8 BOM: tools that write the command file (PowerShell's
  // Set-Content -Encoding utf8, some editors, the future coordinator) may prepend
  // one, and JSON.parse rejects it. Strip it before parsing.
  if (typeof text === "string") text = text.replace(/^﻿/, "");
  try {
    obj = JSON.parse(text);
  } catch (e) {
    throw new Error("invalid JSON: " + e.message);
  }
  if (!obj || typeof obj !== "object" || Array.isArray(obj)) {
    throw new Error("command must be a JSON object");
  }
  const op = obj.op;
  if (typeof op !== "string" || !Object.prototype.hasOwnProperty.call(OPS, op)) {
    throw new Error("unknown or missing op: " + JSON.stringify(op));
  }
  const args = obj.args == null ? {} : obj.args;
  if (typeof args !== "object" || Array.isArray(args)) {
    throw new Error("args must be an object");
  }
  // id is echoed back so the caller can correlate the reply; synthesize a
  // stable-ish one when absent so a missing id never blocks execution.
  const id =
    obj.id != null ? String(obj.id) : "cmd-" + Date.now() + "-" + Math.floor(Math.random() * 1e6);
  return { id, op, args };
}

/** Op metadata (or undefined) — lets extension.js branch without re-hardcoding. */
function opInfo(op) {
  return OPS[op];
}

/**
 * Resolve a user-supplied path against a workspace base. Relative paths join
 * onto base; absolute paths are used as-is (path.resolve handles both). Pure.
 */
function resolvePath(base, p) {
  if (typeof p !== "string" || p.length === 0) {
    throw new Error("path is required");
  }
  return path.resolve(base || process.cwd(), p);
}

/**
 * True when `abs` is inside `base` (or equal to it). Used to stop workspace
 * file ops from escaping the workspace via .. or an absolute path. Pure.
 */
function isContained(base, abs) {
  if (!base) return false;
  const b = path.resolve(base);
  const a = path.resolve(abs);
  if (a === b) return true;
  const withSep = b.endsWith(path.sep) ? b : b + path.sep;
  return a.startsWith(withSep);
}

module.exports = { OPS, parseCommand, opInfo, resolvePath, isContained };
