# A6 Tool Protocol

The contract between **whatever drives Agent 6** (the GGUF Chatbox coordinator, a
test script, eventually A6's own model output) and this extension's **hands**
inside VSCodium. This is the single source of truth for two consumers:

1. the **coordinator** (lives in GGUF Chatbox) that turns A6's tool-call into an
   inbox file and reads the result back, and
2. **A6's SOP / header text**, so the agent *knows the tool exists and how to
   call it* (a tool the model isn't told about can never be used).

## The channel (file drop)

A bridge directory with three sub-folders:

```
<bridgeDir>/
  inbox/       drop a command here  (one .json file per command)
  outbox/      the result appears here as <id>.json
  processed/   the extension moves each handled command here
  a6_tools.log append-only activity log
```

`<bridgeDir>` resolution order: `A6_TOOLS_DIR` env var → the `a6Tools.bridgeDir`
setting → default `~/.gguf-chatbox/a6_tools` (under GGUF Chatbox's home, because
the coordinator lives in GGUF Chatbox — no Master Widget dependency).

The extension polls `inbox/` (default 500 ms) and only processes a file once its
size is **stable across two polls** (a write-complete gate, so a half-written
drop is never read). Writers should still write atomically where possible
(write to `name.tmp`, then rename to `name.json`). A leading UTF-8 BOM is
tolerated.

## Command (inbox → )

A single JSON object in a `.json` file:

```json
{ "id": "any-string", "op": "create_folder", "args": { "path": "src/models" } }
```

- `id` *(optional)* — echoed back on the result so the caller can correlate.
  If omitted, the extension synthesizes one; if the JSON can't be parsed, the
  file's basename is used as the id on the error result.
- `op` *(required)* — one of the operations below.
- `args` *(object)* — per-op arguments.

## Result ( → outbox)

Written to `outbox/<id>.json`, published atomically:

```json
{ "id": "…", "op": "create_folder", "ok": true,  "result": { … }, "ts": 1710000000000 }
{ "id": "…", "op": "create_folder", "ok": false, "error": "…",    "ts": 1710000000000 }
```

## Path rules

- A **relative** `path` resolves against the **first workspace folder**.
- An **absolute** `path` is used as-is.
- File/folder ops (`create_folder`, `create_file`, `read_file`, `list_dir`) are
  **contained to the workspace**: a path that escapes via `..` or an absolute
  path outside the workspace is rejected (`ok:false`). Bringing another folder
  under A6's reach is an explicit `add_workspace_folder`.

## Operations

| op | args | result | notes |
|---|---|---|---|
| `create_folder` | `path` | `{path}` | recursive; contained |
| `create_file` | `path`, `content` (string) | `{path, bytes}` | overwrites; contained |
| `read_file` | `path` | `{path, content}` | UTF-8; contained |
| `list_dir` | `path` (default `.`) | `{path, entries:[{name,type}]}` | `type` = dir\|file\|other; contained |
| `add_workspace_folder` | `path` | `{path, added}` | any path |
| `remove_workspace_folder` | `path` | `{path, removed, reason?}` | any path |
| `list_workspace_folders` | — | `{folders:[…]}` | any path |

Ops needing a relative path but with no workspace folder open fail with a clear
error rather than guessing.

## Agent-facing emission — the `a6-tool` fenced block (for A6's SOP)

A6 doesn't write to the inbox itself. It emits a **fenced block** tagged
`a6-tool` in its reply; GGUF Chatbox's `:8080` proxy detects it, dispatches the
`vscodium_workspace` tool (which writes the inbox command, waits for the outbox,
and returns the result), feeds the result back into A6's context as a follow-up
turn, and re-queries — the same loop it uses for native OpenAI tool calls. This
is **Path B** (config-independent): it works even though llama-server runs
without `--jinja`, so A6's model never needs to emit native `tool_calls`.

The block body is the tool arguments **flat** (op + path/content at top level),
matching `vscodium_workspace`'s schema. Teach A6's SOP to emit, e.g.:

~~~
```a6-tool
{ "op": "create_folder", "path": "src/models" }
```
~~~

An optional `"tool"` field names the tool (default `vscodium_workspace`), leaving
room for more A6 tools later. Built + verified in GGUF Chatbox:
`crates/server/src/proxy.rs` (`extract_a6_tool` + the loop) and
`crates/tool_belt/src/vscodium_ops.rs` (`VscodiumTool`). **Still to do:** teach A6
the block format in its SOP (the `_local_agent_header` / SOP text on the SOC
side) so it actually knows to emit it.

## Adding an operation

1. Add it to `OPS` in `lib/commands.js` (set `needsWorkspace` / `contained`).
2. Add a `case` in `dispatch()` in `extension.js` using the `vscode` API.
3. Add a row to the table above and a unit test in `test/commands.test.js`.
