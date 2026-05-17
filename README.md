![GGUF Chatbox](assets/gguf%20chatbox%20banner.png)

# GGUF Chatbox

A Windows desktop app built with Tauri v2 that runs local GGUF language models through **llama-server** and exposes them as OpenAI-compatible endpoints — so tools like [Continue.dev](https://continue.dev), custom agents, and any OpenAI-compatible client can connect without cloud dependencies.

---

## What it does

- Loads any GGUF model file and starts a local `llama-server` instance
- Auto-detects VRAM and calculates optimal GPU layers and context size
- Proxies requests on `127.0.0.1:8080` (OpenAI-compatible `/v1/chat/completions`)
- Reads model metadata from the GGUF file and caches a **model card** per model
- Applies **app profiles** (coding agent, literary cognition, audio synthesis, general) that tune temperature, token limits, and context caps per use case
- Supports vision inference via a secondary llava/clip server on port 8082
- Supports audio tagging and transcription via a Python listening server on port 8083
- Downloads models directly from HuggingFace with resume support
- Writes Continue.dev `config.yaml` for the active workspace automatically

---

## Requirements

- Windows 10/11
- [Rust toolchain](https://rustup.rs/) (for building from source)
- [llama.cpp](https://github.com/ggerganov/llama.cpp) — auto-downloaded on first run if not found
- Python 3.10+ (optional — required only for the listening server and configurator GUI)
- A GGUF model file

---

## Building from source

```powershell
# Install Tauri CLI if you don't have it
cargo install tauri-cli

# Build and run in dev mode
cargo tauri dev

# Build a release installer
cargo tauri build
```

---

## Port layout

| Port | Service |
|------|---------|
| 8080 | OpenAI-compatible proxy (public) |
| 8081 | llama-server (internal — do not expose) |
| 8082 | Vision server (llava/clip) |
| 8083 | Listening server (audio intelligence) |

---

## App profiles

Profiles are stored in `~/.gguf-chatbox/app_profiles.json` and applied automatically when the server starts. The default profiles are:

| Profile | Temperature | Max Tokens | Ctx Cap |
|---------|------------|------------|---------|
| General | model default | model default | none |
| Coding Agent | 0.1 | unlimited | 8192 |
| Literary Cognition | 0.85 | unlimited | none |
| Exotic Bass Maker | 0.7 | 512 | 4096 |

Select the active profile from the **Model Card** panel (slot 9) in the app. The profile is applied the next time you start the server.

---

## Model cards

When a model is loaded, GGUF Chatbox reads its metadata header and builds a card containing:
- Architecture, author, quantisation, native context length, layer count
- Recommended inference parameters (if present in the GGUF metadata)
- HuggingFace repo ID (populated automatically when downloading from HF)

Cards are cached as JSON at `~/.gguf-chatbox/cards/<model-stem>.json`. You can enrich a card with the model's HuggingFace README by clicking **Fetch HF README** in the Model Card panel.

---

## Continue.dev integration

Point Continue.dev at the proxy:

```yaml
models:
  - name: local-model
    provider: openai
    model: your-model-name
    apiBase: http://127.0.0.1:8080
    apiKey: none
```

Use the **Write Continue Config** button in the app to generate this automatically for the current workspace.

---

## Project structure

```
gguf chatbox/
├── src-tauri/          # Tauri app shell and Rust command handlers
├── frontend/           # HTML/JS/CSS UI (no framework — vanilla)
├── crates/
│   ├── adaptive_llama/ # Model loading, GPU detection, model card reader
│   ├── server/         # llama-server manager and OpenAI proxy
│   ├── context_echo/   # Adaptive system prompt injection
│   ├── tool_belt/      # Built-in tool registry for function calling
│   ├── logging/        # Structured logging
│   ├── error_system/   # Error classification and GPU recovery
│   └── utils/          # Shared utilities
├── backend/
│   ├── configurator/   # Python GUI for Continue.dev profile management
│   └── listening_server.py  # Audio intelligence server (port 8083)
├── assets/             # Icons and banner
└── tools/              # CLI helpers for config management
```

---

## License

MIT
