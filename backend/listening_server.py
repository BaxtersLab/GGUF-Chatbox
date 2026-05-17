"""
GGUF Chatbox — Listening Server  (port 8083)
=============================================
A lightweight HTTP server that provides audio intelligence endpoints
called by the Listening expansion slot (slot 8) in the frontend.

Endpoints
---------
GET  /health          → {"ok": true, "version": "..."}
POST /action          → {"action": "generate_tags"|"transcribe"|"review",
                          "file_path": "/abs/path/to/audio.mp3"}

Optional at startup
-------------------
--port <N>            bind port (default 8083)
--model-path <path>   path to musicnn .h5 weights or AcousticBrainz
                      TF SavedModel directory (optional — used by
                      generate_tags when available)

Dependencies (all optional — server degrades gracefully if absent)
------------------------------------------------------------------
mutagen               audio tag reading
musicnn               deep-learning genre/mood tagging (needs TF/PyTorch)
openai-whisper        transcription  (CLI: whisper <file>)
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import threading
import traceback
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from typing import Any

__version__ = "1.0.0"

# ---------------------------------------------------------------------------
# Globals
# ---------------------------------------------------------------------------

MODEL_PATH: str | None = None   # set from --model-path at startup
_musicnn_loaded = False
_musicnn_error: str | None = None


# ---------------------------------------------------------------------------
# Optional import helpers
# ---------------------------------------------------------------------------

def _try_import_mutagen():
    try:
        import mutagen  # noqa: F401
        return True
    except ImportError:
        return False


def _try_import_musicnn():
    global _musicnn_loaded, _musicnn_error
    try:
        from musicnn.extractor import extractor  # noqa: F401
        _musicnn_loaded = True
    except Exception as e:
        _musicnn_error = str(e)


# Pre-check availability
_HAS_MUTAGEN = _try_import_mutagen()


# ---------------------------------------------------------------------------
# Audio tag reading (mutagen)
# ---------------------------------------------------------------------------

def _read_tags(file_path: str) -> dict[str, Any]:
    """Return a dict of human-readable audio tags using mutagen."""
    if not _HAS_MUTAGEN:
        return {"error": "mutagen not installed — run: pip install mutagen"}

    from mutagen import File as MutagenFile
    from mutagen.easyid3 import EasyID3
    from mutagen.mp3 import MP3

    tags: dict[str, Any] = {}
    try:
        af = MutagenFile(file_path, easy=True)
        if af is None:
            return {"error": f"mutagen could not parse: {file_path}"}
        if af.tags:
            for k, v in af.tags.items():
                tags[k] = v[0] if isinstance(v, list) and len(v) == 1 else v
        # Duration
        if hasattr(af, "info") and hasattr(af.info, "length"):
            length = af.info.length
            tags["duration_sec"] = round(length, 2)
            m, s = divmod(int(length), 60)
            tags["duration_human"] = f"{m}:{s:02d}"
        # Bitrate
        if hasattr(af, "info") and hasattr(af.info, "bitrate"):
            tags["bitrate_kbps"] = af.info.bitrate // 1000
        # Sample rate
        if hasattr(af, "info") and hasattr(af.info, "sample_rate"):
            tags["sample_rate_hz"] = af.info.sample_rate
        # Channels
        if hasattr(af, "info") and hasattr(af.info, "channels"):
            tags["channels"] = af.info.channels
    except Exception as exc:
        tags["error"] = str(exc)

    return tags


# ---------------------------------------------------------------------------
# musicnn genre/mood tagging
# ---------------------------------------------------------------------------

def _musicnn_tags(file_path: str, model_path: str | None) -> dict[str, Any]:
    """Run musicnn extractor on the audio file and return top tags."""
    try:
        from musicnn.extractor import extractor

        # musicnn supports MSD-MusicCNN and MTT-MusicCNN models out of the box.
        # If a custom model path is given, use it; otherwise use the default.
        if model_path and Path(model_path).exists():
            taggram, tags, features = extractor(
                file_path, model=model_path, input_length=3, input_overlap=False, extract_features=True
            )
        else:
            taggram, tags, features = extractor(
                file_path, model="MSD-MusicCNN", input_length=3, input_overlap=False, extract_features=True
            )

        import numpy as np
        mean_activations = taggram.mean(axis=0)
        top_indices = mean_activations.argsort()[::-1][:10]
        top_tags = [
            {"tag": tags[i], "score": float(round(mean_activations[i], 4))}
            for i in top_indices
        ]
        return {"source": "musicnn", "top_tags": top_tags}

    except ImportError:
        return {"source": "musicnn", "error": "musicnn not installed — run: pip install musicnn"}
    except Exception as exc:
        return {"source": "musicnn", "error": str(exc)}


# ---------------------------------------------------------------------------
# Whisper transcription
# ---------------------------------------------------------------------------

def _whisper_transcribe(file_path: str) -> dict[str, Any]:
    """Transcribe audio using openai-whisper (Python API) or whisper CLI."""
    # Try Python API first
    try:
        import whisper
        model = whisper.load_model("base")
        result = model.transcribe(file_path)
        return {
            "source": "whisper-python",
            "language": result.get("language", "unknown"),
            "text": result.get("text", "").strip(),
        }
    except ImportError:
        pass
    except Exception as exc:
        return {"source": "whisper-python", "error": str(exc)}

    # Fall back to whisper CLI
    try:
        result = subprocess.run(
            ["whisper", file_path, "--output_format", "json", "--output_dir", "/tmp"],
            capture_output=True, text=True, timeout=300
        )
        if result.returncode == 0:
            # whisper writes <filename>.json to output_dir
            json_out = Path("/tmp") / (Path(file_path).stem + ".json")
            if json_out.exists():
                data = json.loads(json_out.read_text(encoding="utf-8"))
                return {"source": "whisper-cli", "text": data.get("text", "").strip()}
        return {"source": "whisper-cli", "error": result.stderr or "non-zero exit"}
    except FileNotFoundError:
        return {
            "source": "whisper",
            "error": "Neither openai-whisper Python package nor whisper CLI found.\n"
                     "Install with: pip install openai-whisper",
        }
    except subprocess.TimeoutExpired:
        return {"source": "whisper-cli", "error": "Transcription timed out (>5 min)"}
    except Exception as exc:
        return {"source": "whisper-cli", "error": str(exc)}


# ---------------------------------------------------------------------------
# File review (comprehensive metadata)
# ---------------------------------------------------------------------------

def _review_audio(file_path: str, model_path: str | None) -> dict[str, Any]:
    """Collect all available metadata about the audio file."""
    p = Path(file_path)
    review: dict[str, Any] = {
        "file": {
            "name": p.name,
            "extension": p.suffix.lower(),
            "size_bytes": p.stat().st_size,
            "size_human": _human_size(p.stat().st_size),
            "path": str(p),
        }
    }

    # Audio tags
    review["tags"] = _read_tags(file_path)

    # musicnn (if loaded / available)
    if _HAS_MUTAGEN:
        mn = _musicnn_tags(file_path, model_path)
        review["ai_tags"] = mn
    else:
        review["ai_tags"] = {"error": "musicnn requires mutagen; install: pip install mutagen musicnn"}

    return review


def _human_size(n: int) -> str:
    for unit in ("B", "KB", "MB", "GB"):
        if n < 1024:
            return f"{n:.1f} {unit}"
        n //= 1024
    return f"{n:.1f} TB"


# ---------------------------------------------------------------------------
# HTTP handler
# ---------------------------------------------------------------------------

class ListeningHandler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        # Redirect to stderr so Rust can capture it
        print(f"[listening] {self.address_string()} - {fmt % args}", file=sys.stderr, flush=True)

    def _send_json(self, data: Any, code: int = 200):
        body = json.dumps(data, ensure_ascii=False, indent=2).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _send_error_json(self, msg: str, code: int = 400):
        self._send_json({"ok": False, "error": msg}, code=code)

    def do_GET(self):
        if self.path == "/health":
            self._send_json({
                "ok": True,
                "version": __version__,
                "mutagen": _HAS_MUTAGEN,
                "musicnn": _musicnn_loaded,
                "musicnn_error": _musicnn_error,
                "model_path": MODEL_PATH,
            })
        else:
            self._send_error_json(f"Unknown endpoint: {self.path}", code=404)

    def do_POST(self):
        if self.path != "/action":
            self._send_error_json(f"Unknown endpoint: {self.path}", code=404)
            return

        # Read body
        try:
            length = int(self.headers.get("Content-Length", 0))
            raw = self.rfile.read(length)
            req = json.loads(raw)
        except Exception as exc:
            self._send_error_json(f"Bad request body: {exc}")
            return

        action = req.get("action", "")
        file_path = req.get("file_path", "")

        # Validate
        if action not in ("generate_tags", "transcribe", "review"):
            self._send_error_json(f"Unknown action: {action!r}")
            return
        if not file_path:
            self._send_error_json("file_path is required")
            return
        if not os.path.isabs(file_path):
            self._send_error_json("file_path must be an absolute path")
            return
        if not os.path.exists(file_path):
            self._send_error_json(f"File not found: {file_path}", code=404)
            return

        try:
            if action == "generate_tags":
                tags = _read_tags(file_path)
                ai = _musicnn_tags(file_path, MODEL_PATH)
                result = {"ok": True, "action": action, "file": file_path,
                          "tags": tags, "ai_tags": ai}

            elif action == "transcribe":
                result = {"ok": True, "action": action, "file": file_path,
                          "transcription": _whisper_transcribe(file_path)}

            elif action == "review":
                result = {"ok": True, "action": action,
                          "review": _review_audio(file_path, MODEL_PATH)}

            self._send_json(result)

        except Exception as exc:
            print(traceback.format_exc(), file=sys.stderr, flush=True)
            self._send_error_json(f"Internal error: {exc}", code=500)


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

def main():
    global MODEL_PATH

    parser = argparse.ArgumentParser(description="GGUF Chatbox Listening Server")
    parser.add_argument("--port", type=int, default=8083)
    parser.add_argument("--model-path", default=None,
                        help="Path to musicnn weights or AcousticBrainz TF SavedModel directory")
    args = parser.parse_args()

    MODEL_PATH = args.model_path

    # Try to pre-load musicnn in a background thread (non-blocking startup)
    if MODEL_PATH:
        threading.Thread(target=_try_import_musicnn, daemon=True).start()

    server = HTTPServer(("127.0.0.1", args.port), ListeningHandler)
    print(f"[listening] Listening server v{__version__} on port {args.port}", file=sys.stderr, flush=True)
    if MODEL_PATH:
        print(f"[listening] Model path: {MODEL_PATH}", file=sys.stderr, flush=True)
    print(f"[listening] mutagen available: {_HAS_MUTAGEN}", file=sys.stderr, flush=True)
    print(f"[listening] Endpoints: GET /health  POST /action", file=sys.stderr, flush=True)

    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("[listening] Shutting down.", file=sys.stderr, flush=True)
        server.shutdown()


if __name__ == "__main__":
    main()
