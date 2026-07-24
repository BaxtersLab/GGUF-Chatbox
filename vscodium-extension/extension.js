"use strict";
// A6 VSCodium Tools — the "hands" for SOC Agent 6.
//
// A6 already thinks and talks (it's a local CD-changer agent served through the
// GGUF Chatbox / llama.cpp harness); what it lacked was a way to ACT on the
// workspace. This extension gives it that: commands arrive as JSON file drops in
// a watched inbox, are executed through VSCodium's OWN extension API (so the work
// really goes through VSCodium's tools), and results are written to an outbox —
// the same file-drop idiom the SOC bridge already uses for A5/A6/A7 replies.
//
// Thin executor first: no reasoning here. The brain (A6's model, or Agent 1 via a
// cross-hemisphere volley) decides WHAT to do and drops the command; this just
// executes it deterministically and reports back.

const vscode = require("vscode");
const fs = require("fs");
const path = require("path");
const os = require("os");

const { parseCommand, opInfo, resolvePath, isContained } = require("./lib/commands");

let poller = null;
let output = null;
let dirs = null;
const seen = new Map(); // inbox filename -> last observed size (write-complete gate)

function resolveBridgeDir() {
  const envDir = process.env.A6_TOOLS_DIR;
  if (envDir && envDir.trim()) return envDir.trim();
  const cfg = vscode.workspace.getConfiguration("a6Tools").get("bridgeDir");
  if (cfg && String(cfg).trim()) return String(cfg).trim();
  // Default under GGUF Chatbox's home: the coordinator that fills this bridge
  // lives in GGUF Chatbox (so A6's tools work wherever GGUF Chatbox is, with no
  // Master Widget dependency). Just a folder — created if missing, needs no
  // GGUF Chatbox install; the env var / setting overrides for standalone use.
  return path.join(os.homedir(), ".gguf-chatbox", "a6_tools");
}

function ensureDirs(root) {
  const d = {
    root,
    inbox: path.join(root, "inbox"),
    outbox: path.join(root, "outbox"),
    processed: path.join(root, "processed"),
    log: path.join(root, "a6_tools.log"),
  };
  for (const p of [d.root, d.inbox, d.outbox, d.processed]) {
    fs.mkdirSync(p, { recursive: true });
  }
  return d;
}

function log(msg) {
  const line = "[" + new Date().toISOString() + "] " + msg;
  try {
    if (output) output.appendLine(line);
  } catch (_) {}
  try {
    if (dirs) fs.appendFileSync(dirs.log, line + "\n");
  } catch (_) {}
}

function firstWorkspaceFolder() {
  const folders = vscode.workspace.workspaceFolders;
  return folders && folders.length ? folders[0].uri.fsPath : null;
}

// ── the actual tool implementations (side effects live here) ─────────────────
async function dispatch(op, args) {
  const info = opInfo(op);
  const base = firstWorkspaceFolder();
  if (info.needsWorkspace && !base) {
    throw new Error("no workspace folder is open — cannot resolve a relative path");
  }

  const resolveContained = (p) => {
    const abs = resolvePath(base, p);
    if (info.contained && !isContained(base, abs)) {
      throw new Error("path escapes the workspace: " + abs);
    }
    return abs;
  };

  switch (op) {
    case "create_folder": {
      const abs = resolveContained(args.path);
      await vscode.workspace.fs.createDirectory(vscode.Uri.file(abs));
      return { path: abs };
    }
    case "create_file": {
      const abs = resolveContained(args.path);
      const content = args.content != null ? String(args.content) : "";
      await vscode.workspace.fs.writeFile(vscode.Uri.file(abs), Buffer.from(content, "utf8"));
      return { path: abs, bytes: Buffer.byteLength(content, "utf8") };
    }
    case "read_file": {
      const abs = resolveContained(args.path);
      const data = await vscode.workspace.fs.readFile(vscode.Uri.file(abs));
      return { path: abs, content: Buffer.from(data).toString("utf8") };
    }
    case "list_dir": {
      const abs = resolveContained(args.path != null ? args.path : ".");
      const entries = await vscode.workspace.fs.readDirectory(vscode.Uri.file(abs));
      return {
        path: abs,
        entries: entries.map(([name, type]) => ({
          name,
          type: type === vscode.FileType.Directory ? "dir" : type === vscode.FileType.File ? "file" : "other",
        })),
      };
    }
    case "add_workspace_folder": {
      const abs = resolvePath(base || os.homedir(), args.path);
      const start = vscode.workspace.workspaceFolders ? vscode.workspace.workspaceFolders.length : 0;
      const ok = vscode.workspace.updateWorkspaceFolders(start, null, { uri: vscode.Uri.file(abs) });
      return { path: abs, added: !!ok };
    }
    case "remove_workspace_folder": {
      const abs = resolvePath(base || os.homedir(), args.path);
      const folders = vscode.workspace.workspaceFolders || [];
      const idx = folders.findIndex((f) => path.resolve(f.uri.fsPath) === path.resolve(abs));
      if (idx < 0) return { path: abs, removed: false, reason: "not in workspace" };
      const ok = vscode.workspace.updateWorkspaceFolders(idx, 1);
      return { path: abs, removed: !!ok };
    }
    case "list_workspace_folders": {
      const folders = vscode.workspace.workspaceFolders || [];
      return { folders: folders.map((f) => f.uri.fsPath) };
    }
    default:
      throw new Error("unhandled op: " + op);
  }
}

