#!/usr/bin/env python3
"""CLI wrapper to start/stop/status the Vens configurator GUI.

Usage:
  open_configurator_cli.py start
  open_configurator_cli.py status
  open_configurator_cli.py stop

This script mirrors the Tauri command behavior but is useful for CLI testing.
It writes a PID file to the `vens` folder when starting.
"""
import sys
import os
import subprocess
import time

_SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
VENS_PATH = os.path.join(_SCRIPT_DIR, "..", "backend", "configurator")
PID_FILE = os.path.join(VENS_PATH, "configurator.pid")

def find_python():
    venv = os.path.join(_SCRIPT_DIR, "..", ".venv", "Scripts", "python.exe")
    if os.path.exists(venv):
        return venv
    return "python"

def start():
    if not os.path.isdir(VENS_PATH):
        print("Vens path not found:", VENS_PATH)
        return 2
    if os.path.exists(PID_FILE):
        print("PID file exists; configurator may already be running.")
        return 1
    py = find_python()
    cmd = [py, "-m", "vens.configurator.gui"]
    try:
        # Detached spawn
        p = subprocess.Popen(cmd, cwd=VENS_PATH, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, close_fds=True)
        pid = p.pid
        with open(PID_FILE, 'w') as f:
            f.write(str(pid))
        print("Started configurator, pid", pid)
        return 0
    except Exception as e:
        print("Failed to start configurator:", e)
        return 3

def status():
    if not os.path.exists(PID_FILE):
        print("No PID file; configurator not started via this CLI.")
        return 1
    try:
        with open(PID_FILE, 'r') as f:
            pid = int(f.read().strip())
    except Exception:
        print("PID file invalid")
        return 2
    # Check process exists
    try:
        os.kill(pid, 0)
        print("Configurator running, pid", pid)
        return 0
    except Exception:
        print("No process with pid", pid)
        return 3

def stop():
    if not os.path.exists(PID_FILE):
        print("No PID file; nothing to stop.")
        return 1
    try:
        with open(PID_FILE, 'r') as f:
            pid = int(f.read().strip())
    except Exception:
        print("PID file invalid")
        return 2
    try:
        os.kill(pid, 15)
        # give it a moment
        time.sleep(0.5)
    except Exception as e:
        print("Warning: could not signal pid", pid, "-", e)
    try:
        os.remove(PID_FILE)
    except Exception:
        pass
    print("Stopped configurator (requested)")
    return 0

if __name__ == '__main__':
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(2)
    cmd = sys.argv[1].lower()
    if cmd == 'start':
        sys.exit(start())
    if cmd == 'status':
        sys.exit(status())
    if cmd == 'stop':
        sys.exit(stop())
    print('Unknown command:', cmd)
    sys.exit(2)
