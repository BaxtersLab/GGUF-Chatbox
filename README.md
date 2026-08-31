![GGUF Chatbox](assets/gguf%20chatbox%20banner.png)

# GGUF Chatbox

> ### 🐧 Running Linux? Use the Linux build instead.
>
> This repository is the **Windows** line of development. A separate Linux build lives
> at **[BaxtersLab/GGUF_Chatbox_Lin](https://github.com/BaxtersLab/GGUF_Chatbox_Lin)** and
> carries the launcher, the GSettings schema shim, the snap environment scrub and the
> Debian packaging that this tree does not have. It requires **Ubuntu 26.04 LTS / GNOME 50
> or newer**.
>
> Do not use this repository on Linux. It lacks the schema shim, without which WebKitGTK
> aborts at startup on GNOME 50.

A Windows desktop app built with [Tauri v2](https://tauri.app) that runs local GGUF language models through **llama-server** and exposes them as OpenAI-compatible endpoints — so tools like [Continue.dev](https://continue.dev), custom agents, and any OpenAI-compatible client can connect without cloud dependencies.

---

## Features

- Loads any GGUF model and starts a local `llama-server` instance automatically
- Auto-detects VRAM and calculates optimal GPU layers and context window size
- Proxies requests on `127.0.0.1:8080` with an OpenAI-compatible `/v1/chat/completions` API
- **Model card system** — reads GGUF metadata headers and caches a card per model with architecture, quant format, native context length, and recommended parameters
- **App profile system** — per-use-case parameter presets (coding agent, literary cognition, audio synthesis, general) applied automatically on server start
- **Vision inference** via a secondary llava/clip server on port 8082
- **Audio intelligence** via a Python listening server on port 8083 (genre tagging, transcription, audio review)
- HuggingFace model downloader with resume support and automatic card population
- Writes Continue.dev `config.yaml` for the active workspace
- Expansion slot plugin system in the UI — 11 slots, each independently initialised
- Context Echo — adaptive system prompt injection that scales with conversation depth
- Hot Rod Tuner integration for live inference parameter adjustment

---

## Requirements

| Requirement | Notes |
|-------------|-------|
| Windows 10 or 11 (64-bit) | The platform this repository targets. For Linux see [GGUF_Chatbox_Lin](https://github.com/BaxtersLab/GGUF_Chatbox_Lin) |
| [Rust toolchain](https://rustup.rs/) | Required to build from source |
| [Node.js](https://nodejs.org/) (optional) | Only needed if you modify the frontend |
| llama.cpp | Auto-downloaded on first launch if not found |
| NVIDIA GPU (optional) | CPU inference works; GPU strongly recommended for larger models |
| Python 3.10+ (optional) | Required only for the listening server and configurator GUI |

---

## Installation

### Option A — Build from source

1. **Install Rust**

   ```powershell
   winget install Rustlang.Rustup
   rustup update stable
   ```

2. **Install the Tauri CLI**

   ```powershell
   cargo install tauri-cli
   ```

3. **Clone the repository**

   ```powershell
   git clone https://github.com/BaxtersLab/GGUF-Chatbox.git
   cd "GGUF-Chatbox"
   ```

4. **Build and run in development mode**

   ```powershell
   cargo tauri dev
   ```

   On first run, if `llama-server` is not found the app will download the latest llama.cpp release from GitHub automatically (CUDA build if an NVIDIA GPU is detected, CPU build otherwise).

5. **Build a release installer** (optional)

   ```powershell
   cargo tauri build
   ```

   The installer and standalone executable are placed in `src-tauri/target/release/bundle/`.

### Option B — Install Python backend (optional, for audio and configurator features)

The listening server and configurator GUI require Python and a small set of packages.

```powershell
cd backend
pip install -r configurator/requirements.txt

# Optional: audio intelligence dependencies
pip install mutagen openai-whisper
# For deep learning genre tagging (GPU recommended):
pip install musicnn
```

---

## How to use

### 1. Load a model

Click **Browse** or **Scan Folder** to find a `.gguf` file. The app reads the model header and builds a card showing architecture, quantisation, native context length, and layer count. VRAM is queried automatically and GPU layers are calculated to fit the model into available memory.

### 2. Select an App Profile

Open **Slot 9 — Model Card** in the expansion tray. Use the profile dropdown to choose the preset that matches your use case:

| Profile | Best for |
|---------|----------|
| **General** | Default — uses the model's own recommended parameters |
| **Coding Agent** | Low temperature (0.1), unlimited output, 8 K context cap |
| **Literary Cognition** | Higher temperature (0.85), unlimited output |
| **Exotic Bass Maker** | Creative/generative audio synthesis workflows |

Custom profiles can be added by editing `~/.gguf-chatbox/app_profiles.json`.

### 3. Start the server

Click **Start Server**. The app:
- Starts `llama-server` on `127.0.0.1:8081` (internal)
- Starts the OpenAI-compatible proxy on `127.0.0.1:8080` (public)
- Applies the active profile's parameter overrides to the server launch flags

The **Server** panel shows status (Starting → Running) and the endpoint URL.

### 4. Connect Continue.dev (or any OpenAI-compatible client)

Use the **Write Config** button to generate a `.continue/config.yaml` for your current workspace, or add manually:

```yaml
models:
  - name: my-local-model
    provider: openai
    model: <your-model-name>
    apiBase: http://127.0.0.1:8080
    apiKey: none
```

### 5. Download a model from HuggingFace

Paste a direct `.gguf` download URL from HuggingFace into the **HF Fetcher** slot (slot 4) and click Download. The download supports resume — if interrupted, just paste the same URL again. After download, the model card is populated automatically with the HuggingFace repo ID.

### 6. Enrich a model card with HuggingFace metadata

Open **Slot 9 — Model Card** after loading a model and click **Fetch HF README**. Enter the repo ID (e.g. `Qwen/Qwen3-8B`). The README is fetched and merged into the cached card — description and temperature recommendations are extracted where present.

### 7. Vision inference (slot 7)

Set paths to a llava-compatible model and its `mmproj` file in **Advanced Settings**, then start the Vision Server. Drop an image into the Vision slot and click Annotate, OCR, or Analyze.

### 8. Audio intelligence (slot 8)

Set a Python interpreter and (optionally) model weights in **Advanced Settings**, then start the Listening Server. Browse an audio file and run Generate Tags, Transcribe, or Review.

---

## Port layout

| Port | Service |
|------|---------|
| 8080 | OpenAI-compatible proxy — connect your tools here |
| 8081 | llama-server internal — not exposed outside localhost |
| 8082 | Vision server (llava/clip) |
| 8083 | Listening server (audio intelligence, Python) |

---

## File locations

| Path | Contents |
|------|----------|
| `~/.gguf-chatbox/settings.json` | App settings (persisted across sessions) |
| `~/.gguf-chatbox/app_profiles.json` | App profile presets |
| `~/.gguf-chatbox/cards/<stem>.json` | Cached model cards |
| `~/.gguf-chatbox/models/` | Default model download directory |
| `~/.gguf-chatbox/uploads/` | Temporary image uploads for vision inference |

---

## Dependencies and Licenses

### Rust crates (compiled into the app)

| Crate | Version | License | Purpose |
|-------|---------|---------|---------|
| [tauri](https://github.com/tauri-apps/tauri) | 2.x | MIT / Apache-2.0 | Desktop app framework (Rust + WebView) |
| [tauri-build](https://github.com/tauri-apps/tauri) | 2.x | MIT / Apache-2.0 | Tauri build-time code generation |
| [serde](https://github.com/serde-rs/serde) | 1.x | MIT / Apache-2.0 | Serialisation / deserialisation framework |
| [serde_json](https://github.com/serde-rs/json) | 1.x | MIT / Apache-2.0 | JSON serialisation |
| [ureq](https://github.com/algesten/ureq) | 2.x | MIT / Apache-2.0 | Synchronous HTTP client (downloads, HF README fetch) |
| [rfd](https://github.com/PolyMeilex/rfd) | 0.14 | MIT | Native file/folder picker dialogs |
| [dirs](https://github.com/dirs-dev/dirs-rs) | 5.x | MIT / Apache-2.0 | Platform home directory resolution |
| [zip](https://github.com/zip-rs/zip2) | 2.x | MIT | ZIP extraction (llama.cpp auto-installer) |
| [base64](https://github.com/marshallpierce/rust-base64) | 0.21 | MIT / Apache-2.0 | Base64 encode/decode (vision data URLs) |
| [once_cell](https://github.com/matklad/once_cell) | 1.x | MIT / Apache-2.0 | Lazy static initialisation |
| [serde_json](https://github.com/serde-rs/json) | 1.x | MIT / Apache-2.0 | JSON for tool call payloads |
| [sha2](https://github.com/RustCrypto/hashes) | 0.10 | MIT / Apache-2.0 | SHA-256 hashing (utils crate) |
| [hex](https://github.com/KokaKiwi/rust-hex) | 0.4 | MIT / Apache-2.0 | Hex encoding (utils crate) |
| [chrono](https://github.com/chronotope/chrono) | 0.4 | MIT / Apache-2.0 | Date/time (logging crate) |
| [windows-sys](https://github.com/microsoft/windows-rs) | 0.52 | MIT / Apache-2.0 | Windows API bindings (GPU query, job objects) |

### Python packages (optional — listening server and configurator GUI)

| Package | License | Purpose |
|---------|---------|---------|
| [PyQt5](https://riverbankcomputing.com/software/pyqt/) | GPL v3 / Commercial | Configurator GUI toolkit |
| [requests](https://github.com/psf/requests) | Apache-2.0 | HTTP client used by configurator |
| [PyYAML](https://github.com/yaml/pyyaml) | MIT | YAML parsing for Continue.dev config |
| [mutagen](https://github.com/quodlibet/mutagen) | GPL v2 | Audio tag reading in listening server |
| [openai-whisper](https://github.com/openai/whisper) | MIT | Audio transcription |
| [musicnn](https://github.com/jordipons/musicnn) | ISC | Deep-learning audio tagging (optional) |

### External tools (downloaded/installed separately)

| Tool | License | Purpose |
|------|---------|---------|
| [llama.cpp](https://github.com/ggerganov/llama.cpp) | MIT | GGUF model inference engine and server |
| [llama-server](https://github.com/ggerganov/llama.cpp) | MIT | OpenAI-compatible HTTP server (part of llama.cpp) |

> **Note on PyQt5:** PyQt5 is licensed under the GPL v3, which applies to the configurator GUI component only. The core Tauri app (Rust) is MIT/Apache-2.0. If you distribute a modified version of the configurator GUI you must comply with GPL v3 terms. The remainder of the application is unaffected.

---

## License

This project is released under the **Apache License 2.0**. The full text is in
[`LICENSE`](LICENSE).

Apache-2.0 was chosen over MIT deliberately: it carries an express patent grant with a
retaliation clause, requires that modifications be stated, and disclaims trademark
rights. MIT provides none of those. The Linux build at
[GGUF_Chatbox_Lin](https://github.com/BaxtersLab/GGUF_Chatbox_Lin) is licensed
identically.

> **Note on the earlier MIT release.** This project was previously published under the
> MIT License. That relicensing is forward-looking only: anyone who already received a
> copy under MIT keeps the rights MIT granted them for that copy. Apache-2.0 applies from
> this commit onward.

### llama.cpp

`llama-server` is **MIT** licensed and is **not bundled with this application**. It is
downloaded from upstream at first launch, or supplied by you — an external tool whose
licence is not an obligation this package carries.

### Models and model output

**This licence covers the application only. It says nothing about the models you run
through it, or about what those models produce.**

Every GGUF model carries its own licence, chosen by whoever trained it, and they differ
enormously — some permissive, some bespoke community licences with acceptable-use
policies, some non-commercial only, and a few placing explicit conditions on generated
output. You choose the model, so you are responsible for complying with its licence,
including for anything you do with what it generates. The presence of a downloader in
this app is not a statement about any model it can fetch.
