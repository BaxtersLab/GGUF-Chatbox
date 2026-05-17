#!/usr/bin/env python3
"""Run the integration test suite (pytest).

Usage: python tools/run_integration_tests.py
"""
import subprocess
import sys

def main():
    cmd = [sys.executable, '-m', 'pytest', 'tests', '-q']
    p = subprocess.run(cmd)
    return p.returncode

if __name__ == '__main__':
    raise SystemExit(main())