function writeResult(id, payload) {
  const file = path.join(dirs.outbox, id + ".json");
  const tmp = file + ".tmp";
  fs.writeFileSync(tmp, JSON.stringify(payload, null, 2), "utf8");
  fs.renameSync(tmp, file); // atomic publish — a reader never sees a partial file
}

async function processInboxFile(name) {
  const src = path.join(dirs.inbox, name);
  let text;
  try {
    text = fs.readFileSync(src, "utf8");
  } catch (e) {
    return; // vanished between scan and read — ignore
  }
  let cmd = null;
  let result;
  try {
    cmd = parseCommand(text);
    log("exec " + cmd.op + " id=" + cmd.id + " args=" + JSON.stringify(cmd.args));
    const data = await dispatch(cmd.op, cmd.args);
    result = { id: cmd.id, op: cmd.op, ok: true, result: data, ts: Date.now() };
    log("ok   " + cmd.op + " id=" + cmd.id);
  } catch (e) {
    const id = cmd ? cmd.id : path.parse(name).name;
    const op = cmd ? cmd.op : null;
    result = { id, op, ok: false, error: String(e && e.message ? e.message : e), ts: Date.now() };
    log("FAIL " + (op || "?") + " id=" + id + " : " + result.error);
  }
  try {
    writeResult(result.id, result);
  } catch (e) {
    log("could not write result for " + result.id + ": " + e.message);
  }
  // Archive the command so it isn't re-run; a name clash just gets a suffix.
  try {
    let dest = path.join(dirs.processed, name);
    if (fs.existsSync(dest)) dest = path.join(dirs.processed, Date.now() + "-" + name);
    fs.renameSync(src, dest);
  } catch (_) {}
  seen.delete(name);
}

function scanOnce() {
  let names;
  try {
    names = fs.readdirSync(dirs.inbox).filter((n) => n.toLowerCase().endsWith(".json"));
  } catch (_) {
    return;
  }
  for (const name of names) {
    let size;
    try {
      size = fs.statSync(path.join(dirs.inbox, name)).size;
    } catch (_) {
      continue;
    }
    const prev = seen.get(name);
    // Only process once the size is stable across two scans and non-empty — the
    // same write-complete gate the SOC bridge uses, so we never read a half-
    // written drop.
    if (prev === size && size > 0) {
      seen.set(name, -1); // mark in-flight so a slow async op isn't double-started
      processInboxFile(name);
    } else if (prev !== -1) {
      seen.set(name, size);
    }
  }
}

function activate(context) {
  output = vscode.window.createOutputChannel("A6 Tools");
  dirs = ensureDirs(resolveBridgeDir());
  const pollMs = Number(vscode.workspace.getConfiguration("a6Tools").get("pollMs")) || 500;
  log("A6 VSCodium Tools active. bridge=" + dirs.root + " poll=" + pollMs + "ms");
  log("workspace=" + (firstWorkspaceFolder() || "(none open)"));

  poller = setInterval(scanOnce, pollMs);
  context.subscriptions.push({ dispose: () => clearInterval(poller) });

  context.subscriptions.push(
    vscode.commands.registerCommand("a6Tools.status", () => {
      const ws = firstWorkspaceFolder() || "(none)";
      vscode.window.showInformationMessage("A6 Tools watching " + dirs.inbox + "  ·  workspace: " + ws);
      output.show(true);
    })
  );
}

function deactivate() {
  if (poller) clearInterval(poller);
}

module.exports = { activate, deactivate };
