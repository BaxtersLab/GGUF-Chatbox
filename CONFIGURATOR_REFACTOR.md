Configurator Refactor Plan — Move configurator into `gguf chatbox`
===============================================================

Goal
----
Move the Python-based configurator from a separate workspace into `gguf chatbox` so the app is single-source and easier to ship with the chatbox frontend/backend.

High-level steps
----------------
1. Audit the existing configurator to list Python modules, data files, and external deps.
2. Run the provided migration script to copy the configurator into `gguf chatbox/backend/configurator/` (script: `tools/migrate_configurator.py --source <path>`).
3. Verify the workspace venv has the required Python deps; update `pyproject.toml` or `requirements.txt` under `gguf chatbox` as needed.
4. Replace the `cmd_open_configurator` Tauri command to spawn the internal module (or keep both: `internal` / `external` modes behind a setting). Update `src-tauri/src/main.rs` accordingly.
5. Run integration checklist and tests (`tools/run_integration_tests.py` and `tests/`).
6. Remove or archive the original `vens/configurator` only after sign-off and CI passing.

Notes & safety
--------------
- Keep backups of original files; the migration script doesn't delete source by default.
- Prefer feature-flagging the new internal configurator in Tauri until everything is validated.
- Be mindful of GUI toolkits (PyQt/PySide) — bundling with Tauri may require shipping a Python runtime.

Files added to support this refactor:
- `tools/migrate_configurator.py` — copies configurator into `gguf chatbox/backend/configurator/`.
- `tests/integration_checklist.md` — integration checklist and test instructions.
- `tests/test_basic.py` — pytest skeleton that verifies presence of key files and scripts.
- `tools/run_integration_tests.py` — runs the pytest suite (simple wrapper).
