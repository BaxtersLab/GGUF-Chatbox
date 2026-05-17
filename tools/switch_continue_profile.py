#!/usr/bin/env python3
"""
Safe helper to switch the active profile in ~/.continue/config.yaml and
copy the selected profile into the `models` array so continue.dev sees it.

Usage:
  python tools/switch_continue_profile.py --config "%USERPROFILE%\\.continue\\config.yaml" --profile Qwen3-8B

Requires: pyyaml (`pip install pyyaml`)
"""
import argparse
import shutil
from pathlib import Path

import yaml


def load(path: Path):
    with path.open("r", encoding="utf-8") as f:
        return yaml.safe_load(f)


def atomic_write(path: Path, data: str):
    tmp = path.with_suffix(path.suffix + ".tmp")
    with tmp.open("w", encoding="utf-8") as f:
        f.write(data)
    shutil.move(str(tmp), str(path))


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--config", required=True)
    p.add_argument("--profile", required=True)
    p.add_argument("--mcp-memory", action="store_true", help="Write an MCP memory file for the applied profile into the workspace memories/repo folder")
    args = p.parse_args()

    cfg_path = Path(args.config).expanduser()
    if not cfg_path.exists():
        raise SystemExit(f"config not found: {cfg_path}")

    cfg = load(cfg_path)
    profiles = cfg.get("profiles", {})
    if args.profile not in profiles:
        raise SystemExit(f"profile not found: {args.profile}")

    selected = profiles[args.profile]

    # Update active_profile
    cfg["active_profile"] = args.profile

    # Ensure models key exists and replace first entry with selected profile
    cfg_models = cfg.get("models", [])
    entry = {
        "name": args.profile,
        "provider": selected.get("provider", "openai"),
        "apiBase": selected.get("apiBase", ""),
        "apiKey": selected.get("apiKey", ""),
        "model": selected.get("model", ""),
        # pass through optional higher-level config fields
        "context": selected.get("context", {}),
        "rules": selected.get("rules", {}),
        "prompts": selected.get("prompts", {}),
        "docs": selected.get("docs", {}),
        "mcpServers": selected.get("mcpServers", {}),
        "data": selected.get("data", {}),
    }
    if cfg_models:
        cfg["models"][0] = entry
    else:
        cfg["models"] = [entry]

    # Write back atomically
    out = yaml.safe_dump(cfg, sort_keys=False)
    atomic_write(cfg_path, out)
    print(f"Switched active_profile -> {args.profile}")

    # Optionally write an MCP memory file using the Vens mempalace adapter
    if args.mcp_memory:
        try:
            from vens.mempalace_adapter import save_profile_memory

            script_root = Path(__file__).resolve().parents[1]
            mem_dir = script_root.joinpath("memories", "repo")
            mem_dir.mkdir(parents=True, exist_ok=True)
            mem_file = save_profile_memory(selected, out_dir=mem_dir)
            print(f"Wrote MCP memory file: {mem_file}")
        except Exception as e:
            print(f"Warning: failed to write MCP memory file: {e}")


if __name__ == "__main__":
    main()
