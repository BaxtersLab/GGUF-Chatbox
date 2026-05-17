Integration Checklist — gguf chatbox configurator & vision adapter
===============================================================

Quick checklist to validate the refactor and integration:

- [ ] `tools/migrate_configurator.py` executed and `backend/configurator/` populated.
- [ ] `open_configurator_cli.py` start/status/stop works for the internal module when invoked from `gguf chatbox`.
- [ ] `cmd_open_configurator` Tauri command updated (or dual-mode) and tested via UI.
- [ ] Vision slot: upload → saved to `vens/uploads/` and `cmd_launch_bsr` launches BSR.
- [ ] Vision action buttons trigger `cmd_vision_action` and return structured JSON.
- [ ] Rollback script restores `C:\Users\Baxter\.continue\config.yaml` from backups.
- [ ] Security review: ensure uploads folder has sensible perms and files are not served publicly.
- [ ] Add unit tests for Python CLI wrappers and small integration smoke tests.

Run notes:
- Use `tools/run_integration_tests.py` to run the pytest skeleton.
