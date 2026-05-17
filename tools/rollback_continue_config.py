#!/usr/bin/env python3
"""Safe rollback helper for Continue `config.yaml`.

This script finds the most recent `config.yaml.bak-*` in the
`vens/onboarding-reports/backups/` folder and can restore it to
`~/.continue/config.yaml` when run with --confirm.

Usage:
  rollback_continue_config.py          # show candidate backup
  rollback_continue_config.py --confirm  # perform restore
"""
import os
import sys
import shutil
import glob
from pathlib import Path

BACKUPS_DIR = Path.home() / ".continue" / "backups"
LIVE_CONFIG = Path.home() / ".continue" / "config.yaml"

def find_latest_backup():
    if not BACKUPS_DIR.exists():
        return None
    patterns = list(BACKUPS_DIR.rglob('config.yaml.bak*'))
    if not patterns:
        return None
    patterns.sort(key=lambda p: p.stat().st_mtime, reverse=True)
    return patterns[0]

def show():
    b = find_latest_backup()
    if not b:
        print('No backup files found in', BACKUPS_DIR)
        return 1
    print('Latest backup candidate:', b)
    print('Live config path:', LIVE_CONFIG)
    print('\nTo restore run with --confirm')
    return 0

def restore():
    b = find_latest_backup()
    if not b:
        print('No backup files found; aborting')
        return 2
    if not LIVE_CONFIG.parent.exists():
        try:
            LIVE_CONFIG.parent.mkdir(parents=True, exist_ok=True)
        except Exception as e:
            print('Failed to create directory for live config:', e)
            return 3
    # create a timestamped copy of current live config (if exists)
    if LIVE_CONFIG.exists():
        bak = LIVE_CONFIG.parent.joinpath('config.yaml.pre-rollback')
        try:
            shutil.copy2(LIVE_CONFIG, bak)
            print('Saved existing live config to', bak)
        except Exception as e:
            print('Failed to back up live config:', e)
            return 4
    try:
        shutil.copy2(b, LIVE_CONFIG)
        print('Restored', b, '->', LIVE_CONFIG)
        return 0
    except Exception as e:
        print('Failed to restore:', e)
        return 5

if __name__ == '__main__':
    confirm = '--confirm' in sys.argv
    if not confirm:
        sys.exit(show())
    sys.exit(restore())
