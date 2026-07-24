# A6 VSCodium Tools

The **hands** for SOC **Agent 6** — a VSCodium extension that lets A6 act on a
workspace (create/read/write files and folders, manage workspace folders)
through **VSCodium's own extension API**, driven by a simple file-drop channel.

A6 already thinks and talks (it's a local CD-changer agent served through the
GGUF Chatbox / llama.cpp harness). What it lacked was a way to *act*. This
extension is that missing piece — and only that piece: it executes commands
deterministically and reports back. The **brain** (A6's model, or Agent 1 via a
cross-hemisphere volley) decides *what* to do; this just does it. That's the
"thin executor first" plan — prove the tool rail before layering intelligence.

## How it fits

```
 A6 (model, in GGUF Chatbox) ──emit tool-call──▶ coordinator (in GGUF Chatbox)
                                                        │ writes inbox/<id>.json
                                                        ▼
                                          THIS EXTENSION (in A6's VSCodium)
                                                        │ runs it via vscode API
                                                        ▼
                                          outbox/<id>.json ──▶ back to A6
```

- The **coordinator lives in GGUF Chatbox** (not the Master Widget) so A6's tools
  work wherever GGUF Chatbox is — no widget dependency. This extension doesn't
  care who fills its inbox, so that split needs no change here.
- The command/result contract is in **[PROTOCOL.md](PROTOCOL.md)** — the same
  spec the coordinator implements and A6's SOP teaches.

## Two VSCodium paths (kept separate on purpose)

- **Human / GUI** — the Master Widget's "Start VSCodium" (full editor), for
  Blind Box, Key Assistant, manual work.
- **Agent** — A6's *own* VSCodium instance, launched by `launch_a6_vscodium.bat`
  with an **isolated profile** (`--user-data-dir`) so it never collides with the
  GUI instance's single-instance lock. This is the autonomous rail.

## Run it

```bat
launch_a6_vscodium.bat        REM opens A6's VSCodium on ../../A6_workspace with this extension loaded
```

Then drop a command (see PROTOCOL.md), e.g. `~/.gguf-chatbox/a6_tools/inbox/x.json`:

```json
{ "op": "create_folder", "args": { "path": "src/models" } }
```

Override with env vars for a throwaway run: `A6_TOOLS_DIR`, `A6_PROFILE`,
`A6_WORKSPACE`.

### Launch notes (learned the hard way)

- Launch via the **CLI wrapper** `bin/codium.cmd` (or the `.bat`, which does), not
  `VSCodium.exe` directly: this build honors `--user-data-dir` only in the
  **space form** (`--user-data-dir "path"`), not `--user-data-dir=path`. The `=`
  form silently falls back to the default profile and forwards to any running
  instance.

## Tests

```
node --test        # pure command/path logic — no VSCodium needed
```

The tool execution itself is verified end-to-end by launching an isolated
VSCodium dev-host, dropping commands, and confirming the files/folders appear and
containment is enforced.

## Files

| file | role |
|---|---|
| `extension.js` | activation, the inbox poller, and the `vscode`-API tool dispatch |
| `lib/commands.js` | pure parse / validate / path-resolve logic (no `vscode`) |
| `test/commands.test.js` | unit tests for the pure logic |
| `launch_a6_vscodium.bat` | launch A6's isolated VSCodium with this extension |
| `PROTOCOL.md` | the command/result contract |
| `../../A6_workspace` | A6's default workspace (in the file-cabinet hub) |
| `../crates/tool_belt/src/vscodium_ops.rs` | the Rust half — GGUF Chatbox's `VscodiumTool` that fills this extension's inbox |

Status: **thin-executor MVP, tool rail verified.** Next: the GGUF Chatbox
coordinator + teaching A6 the protocol in its SOP.
