Configurator CLI and Rollback Helper
===================================

Files:
- `open_configurator_cli.py` — small CLI to start/status/stop the Python configurator GUI. Writes `vens/configurator.pid` when starting.
- `rollback_continue_config.py` — safe helper to restore the latest `config.yaml.bak-*` from `~/.continue/backups/` to `~/.continue/config.yaml`. Requires `--confirm` to perform changes.

Usage examples (PowerShell):

Start the configurator (uses workspace venv python if present):
```powershell
python .\tools\open_configurator_cli.py start
```

Check status:
```powershell
python .\tools\open_configurator_cli.py status
```

Stop the configurator (asks the recorded PID to terminate):
```powershell
python .\tools\open_configurator_cli.py stop
```

Preview rollback candidate:
```powershell
python .\tools\rollback_continue_config.py
```

Perform rollback (restores latest backup to `~/.continue/config.yaml`):
```powershell
python .\tools\rollback_continue_config.py --confirm
```

Notes:
- These scripts are lightweight helpers for local development and assume the filesystem paths used in this workspace. Edit the constants at the top of the scripts if your setup differs.
- The rollback script makes a `config.yaml.pre-rollback` copy of the existing live config before overwriting.
