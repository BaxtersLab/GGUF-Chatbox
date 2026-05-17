#!/usr/bin/env python3
r"""Migration helper: copy the existing `vens/configurator` into this repo's backend folder.

Usage:
  python tools/migrate_configurator.py [--source DIR] [--target DIR]

Defaults:
  source: (provide via --source)
  target: backend/configurator

This script performs a safe copy and does not remove the source.
r"""
import os
import shutil
import argparse


def main():
    p = argparse.ArgumentParser()
    p.add_argument('--source', required=True, help='Path to the source configurator folder to copy from')
    p.add_argument('--target', default=os.path.join(os.path.dirname(__file__), '..', 'backend', 'configurator'))
    args = p.parse_args()

    src = os.path.abspath(args.source)
    tgt = os.path.abspath(args.target)

    if not os.path.isdir(src):
        print('Source configurator not found:', src)
        return 2

    if os.path.exists(tgt):
        print('Target exists; creating backup of existing target')
        shutil.move(tgt, tgt + '.bak')

    print('Copying', src, '->', tgt)
    shutil.copytree(src, tgt)
    print('Done. Please inspect', tgt)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
