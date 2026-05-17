#!/usr/bin/env python3
"""
Probe local LLM server for an accepted generation payload.
Sends multiple JSON shapes to common endpoints and prints the first successful response.
"""
import json
import urllib.request
import urllib.error
import sys
import time

BASE = "http://127.0.0.1:8080"
MODEL = "gemma-4-31B-it-Q8_0.gguf"
ENDPOINTS = [
    "/generate",
    "/api/generate",
    "/v1/generate",
    "/v1/completions",
    "/v1/chat/completions",
    "/completions",
    "/api/completions",
    "/complete",
    "/v1/complete",
]

PAYLOADS = [
    {"model": MODEL, "prompt": "Hello"},
    {"model": MODEL, "input": "Hello"},
    {"model": MODEL, "messages": [{"role": "user", "content": "Hello"}]},
    {"prompt": "Hello", "max_tokens": 32, "model": MODEL},
    {"text": "Hello", "model": MODEL},
    {"data": {"text": "Hello"}, "model": MODEL},
]

HEADERS = {"Content-Type": "application/json"}
TIMEOUT = 6


def try_post(url, payload):
    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(url, data=data, headers=HEADERS, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:
            body = resp.read().decode("utf-8", errors="replace")
            return resp.getcode(), body, dict(resp.getheaders())
    except urllib.error.HTTPError as e:
        try:
            body = e.read().decode("utf-8", errors="replace")
        except Exception:
            body = f"HTTPError {e.code}"
        return e.code, body, dict(e.headers or {})
    except Exception as e:
        return None, repr(e), {}


def main():
    print("Probing", BASE)
    for ep in ENDPOINTS:
        url = BASE.rstrip("/") + ep
        for p in PAYLOADS:
            print(f"-> Trying {url} with payload keys={list(p.keys())}")
            status, body, headers = try_post(url, p)
            if status is None:
                print(f"   ERROR: {body}")
                time.sleep(0.15)
                continue
            print(f"   HTTP {status}")
            snippet = (body[:1000] + "...") if len(body) > 1000 else body
            print("   body:", snippet)
            # treat 200 as success; also return on 201/202
            if status in (200, 201, 202):
                print("\nSUCCESS:\nendpoint=", url)
                print("payload=", json.dumps(p))
                print("status=", status)
                print("body=", body)
                return 0
            # if server returned a JSON parse error or similar, keep trying
            time.sleep(0.12)
    print("No successful generation response found.")
    return 2


if __name__ == '__main__':
    sys.exit(main())
