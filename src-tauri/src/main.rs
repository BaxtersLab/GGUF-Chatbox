#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};
use adaptive_llama::{
    load_model, unload_model, run_inference,
    auto_gpu_layers, gguf_block_count, gguf_context_length, max_context_for_vram, query_vram_mb,
    ModelConfig, ModelInstance, InferenceRequest, SamplingConfig, ChatMessage, DEFAULT_TIMEOUT,
    load_or_build_card, ModelCard, save_card, merge_hf_readme,
};
use context_echo::{
    capture_echo_source, check_milestone, inject_echo,
    EchoConfig, EchoSource,
};
use server::{start_server, stop_server, health_check, model_name_from_path, ServerConfig, ServerStatus};
use tool_belt::default_registry;

/// Wrapper to send a `*mut ModelInstance` across thread boundaries.
/// SAFETY: caller must guarantee the pointee outlives the thread and
/// no other thread mutates it concurrently.
struct SendPtr(*mut ModelInstance);
unsafe impl Send for SendPtr {}

impl SendPtr {
    unsafe fn as_mut(&self) -> &mut ModelInstance {
        &mut *self.0
    }
}
use serde::{Serialize, Deserialize};
use serde_json::json;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
use std::fs::File;
use base64;
use std::env;
use std::time::Duration;
use std::thread::sleep;
use std::fs::OpenOptions;

// ── ToolDispatcher adapter ────────────────────────────────────────────────
// Wraps tool_belt::ToolRegistry and implements server::ToolDispatcher so the
// proxy can dispatch tool calls through the registry without a circular dep.

struct RegistryDispatcher(tool_belt::ToolRegistry);

impl server::ToolDispatcher for RegistryDispatcher {
    fn dispatch(&self, name: &str, args: serde_json::Value) -> Result<String, String> {
        self.0.dispatch(name, args).map_err(|e| e.to_string())
    }
}

// ── Vision server process handle ─────────────────────────────────────────
// Separate from the text server (port 8081). Vision server binds port 8082.
static VISION_SERVER: Mutex<Option<std::process::Child>> = Mutex::new(None);

// ── Listening server process handle ───────────────────────────────────────
// Python listening_server.py on port 8083 (musicnn / AcousticBrainz tagging).
static LISTENING_SERVER: Mutex<Option<std::process::Child>> = Mutex::new(None);

// ── Managed state ─────────────────────────────────────────────────────────

struct AppState {
    instance: Option<ModelInstance>,
    chat_history: Vec<ChatMessage>,
    system_prompt: String,
    echo_config: EchoConfig,
    echo_source: Option<EchoSource>,
    echo_fired: Vec<f32>,
    echo_enabled: bool,
    spawned_children: Vec<std::process::Child>,
    spawned_pids: Vec<u32>,
}

// ── Serialisable types ────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
struct ModelInfo {
    path: String,
    size_mb: u64,
    context_length: u32,
    layers: u32,
}

#[derive(Serialize)]
struct ModelSubfolder {
    name: String,
    path: String,
    gguf_files: Vec<ModelInfo>,
}

#[derive(Clone, Serialize)]
struct HfDownloadProgress {
    filename: String,
    downloaded_mb: u64,
    total_mb: u64,
    percent: f64,
    done: bool,
    error: String,
}

#[derive(Clone, Serialize)]
struct HfDownloadLog {
    line: String,
    done: bool,
    error: bool,
}

#[derive(Serialize)]
struct ModelMeta {
    path: String,
    context_length: u32,
    layers: u32,
    gpu_layers: u32,
    size_mb: u64,
}

#[derive(Serialize)]
struct EchoState {
    enabled: bool,
    milestones: Vec<f32>,
    max_echo_pct: f32,
    system_prompt: String,
}

#[derive(Serialize)]
struct ServerState {
    status: String,
    endpoint: String,
    model_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AppProfile {
    id: String,
    label: String,
    #[serde(default)]
    temperature_override: Option<f32>,
    #[serde(default)]
    n_predict_override: Option<i32>,
    #[serde(default)]
    ctx_cap_override: Option<u32>,
}

#[derive(Serialize, Deserialize, Clone)]
struct AppSettings {
    throttle_pct: u32,
    default_system_prompt: String,
    #[serde(default)]
    models_folder: String,
    #[serde(default)]
    voice_enabled: bool,
    #[serde(default = "default_voicebox_url")]
    voicebox_url: String,
    #[serde(default)]
    voice_profile_id: String,
    #[serde(default)]
    voicebox_exe_path: String,
    #[serde(default)]
    whisper_exe_path: String,
    #[serde(default)]
    whisper_model_path: String,
    #[serde(default = "default_silence_timeout")]
    whisper_silence_timeout: u32,
    #[serde(default)]
    last_model_path: String,
    #[serde(default)]
    last_gpu_layers: Option<u32>,
    #[serde(default)]
    max_context_override: u32,
    #[serde(default)]
    filter_msg_tags: bool,
    #[serde(default)]
    vision_model_path: String,
    #[serde(default)]
    mmproj_path: String,
    #[serde(default)]
    main_mmproj_path: String,
    #[serde(default)]
    listening_model_path: String,
    #[serde(default)]
    active_app_profile: String,
    #[serde(default = "default_auto_apply_card")]
    auto_apply_card_params: bool,
    #[serde(default)]
    use_main_for_vision: bool,
    #[serde(default)]
    use_main_for_audio: bool,
    /// CD-changer magazine: the model-loader's disks (index = slot). SOC reads
    /// this from settings.json as the swap disk registry. Scales to N slots.
    #[serde(default)]
    magazine: Vec<MagazineDisk>,
}

fn default_silence_timeout() -> u32 { 5 }

fn default_auto_apply_card() -> bool { true }

fn default_voicebox_url() -> String { "http://localhost:17493".to_string() }

impl Default for AppSettings {
    fn default() -> Self {
        AppSettings {
            throttle_pct: 100,
            default_system_prompt: String::new(),
            models_folder: String::new(),
            voice_enabled: false,
            voicebox_url: default_voicebox_url(),
            voice_profile_id: String::new(),
            voicebox_exe_path: String::new(),
            whisper_exe_path: String::new(),
            whisper_model_path: String::new(),
            whisper_silence_timeout: 5,
            last_model_path: String::new(),
            last_gpu_layers: None,
            max_context_override: 0,
            filter_msg_tags: true,
            vision_model_path: String::new(),
            mmproj_path: String::new(),
            main_mmproj_path: String::new(),
            listening_model_path: String::new(),
            active_app_profile: "general".to_string(),
            auto_apply_card_params: true,
            use_main_for_vision: false,
            use_main_for_audio: false,
            magazine: Vec::new(),
        }
    }
}

/// One CD-changer disk: a model (+ optional mmproj) the loader can load and SOC
/// can swap to. The `magazine` array (in AppSettings) is the ordered slot list.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct MagazineDisk {
    #[serde(default)]
    model_path: String,
    #[serde(default)]
    mmproj_path: String,
    #[serde(default)]
    label: String,
}

fn settings_path() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".gguf-chatbox")
        .join("settings.json")
}

fn profiles_path() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".gguf-chatbox")
        .join("app_profiles.json")
}

#[derive(Serialize)]
struct ModelStats {
    tokens_used: usize,
    ctx_size: u32,
    tok_s: f64,
    vram_mb: u32,
    temperature: f32,
    done: bool,
}

// ── Tauri commands ────────────────────────────────────────────────────────

#[tauri::command]
fn cmd_list_models() -> Vec<ModelInfo> {
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    let models_dir = home.join(".gguf-chatbox").join("models");
    let mut results = Vec::new();
    let entries = match std::fs::read_dir(&models_dir) {
        Ok(e) => e,
        Err(_) => return results,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("gguf") {
            continue;
        }
        let size_mb = std::fs::metadata(&path)
            .map(|m| m.len() / (1024 * 1024))
            .unwrap_or(0);
        let context_length = gguf_context_length(&path).unwrap_or(4096);
        let layers = gguf_block_count(&path).unwrap_or(0);
        results.push(ModelInfo {
            path: path.to_string_lossy().into_owned(),
            size_mb,
            context_length,
            layers,
        });
    }
    results
}

#[tauri::command]
fn cmd_download_hf(
    url: String,
    filename: Option<String>,
    app: AppHandle,
) -> Result<String, String> {
    // Spawn background thread to perform resumable download and emit progress events
    std::thread::spawn(move || {
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
        let models_dir = home.join(".gguf-chatbox").join("models");
        if let Err(e) = std::fs::create_dir_all(&models_dir) {
            emit_debug_log(&app, &format!("mkdir failed: {}", e), true);
            let _ = app.emit("hf-download-progress", serde_json::json!({"error": e.to_string(), "done": true}));
            return;
        }

        // Determine filename
        let fname = match filename {
            Some(f) => f,
            None => {
                match url.rsplit('/').next() {
                    Some(s) if !s.is_empty() => s.to_string(),
                    _ => "download.bin".to_string(),
                }
            }
        };

        let dest_path = models_dir.join(&fname);
        let part_path = models_dir.join(format!("{}.part", fname));

        // Check existing size for resume (from .part)
        let mut existing: u64 = std::fs::metadata(&part_path).map(|m| m.len()).unwrap_or(0);

        // Try to get total size via HEAD (best-effort)
        let agent_for_head = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(30))
            .build();
        let total_size_opt = agent_for_head.head(&url).call().ok().and_then(|r| {
            r.header("Content-Length").and_then(|s| s.parse::<u64>().ok())
        });

        let mut last_emit = 0u64;

        // Retry loop for transient read errors
        let max_attempts = 3usize;
        let mut attempt = 0usize;
        loop {
            attempt += 1;

            let agent = ureq::AgentBuilder::new()
                .timeout_connect(std::time::Duration::from_secs(30))
                .timeout_read(std::time::Duration::from_secs(300))
                .build();

            // Request with Range header to resume
            let mut req = agent.get(&url);
            if existing > 0 {
                req = req.set("Range", &format!("bytes={}-", existing));
            }

            let resp = match req.call() {
                Ok(r) => r,
                Err(e) => {
                    emit_debug_log(&app, &format!("download request failed: {}", e), true);
                    let _ = app.emit("hf-download-progress", serde_json::json!({"filename": fname, "downloaded_mb": existing/1024/1024, "total_mb": total_size_opt.unwrap_or(0)/1024/1024, "percent": if total_size_opt.is_some() { (existing as f64)/(total_size_opt.unwrap() as f64) * 100.0 } else { 0.0 }, "done": true, "error": e.to_string()}));
                    return;
                }
            };

            // Reject HTML/JSON responses — means we hit a web page, not a binary file.
            let content_type = resp.header("Content-Type").unwrap_or_default().to_lowercase();
            if content_type.contains("text/html") || content_type.contains("application/json") {
                let msg = "Server returned a web page, not a model file. Check your URL — you need a direct .gguf download link.";
                let _ = app.emit("hf-download-progress", serde_json::json!({"filename": fname, "error": msg, "done": true}));
                return;
            }

            let status = resp.status();
            let resumed = status == 206 && existing > 0;

            let content_len: Option<u64> = resp.header("Content-Length").and_then(|v| v.parse().ok());
            let total: u64 = if resumed {
                content_len.map(|cl| cl + existing).unwrap_or(existing)
            } else {
                content_len.unwrap_or(0)
            };

            // Open file: append if resumed, create/truncate otherwise.
            let mut file = if resumed {
                match std::fs::OpenOptions::new().create(true).append(true).open(&part_path) {
                    Ok(f) => f,
                    Err(e) => {
                        emit_debug_log(&app, &format!("open part failed: {}", e), true);
                        let _ = app.emit("hf-download-progress", serde_json::json!({"filename": fname, "error": e.to_string(), "done": true}));
                        return;
                    }
                }
            } else {
                match std::fs::File::create(&part_path) {
                    Ok(f) => f,
                    Err(e) => {
                        emit_debug_log(&app, &format!("create part failed: {}", e), true);
                        let _ = app.emit("hf-download-progress", serde_json::json!({"filename": fname, "error": e.to_string(), "done": true}));
                        return;
                    }
                }
            };

            let mut reader = resp.into_reader();
            let mut buf = vec![0u8; 1_048_576]; // 1 MiB chunks

            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Err(e) = file.write_all(&buf[..n]) {
                            emit_debug_log(&app, &format!("write failed: {}", e), true);
                            let _ = app.emit("hf-download-progress", serde_json::json!({"filename": fname, "error": e.to_string(), "done": true}));
                            return;
                        }
                        existing += n as u64;

                        // Emit progress every 1 MiB
                        if existing - last_emit >= 1024 * 1024 {
                            last_emit = existing;
                            let percent = if total > 0 { (existing as f64) / (total as f64) * 100.0 } else { 0.0 };
                            let _ = app.emit("hf-download-progress", serde_json::json!({
                                "filename": fname,
                                "downloaded_mb": existing / 1024 / 1024,
                                "total_mb": if total>0 { total / 1024 / 1024 } else { 0 },
                                "percent": percent,
                                "done": false,
                                "error": ""
                            }));
                        }
                    }
                    Err(e) => {
                        // Read error — attempt retry unless we've exhausted attempts
                        emit_debug_log(&app, &format!("read failed (attempt {}): {}", attempt, e), true);
                        if attempt < max_attempts {
                            let _ = app.emit("hf-download-progress", serde_json::json!({"filename": fname, "downloaded_mb": existing/1024/1024, "total_mb": if total>0 { total/1024/1024 } else { 0 }, "percent": if total>0 {(existing as f64)/(total as f64)*100.0} else {0.0}, "done": false, "error": format!("read error, retrying: {}", e)}));
                            // Sleep briefly then retry by breaking to outer loop (which will re-request Range)
                            std::thread::sleep(std::time::Duration::from_millis(500));
                            break;
                        } else {
                            let _ = app.emit("hf-download-progress", serde_json::json!({"filename": fname, "error": e.to_string(), "done": true}));
                            return;
                        }
                    }
                }
            }

            // After inner loop, check if we completed download
            if total > 0 && existing >= total {
                let percent = (existing as f64) / (total as f64) * 100.0;
                // Atomic rename .part → final
                if let Err(e) = std::fs::rename(&part_path, &dest_path) {
                    emit_debug_log(&app, &format!("rename failed: {}", e), true);
                    let _ = app.emit("hf-download-progress", serde_json::json!({"filename": fname, "error": e.to_string(), "done": true}));
                    return;
                }
                let _ = app.emit("hf-download-progress", serde_json::json!({
                    "filename": fname,
                    "downloaded_mb": existing / 1024 / 1024,
                    "total_mb": total / 1024 / 1024,
                    "percent": percent,
                    "done": true,
                    "error": ""
                }));
                break;
            } else if total == 0 && attempt >= max_attempts {
                // Unknown total and we've exhausted attempts — consider download complete if no more data
                if let Err(e) = std::fs::rename(&part_path, &dest_path) {
                    emit_debug_log(&app, &format!("rename failed: {}", e), true);
                    let _ = app.emit("hf-download-progress", serde_json::json!({"filename": fname, "error": e.to_string(), "done": true}));
                    return;
                }
                let _ = app.emit("hf-download-progress", serde_json::json!({"filename": fname, "downloaded_mb": existing/1024/1024, "total_mb": existing/1024/1024, "percent": 100.0, "done": true, "error": ""}));
                break;
            }

            // If we broke for a retry, refresh existing bytes and loop again
            existing = std::fs::metadata(&part_path).map(|m| m.len()).unwrap_or(existing);

            // Small backoff before next attempt
            if attempt < max_attempts {
                std::thread::sleep(std::time::Duration::from_millis(300));
                continue;
            } else {
                // Exhausted attempts, emit final error
                let _ = app.emit("hf-download-progress", serde_json::json!({"filename": fname, "downloaded_mb": existing/1024/1024, "total_mb": if total>0 { total/1024/1024 } else { 0 }, "percent": if total>0 {(existing as f64)/(total as f64)*100.0} else {0.0}, "done": true, "error": "exhausted retries"}));
                return;
            }
        }
    });

    Ok("started".to_string())
}

#[tauri::command]
fn cmd_load_model(
    path: String,
    app: AppHandle,
    state: State<Mutex<AppState>>,
) -> Result<ModelMeta, String> {
    let model_path = std::path::PathBuf::from(&path);
    let context_length = gguf_context_length(&model_path).unwrap_or(4096);
    let layers = gguf_block_count(&model_path).unwrap_or(0);
    let size_mb = std::fs::metadata(&model_path)
        .map(|m| m.len() / (1024 * 1024))
        .unwrap_or(0);
    let vram_mb = query_vram_mb();
    let (gpu_layers, _) = auto_gpu_layers(&model_path, vram_mb, context_length);
    // Compute the vram-capped context, then apply any user override (safely clamped).
    let vram_capped = max_context_for_vram(&model_path, vram_mb)
        .unwrap_or(context_length)
        .min(context_length);

    // Respect user setting but clamp to model and VRAM limits.
    let settings = cmd_get_settings();
    let mut applied_ctx = vram_capped;
    if settings.max_context_override > 0 {
        let requested = settings.max_context_override;
        let clamped = requested.min(context_length).min(vram_capped);
        if clamped != requested {
            emit_debug_log(&app, &format!("Max context override requested {} but clamped to {} (model max={}, vram_cap={})", requested, clamped, context_length, vram_capped), false);
        } else {
            emit_debug_log(&app, &format!("Max context override applied: {}", clamped), false);
        }
        applied_ctx = clamped;
    }
    let config = ModelConfig {
        model_path: model_path.clone(),
        context_length: applied_ctx,
        gpu_setting: if gpu_layers > 0 { "GPU".to_string() } else { "CPU".to_string() },
    };
    let mut instance = ModelInstance::new(config, gpu_layers);
    load_model(&mut instance).map_err(|e| e.to_string())?;

    emit_debug_log(&app, &format!("Model loaded: {} (ctx={}, gpu_layers={})", path, applied_ctx, gpu_layers), false);

    let mut guard = state.lock().map_err(|_| "state lock poisoned".to_string())?;
    if let Some(ref mut prev) = guard.instance {
        let _ = unload_model(prev);
    }
    guard.instance = Some(instance);
    // Reset per-session echo state for new model.
    guard.chat_history.clear();
    guard.echo_fired.clear();
    guard.echo_source = None;

    // Load or build model card from GGUF metadata; emit to frontend.
    let card = load_or_build_card(&model_path);
    let _ = app.emit("model-card-loaded", &card);

    Ok(ModelMeta { path, context_length: applied_ctx, layers, gpu_layers, size_mb })
}

#[tauri::command]
fn cmd_send_message(
    text: String,
    app: AppHandle,
    state: State<Mutex<AppState>>,
) -> Result<(), String> {
    // ── Prepare under lock ────────────────────────────────────────────────
    let (instance_ptr, ctx_size, final_text) = {
        let mut guard = state.lock().map_err(|_| "state lock poisoned".to_string())?;

        guard.chat_history.push(ChatMessage { role: "user".to_string(), content: text.clone() });

        // ── Context Echo ──────────────────────────────────────────────────
        let mut final_text = text.clone();
        if guard.echo_enabled {
            if guard.echo_source.is_none() {
                let ce_msgs: Vec<context_echo::ChatMessage> = guard.chat_history.iter()
                    .map(|m| context_echo::ChatMessage { role: m.role.clone(), content: m.content.clone() })
                    .collect();
                let sp = guard.system_prompt.clone();
                guard.echo_source = Some(capture_echo_source(&sp, &ce_msgs));
            }

            let current_tokens: usize = guard.chat_history.iter()
                .map(|m| m.content.len() / 2)
                .sum();
            let ctx_size = guard.instance.as_ref()
                .map(|i| i.context_length as usize)
                .unwrap_or(4096);

            let echo_cfg = guard.echo_config.clone();
            let milestone = check_milestone(
                current_tokens,
                ctx_size,
                &echo_cfg,
                &mut guard.echo_fired,
            );
            if let (Some(ms), Some(src)) = (milestone, guard.echo_source.as_ref()) {
                final_text = inject_echo(&final_text, src, &ms);
            }
        }

        let instance = guard.instance.as_mut().ok_or("no model loaded")?;
        let ctx_size = instance.context_length;

        // Step 2 & 3: Reset the cancel flag before each inference
        instance.cancel.store(false, std::sync::atomic::Ordering::Relaxed);

        // SAFETY: We take a raw pointer to the instance so we can use it
        // off-thread without holding the Mutex for the entire inference.
        // The instance lives in the AppState behind an Arc-like Tauri State
        // and is only removed by cmd_unload_model (which the user won't call
        // while inference is running).  The Mutex is re-acquired at the end
        // to append the assistant reply.
        let ptr = SendPtr(instance as *mut ModelInstance);
        (ptr, ctx_size, final_text)
    }; // lock released here

    let app_clone = app.clone();

    std::thread::spawn(move || {
        let instance = unsafe { instance_ptr.as_mut() };
        let temp_dir = std::env::temp_dir();
        let prompt_path = temp_dir.join("gguf_chatbox_prompt.txt");
        let output_path = temp_dir.join("gguf_chatbox_output.txt");
        let log_path    = temp_dir.join("gguf_chatbox_run.log");

        if let Err(e) = std::fs::write(&prompt_path, &final_text) {
            let _ = app_clone.emit("chat-token", serde_json::json!({"token": "", "done": true, "error": e.to_string()}));
            return;
        }

        let request = InferenceRequest {
            chunk_id: 0,
            prompt_path,
            output_path,
            log_path,
            sampling: SamplingConfig::default(),
        };

        let vram_mb     = query_vram_mb();
        let temperature = SamplingConfig::default().temperature;

        let token_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let tc          = token_count.clone();
        let gen_start   = std::time::Instant::now();
        let app2        = app_clone.clone();

        let callback: adaptive_llama::TokenCallback = Box::new(move |line: &str| {
            let count   = tc.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            let elapsed = gen_start.elapsed().as_secs_f64();
            let tok_s   = if elapsed > 0.001 { (count as f64 / elapsed * 10.0).round() / 10.0 } else { 0.0 };
            let _ = app2.emit("model-stats", serde_json::json!({
                "tokens_used": count,
                "ctx_size":    ctx_size,
                "tok_s":       tok_s,
                "vram_mb":     vram_mb,
                "temperature": temperature,
                "done":        false,
            }));
            // Emit raw token and debugging info if token looks suspicious (many ? or replacement chars)
            let q_count = line.chars().filter(|c| *c == '?').count();
            let has_repl = line.chars().any(|c| c == '\u{FFFD}');
            if has_repl || (line.len() > 0 && q_count * 2 > line.len()) {
                let hex = line.as_bytes().iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(" ");
                emit_debug_log(&app2, &format!("token-debug raw={:?} hex={}", line, hex), true);
            }
            let _ = app2.emit("chat-token", serde_json::json!({"token": line, "done": false}));
        });

        let result = run_inference(instance, &request, DEFAULT_TIMEOUT, Some(callback));

        if let Err(ref e) = result {
            emit_debug_log(&app_clone, &format!("Inference error: {}", e), true);
        }

        let final_count = token_count.load(std::sync::atomic::Ordering::Relaxed);
        let _ = app_clone.emit("model-stats", serde_json::json!({
            "tokens_used": final_count,
            "ctx_size":    ctx_size,
            "tok_s":       0.0,
            "vram_mb":     vram_mb,
            "temperature": temperature,
            "done":        true,
        }));

        let _ = app_clone.emit("chat-token", serde_json::json!({"token": "", "done": true}));
    });

    Ok(())
}

#[tauri::command]
fn cmd_reload_gpu_layers(
    gpu_layers: u32,
    app: AppHandle,
    state: State<Mutex<AppState>>,
) -> Result<ModelMeta, String> {
    let mut guard = state.lock().map_err(|_| "state lock poisoned".to_string())?;
    let instance = guard.instance.as_mut().ok_or("no model loaded")?;
    let model_path = instance.model_path.clone();
    let context_length = instance.context_length;

    // Unload current
    let _ = unload_model(instance);
    guard.instance = None;

    // Reload with new gpu_layers
    let layers = gguf_block_count(&model_path).unwrap_or(0);
    let clamped = gpu_layers.min(layers);
    let size_mb = std::fs::metadata(&model_path)
        .map(|m| m.len() / (1024 * 1024))
        .unwrap_or(0);
    let config = ModelConfig {
        model_path: model_path.clone(),
        context_length,
        gpu_setting: if clamped > 0 { "GPU".to_string() } else { "CPU".to_string() },
    };
    let mut new_instance = ModelInstance::new(config, clamped);
    load_model(&mut new_instance).map_err(|e| e.to_string())?;

    let path = model_path.to_string_lossy().into_owned();
    emit_debug_log(&app, &format!("Reloaded: {} (ctx={}, gpu_layers={})", path, context_length, clamped), false);

    guard.instance = Some(new_instance);
    guard.chat_history.clear();
    guard.echo_fired.clear();
    guard.echo_source = None;

    Ok(ModelMeta { path, context_length, layers, gpu_layers: clamped, size_mb })
}

#[tauri::command]
fn cmd_get_model_stats(state: State<Mutex<AppState>>) -> Result<ModelStats, String> {
    let guard   = state.lock().map_err(|_| "state lock poisoned".to_string())?;
    let ctx_size = guard.instance.as_ref().map(|i| i.context_length).unwrap_or(4096);
    drop(guard);
    let vram_mb = query_vram_mb();
    Ok(ModelStats {
        tokens_used: 0,
        ctx_size,
        tok_s:       0.0,
        vram_mb,
        temperature: SamplingConfig::default().temperature,
        done:        false,
    })
}

#[tauri::command]
fn cmd_stop_generation(state: State<Mutex<AppState>>) -> Result<(), String> {
    let guard = state.lock().map_err(|_| "state lock poisoned".to_string())?;
    if let Some(ref instance) = guard.instance {
        instance.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    Ok(())
}

#[tauri::command]
fn cmd_unload_model(state: State<Mutex<AppState>>) -> Result<(), String> {
    let mut guard = state.lock().map_err(|_| "state lock poisoned".to_string())?;
    if let Some(ref mut instance) = guard.instance {
        unload_model(instance).map_err(|e| e.to_string())?;
    }
    guard.instance = None;
    Ok(())
}

#[tauri::command]
fn cmd_set_echo_enabled(enabled: bool, state: State<Mutex<AppState>>) -> Result<(), String> {
    let mut guard = state.lock().map_err(|_| "state lock poisoned".to_string())?;
    guard.echo_enabled = enabled;
    Ok(())
}

#[tauri::command]
fn cmd_set_system_prompt(prompt: String, state: State<Mutex<AppState>>) -> Result<(), String> {
    let mut guard = state.lock().map_err(|_| "state lock poisoned".to_string())?;
    guard.system_prompt = prompt;
    // Invalidate echo source so it re-captures with the new prompt.
    guard.echo_source = None;
    Ok(())
}

#[tauri::command]
fn cmd_set_echo_config(
    milestones: Vec<f32>,
    max_echo_pct: f32,
    state: State<Mutex<AppState>>,
) -> Result<(), String> {
    let mut guard = state.lock().map_err(|_| "state lock poisoned".to_string())?;
    guard.echo_config = EchoConfig { milestones, max_echo_pct };
    guard.echo_fired.clear(); // reset fired list when config changes
    Ok(())
}

#[tauri::command]
fn cmd_get_echo_state(state: State<Mutex<AppState>>) -> Result<EchoState, String> {
    let guard = state.lock().map_err(|_| "state lock poisoned".to_string())?;
    Ok(EchoState {
        enabled: guard.echo_enabled,
        milestones: guard.echo_config.milestones.clone(),
        max_echo_pct: guard.echo_config.max_echo_pct,
        system_prompt: guard.system_prompt.clone(),
    })
}

// ── Server Tauri commands ─────────────────────────────────────────────────

#[tauri::command]
fn cmd_get_settings() -> AppSettings {
    let path = settings_path();
    if path.exists() {
        if let Ok(raw) = std::fs::read_to_string(&path) {
            if let Ok(s) = serde_json::from_str::<AppSettings>(&raw) {
                return s;
            }
        }
    }
    AppSettings::default()
}

#[tauri::command]
fn cmd_update_settings(
    throttle_pct: Option<u32>,
    default_system_prompt: Option<String>,
    models_folder: Option<String>,
    voice_enabled: Option<bool>,
    voicebox_url: Option<String>,
    voice_profile_id: Option<String>,
    voicebox_exe_path: Option<String>,
    whisper_exe_path: Option<String>,
    whisper_model_path: Option<String>,
    whisper_silence_timeout: Option<u32>,
    last_model_path: Option<String>,
    last_gpu_layers: Option<u32>,
    filter_msg_tags: Option<bool>,
    max_context_override: Option<u32>,
    vision_model_path: Option<String>,
    mmproj_path: Option<String>,
    main_mmproj_path: Option<String>,
    listening_model_path: Option<String>,
    use_main_for_vision: Option<bool>,
    use_main_for_audio: Option<bool>,
    magazine: Option<Vec<MagazineDisk>>,
    state: State<Mutex<AppState>>,
) -> Result<(), String> {
    let mut settings = cmd_get_settings();
    if let Some(t) = throttle_pct { settings.throttle_pct = t.clamp(25, 100); }
    if let Some(ref p) = default_system_prompt { settings.default_system_prompt = p.clone(); }
    if let Some(f) = models_folder { settings.models_folder = f; }
    if let Some(v) = voice_enabled { settings.voice_enabled = v; }
    if let Some(u) = voicebox_url { settings.voicebox_url = u; }
    if let Some(p) = voice_profile_id { settings.voice_profile_id = p; }
    if let Some(p) = voicebox_exe_path { settings.voicebox_exe_path = p; }
    if let Some(p) = whisper_exe_path { settings.whisper_exe_path = p; }
    if let Some(p) = whisper_model_path { settings.whisper_model_path = p; }
    if let Some(t) = whisper_silence_timeout { settings.whisper_silence_timeout = t.clamp(1, 15); }
    if let Some(p) = last_model_path { settings.last_model_path = p; }
    if let Some(g) = last_gpu_layers { settings.last_gpu_layers = Some(g); }
    if let Some(f) = filter_msg_tags { settings.filter_msg_tags = f; }
    if let Some(m) = max_context_override { settings.max_context_override = m; }
    if let Some(p) = vision_model_path { settings.vision_model_path = p; }
    if let Some(p) = mmproj_path { settings.mmproj_path = p; }
    if let Some(p) = main_mmproj_path { settings.main_mmproj_path = p; }
    if let Some(p) = listening_model_path { settings.listening_model_path = p; }
    if let Some(v) = use_main_for_vision { settings.use_main_for_vision = v; }
    if let Some(v) = use_main_for_audio { settings.use_main_for_audio = v; }
    if let Some(m) = magazine { settings.magazine = m; }
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    if let Some(prompt) = default_system_prompt {
        let mut guard = state.lock().map_err(|_| "state lock poisoned".to_string())?;
        if guard.system_prompt.is_empty() {
            guard.system_prompt = prompt;
        }
    }
    Ok(())
}

#[tauri::command]
fn cmd_start_server(state: State<Mutex<AppState>>) -> Result<(), String> {
    let guard = state.lock().map_err(|_| "state lock poisoned".to_string())?;
    let instance = guard.instance.as_ref().ok_or("no model loaded")?;
    let model_path = instance.config.model_path.clone();
    let ctx = instance.context_length;
    drop(guard);

    // Load active app profile and apply its parameter overrides.
    let settings = cmd_get_settings();
    let (temp_override, n_predict_override, ctx_cap_override) = if settings.auto_apply_card_params {
        let profile_id = if settings.active_app_profile.is_empty() {
            "general".to_string()
        } else {
            settings.active_app_profile.clone()
        };
        let profiles: Vec<AppProfile> = std::fs::read_to_string(profiles_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        if let Some(p) = profiles.iter().find(|p| p.id == profile_id) {
            (p.temperature_override, p.n_predict_override, p.ctx_cap_override)
        } else {
            (None, None, None)
        }
    } else {
        (None, None, None)
    };

    // Pass main_mmproj_path to the server when set — required for multimodal
    // models (Phi-4, LLaVA, etc.) loaded as the main chat model.
    let main_mmproj: Option<PathBuf> = {
        let p = settings.main_mmproj_path.trim();
        if !p.is_empty() {
            let pb = PathBuf::from(p);
            if pb.exists() { Some(pb) } else { None }
        } else {
            None
        }
    };

    let config = ServerConfig {
        model_path: model_path.clone(),
        context_length: ctx,
        threads: 9,
        mmproj_path: main_mmproj,
        temperature_override: temp_override,
        n_predict_override,
        ctx_cap_override,
    };

    // IMPORTANT: bind/listen on the public proxy port BEFORE spawning the
    // model server. This ordering prevents a failing proxy bind (eg. port
    // collision) from leaving already-spawned model worker processes running
    // orphaned. The proxy will accept connections and return upstream errors
    // until the model server is available.
    std::thread::spawn(|| {
        let dispatcher = RegistryDispatcher(default_registry());
        server::start_proxy(dispatcher);
    });

    // Additive CD-changer swap-control listener on :8086 (proxy :8080 and
    // llama-server :8081 left untouched). Lets an external orchestrator (SOC
    // Ultralight) POST /swap to load a different model, reusing start_server.
    // Safe to call repeatedly: a second bind on :8086 fails harmlessly + returns.
    std::thread::spawn(|| {
        server::start_control_server();
    });

    // Now start the model server. If the proxy bind fails, it will fail
    // before we spawn the server; if the server spawn fails, the proxy
    // remains bound and reports upstream unavailable.
    start_server(&config).map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
fn cmd_stop_server() -> Result<(), String> {
    stop_server().map_err(|e| e.to_string())
}

#[tauri::command]
fn cmd_server_status(state: State<Mutex<AppState>>) -> Result<ServerState, String> {
    let status = health_check();
    let (status_str, model_name) = {
        let guard = state.lock().map_err(|_| "state lock poisoned".to_string())?;
        let mn = guard.instance.as_ref()
            .map(|i| model_name_from_path(&i.config.model_path))
            .unwrap_or_default();
        let st = match status {
            ServerStatus::Running  => "running",
            ServerStatus::Starting => "starting",
            ServerStatus::Stopped  => "stopped",
            ServerStatus::Crashed  => "crashed",
            ServerStatus::Down     => "down",
        };
        (st.to_string(), mn)
    };
    Ok(ServerState {
        status: status_str,
        endpoint: "http://127.0.0.1:8080/v1".to_string(),
        model_name,
    })
}

// ── Model card commands ───────────────────────────────────────────────────

#[tauri::command]
fn cmd_get_model_card(state: State<Mutex<AppState>>) -> Result<Option<ModelCard>, String> {
    let model_path = {
        let guard = state.lock().map_err(|_| "state lock poisoned".to_string())?;
        guard.instance.as_ref().map(|i| i.config.model_path.clone())
    };
    match model_path {
        None => Ok(None),
        Some(p) => Ok(Some(load_or_build_card(&p))),
    }
}

#[tauri::command]
fn cmd_get_app_profiles() -> Result<Vec<AppProfile>, String> {
    let path = profiles_path();
    if !path.exists() {
        return Ok(default_app_profiles());
    }
    let data = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    serde_json::from_str(&data).map_err(|e| e.to_string())
}

#[tauri::command]
fn cmd_set_active_profile(profile_id: String) -> Result<(), String> {
    let mut settings = cmd_get_settings();
    settings.active_app_profile = profile_id;
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())
}

/// Fetch HuggingFace README for a model and merge into its cached card.
/// `hf_repo_id` is the owner/repo form, e.g. "Qwen/Qwen3-8B".
#[tauri::command]
fn cmd_fetch_hf_card(model_path: String, hf_repo_id: String) -> Result<ModelCard, String> {
    let path = std::path::PathBuf::from(&model_path);
    let mut card = load_or_build_card(&path);

    let readme_url = format!("https://huggingface.co/{}/resolve/main/README.md", hf_repo_id);
    let readme = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .get(&readme_url)
        .call()
        .ok()
        .and_then(|r| r.into_string().ok())
        .unwrap_or_default();

    merge_hf_readme(&mut card, &readme, &hf_repo_id);
    save_card(&path, &card)?;
    Ok(card)
}

fn default_app_profiles() -> Vec<AppProfile> {
    vec![
        AppProfile { id: "general".into(), label: "General".into(), temperature_override: None, n_predict_override: None, ctx_cap_override: None },
        AppProfile { id: "coding-agent".into(), label: "Coding Agent".into(), temperature_override: Some(0.1), n_predict_override: Some(-1), ctx_cap_override: Some(8192) },
        AppProfile { id: "literary-cognition".into(), label: "Literary Cognition".into(), temperature_override: Some(0.85), n_predict_override: Some(-1), ctx_cap_override: None },
        AppProfile { id: "exotic-bass-maker".into(), label: "Exotic Bass Maker".into(), temperature_override: Some(0.7), n_predict_override: Some(512), ctx_cap_override: Some(4096) },
    ]
}

/// Write a .continue/config.json provider block into the given workspace directory.
/// Called after the server starts successfully.
#[tauri::command]
fn cmd_write_continue_config(
    workspace_path: String,
    state: State<Mutex<AppState>>,
) -> Result<(), String> {
    let guard = state.lock().map_err(|_| "state lock poisoned".to_string())?;
    let model_name = guard.instance.as_ref()
        .map(|i| model_name_from_path(&i.config.model_path))
        .unwrap_or_else(|| "local".to_string());
    drop(guard);

    let ws = std::path::PathBuf::from(&workspace_path);
    let config_dir = ws.join(".continue");
    std::fs::create_dir_all(&config_dir).map_err(|e| e.to_string())?;
    let config_path = config_dir.join("config.yaml");

    // Build a Continue-compatible YAML config with a single Autodetect model entry.
    // Use the server root (no /v1) and keep the exact model name detected by the app.
    let yaml = format!(
        "name: Local Config\nversion: 1.0.0\nschema: v1\n\nmodels:\n  - name: Autodetect\n    provider: openai\n    apiBase: http://127.0.0.1:8080\n    apiKey: \"\"\n    model: {}\n",
        model_name
    );

    std::fs::write(&config_path, yaml).map_err(|e| e.to_string())
}

// ── Browse / Scan model folder ────────────────────────────────────────────

#[tauri::command]
fn cmd_browse_model_folder() -> Result<String, String> {
    let folder = rfd::FileDialog::new()
        .set_title("Select GGUF Models Folder")
        .pick_folder();
    match folder {
        Some(path) => Ok(path.to_string_lossy().into_owned()),
        None => Err("cancelled".to_string()),
    }
}

#[tauri::command]
fn cmd_open_configurator(app: AppHandle, state: State<'_, Mutex<AppState>>) -> Result<String, String> {
    // Locate backend/configurator/gui.py relative to the executable (works both
    // in dev mode and in the installed bundle).
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));

    let configurator_entry = [
        exe_dir.join("backend").join("configurator").join("gui.py"),
        exe_dir.parent().map(|p| p.join("backend").join("configurator").join("gui.py")).unwrap_or_default(),
        PathBuf::from("backend").join("configurator").join("gui.py"),
    ]
    .into_iter()
    .find(|p| p.exists())
    .ok_or_else(|| "Configurator not found. Expected at backend/configurator/gui.py next to the app.".to_string())?;

    let configurator_dir = configurator_entry.parent()
        .ok_or("Cannot resolve configurator directory")?.to_path_buf();

    // Use .venv in the backend dir if present, otherwise fall back to system python.
    let venv_python = configurator_dir.parent()
        .map(|p| p.join(".venv").join("Scripts").join("python.exe"))
        .unwrap_or_else(|| PathBuf::from("python"));
    let venv_pythonw = venv_python.with_file_name("pythonw.exe");
    let python_cmd = if venv_pythonw.exists() {
        venv_pythonw
    } else if venv_python.exists() {
        venv_python
    } else {
        PathBuf::from("python")
    };

    // Spawn the configurator in a child process (do not forcibly suppress GUI windows)
    let mut cmd = Command::new(python_cmd);
    cmd.arg(&configurator_entry);
    cmd.current_dir(&configurator_dir);
    // Detach IO so Tauri isn't blocked by the child; keep GUI windows visible
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());

    match cmd.spawn() {
        Ok(child) => {
            let pid = child.id();
            if let Ok(mut st) = state.lock() {
                st.spawned_pids.push(pid);
                st.spawned_children.push(child);
            }
            emit_debug_log(&app, &format!("Configurator started pid {}", pid), false);
            Ok(format!("started pid {}", pid))
        }
        Err(e) => {
            emit_debug_log(&app, &format!("failed to spawn configurator: {}", e), true);
            Err(format!("failed to spawn configurator: {}", e))
        }
    }
}

#[tauri::command]
fn cmd_process_image(app: AppHandle, filename: String, data_url: String) -> Result<serde_json::Value, String> {
    // Save images into the app data uploads folder.
    let uploads = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".gguf-chatbox")
        .join("uploads");
    let uploads = uploads;
    if let Err(e) = std::fs::create_dir_all(&uploads) { return Err(e.to_string()); }

    // data_url is expected like: data:image/png;base64,AAAA...
    let comma_idx = data_url.find(',').ok_or_else(|| "Invalid data URL".to_string())?;
    let b64 = &data_url[comma_idx+1..];
    let bytes = base64::decode(b64).map_err(|e| e.to_string())?;

    let out_path = uploads.join(&filename);
    let mut f = File::create(&out_path).map_err(|e| e.to_string())?;
    f.write_all(&bytes).map_err(|e| e.to_string())?;

    // Placeholder analysis result (real vision adapter would run here)
    let res = json!({
        "ok": true,
        "saved_as": out_path.to_string_lossy().to_string(),
        "analysis": [ { "label": "example_object", "confidence": 0.978 } ]
    });
    Ok(res)
}

#[tauri::command]
fn cmd_launch_bsr(app: AppHandle, state: State<'_, Mutex<AppState>>, image_path: String, bsr_path: Option<String>) -> Result<String, String> {
    // Determine BSR executable path: prefer provided, then common workspace locations
    let candidate = if let Some(p) = bsr_path {
        PathBuf::from(p)
    } else {
        // No explicit path provided — caller must supply bsr_path.
        return Err("BSR executable path not provided. Pass the bsr_path argument.".to_string());
    };

    if !candidate.exists() {
        return Err(format!("BSR executable not found: {}", candidate.display()));
    }

    // Spawn BSR with the image path as argument (detached)
    let mut cmd = Command::new(candidate);
    cmd.arg(&image_path);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // Windows: avoid creating a visible console host window for the child.
    // Keep the child process attached to the parent (no CREATE_NEW_PROCESS_GROUP)
    // so the parent can reliably manage its lifecycle.
    #[cfg(windows)] { cmd.creation_flags(CREATE_NO_WINDOW); }

    match cmd.spawn() {
        Ok(child) => {
            let pid = child.id();
            if let Ok(mut st) = state.lock() {
                st.spawned_pids.push(pid);
                st.spawned_children.push(child);
            }
            Ok(format!("launched bsr pid {}", pid))
        }
        Err(e) => Err(format!("failed to spawn bsr: {}", e)),
    }
}

// Attempt to kill a process by PID. Uses `taskkill` on Windows and `kill -9` on Unix.
fn kill_pid(pid: u32) -> Result<(), String> {
    let s = pid.to_string();
    if cfg!(target_os = "windows") {
        let status = Command::new("taskkill")
            .args(&["/PID", &s, "/T", "/F"])
            .status()
            .map_err(|e| e.to_string())?;
        if status.success() { Ok(()) } else { Err(format!("taskkill failed: {:?}", status)) }
    } else {
        let status = Command::new("kill")
            .arg("-9")
            .arg(&s)
            .status()
            .map_err(|e| e.to_string())?;
        if status.success() { Ok(()) } else { Err(format!("kill failed: {:?}", status)) }
    }
}

// Attempt to gracefully terminate spawned Child processes with retries and logging.
fn terminate_spawned_children(mutex: &Mutex<AppState>) {
    // Take children out of state
    let children = if let Ok(mut st) = mutex.lock() {
        std::mem::take(&mut st.spawned_children)
    } else {
        Vec::new()
    };

    if children.is_empty() { return; }

    // Prepare log file under vens/logs
    let mut log_path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    log_path.push("vens");
    log_path.push("logs");
    let _ = std::fs::create_dir_all(&log_path);
    log_path.push("child_kill.log");

    let mut logger = match OpenOptions::new().create(true).append(true).open(&log_path) {
        Ok(f) => Some(f),
        Err(_) => None,
    };

    for mut child in children {
        let pid = child.id();
        if let Some(f) = logger.as_mut() {
            let _ = writeln!(f, "Attempting to terminate pid {}", pid);
        } else {
            eprintln!("Attempting to terminate pid {}", pid);
        }

        // First try graceful kill
        match child.kill() {
            Ok(()) => {
                if let Some(f) = logger.as_mut() { let _ = writeln!(f, "child.kill() sent to pid {}", pid); } else { eprintln!("child.kill() sent to pid {}", pid); }
            }
            Err(e) => {
                if let Some(f) = logger.as_mut() { let _ = writeln!(f, "child.kill() failed for pid {}: {}", pid, e); } else { eprintln!("child.kill() failed for pid {}: {}", pid, e); }
            }
        }

        // Wait up to 3 seconds for exit
        let mut waited = 0;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if let Some(f) = logger.as_mut() { let _ = writeln!(f, "pid {} exited: {:?}", pid, status); } else { eprintln!("pid {} exited: {:?}", pid, status); }
                    break;
                }
                Ok(None) => {
                    if waited >= 3000 { break; }
                    sleep(Duration::from_millis(200));
                    waited += 200;
                    continue;
                }
                Err(e) => {
                    if let Some(f) = logger.as_mut() { let _ = writeln!(f, "try_wait error for pid {}: {}", pid, e); } else { eprintln!("try_wait error for pid {}: {}", pid, e); }
                    break;
                }
            }
        }

        // If still running, fallback to platform kill
        if let Ok(None) = child.try_wait() {
            if let Some(f) = logger.as_mut() { let _ = writeln!(f, "pid {} did not exit, invoking fallback kill", pid); } else { eprintln!("pid {} did not exit, invoking fallback kill", pid); }
            if let Err(e) = kill_pid(pid) {
                if let Some(f) = logger.as_mut() { let _ = writeln!(f, "fallback kill failed for pid {}: {}", pid, e); } else { eprintln!("fallback kill failed for pid {}: {}", pid, e); }
            } else {
                if let Some(f) = logger.as_mut() { let _ = writeln!(f, "fallback kill succeeded for pid {}", pid); } else { eprintln!("fallback kill succeeded for pid {}", pid); }
            }
        }
    }

    // Windows-specific: find descendant processes (like conhost) of recorded PIDs and kill them,
    // and aggressively taskkill known lingering executables we spawn (e.g., llama-completion.exe).
    if cfg!(target_os = "windows") {
        if let Ok(st) = mutex.lock() {
            let pids = st.spawned_pids.clone();
            drop(st);
            for &p in &pids {
                // Use PowerShell to find child process IDs for parent p
                let ps_cmd = format!("Get-CimInstance Win32_Process -Filter \"ParentProcessId={}\" | Select-Object -ExpandProperty ProcessId", p);
                let output = Command::new("powershell").arg("-NoProfile").arg("-Command").arg(ps_cmd).output();
                if let Ok(out) = output {
                    if out.status.success() {
                        let s = String::from_utf8_lossy(&out.stdout);
                        for line in s.lines() {
                            if let Ok(child_pid) = line.trim().parse::<u32>() {
                                let _ = kill_pid(child_pid);
                            }
                        }
                    }
                }
            }

            // Aggressive: kill any lingering llama-completion.exe instances
            let _ = Command::new("taskkill").args(&["/IM", "llama-completion.exe", "/F"]).status();
            // Aggressive per-PID tree kill and logging
            for &p in &pids {
                let s = p.to_string();
                if let Ok(out) = Command::new("taskkill").args(&["/PID", &s, "/T", "/F"]).output() {
                    if let Some(mut f) = logger.as_ref() {
                        let _ = writeln!(f, "taskkill /PID {} exit: {}", s, out.status);
                        let _ = writeln!(f, "stdout: {}", String::from_utf8_lossy(&out.stdout));
                        let _ = writeln!(f, "stderr: {}", String::from_utf8_lossy(&out.stderr));
                    } else {
                        eprintln!("taskkill /PID {} exit: {:?}", s, out.status);
                    }
                }
            }
        }
    }
}

#[tauri::command]
fn cmd_vision_action(app: AppHandle, action: String, image_path: String) -> Result<serde_json::Value, String> {
    // Read the image file and base64-encode it.
    let img_bytes = std::fs::read(&image_path)
        .map_err(|e| format!("Cannot read image {}: {}", image_path, e))?;
    let b64 = base64::encode(&img_bytes);

    // Determine MIME type from extension.
    let ext = std::path::Path::new(&image_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("jpeg")
        .to_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "image/jpeg",
    };
    let data_url = format!("data:{};base64,{}", mime, b64);

    let prompt_text = match action.as_str() {
        "annotate" => "List all objects visible in this image with their approximate positions.",
        "ocr"      => "Extract all text visible in this image verbatim.",
        "custom"   => "Describe this image in as much detail as possible.",
        other      => return Err(format!("Unknown vision action: {}", other)),
    };

    emit_debug_log(&app, &format!("[vision] action={} image={}", action, image_path), false);

    let body = serde_json::json!({
        "model": "vision",
        "messages": [{
            "role": "user",
            "content": [
                { "type": "image_url", "image_url": { "url": data_url } },
                { "type": "text", "text": prompt_text }
            ]
        }],
        "max_tokens": 1024
    });

    let resp = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(120))
        .build()
        .post("http://127.0.0.1:8082/v1/chat/completions")
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
        .map_err(|e| format!("Vision server not reachable (is it started?): {}", e))?;

    let resp_body = resp.into_string()
        .map_err(|e| format!("Cannot read vision server response: {}", e))?;
    let resp_val: serde_json::Value = serde_json::from_str(&resp_body)
        .map_err(|e| format!("Bad JSON from vision server: {}", e))?;

    // Extract assistant reply text.
    let reply = resp_val["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .to_string();

    emit_debug_log(&app, &format!("[vision] reply ({} chars)", reply.len()), false);

    Ok(serde_json::json!({
        "ok": true,
        "action": action,
        "image": image_path,
        "result": reply,
        "raw": resp_val
    }))
}

#[tauri::command]
fn cmd_scan_model_folder(path: String) -> Vec<ModelSubfolder> {
    let dir = std::path::PathBuf::from(&path);
    let mut results = Vec::new();
    let mut root_files = Vec::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return results,
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            let mut gguf_files = Vec::new();
            if let Ok(sub_entries) = std::fs::read_dir(&p) {
                for sub_entry in sub_entries.flatten() {
                    let sp = sub_entry.path();
                    if sp.extension().and_then(|e| e.to_str()) != Some("gguf") { continue; }
                    let size_mb = std::fs::metadata(&sp).map(|m| m.len() / (1024 * 1024)).unwrap_or(0);
                    let context_length = gguf_context_length(&sp).unwrap_or(4096);
                    let layers = gguf_block_count(&sp).unwrap_or(0);
                    gguf_files.push(ModelInfo {
                        path: sp.to_string_lossy().into_owned(),
                        size_mb,
                        context_length,
                        layers,
                    });
                }
            }
            if !gguf_files.is_empty() {
                let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
                results.push(ModelSubfolder { name, path: p.to_string_lossy().into_owned(), gguf_files });
            }
        } else if p.extension().and_then(|e| e.to_str()) == Some("gguf") {
            let size_mb = std::fs::metadata(&p).map(|m| m.len() / (1024 * 1024)).unwrap_or(0);
            let context_length = gguf_context_length(&p).unwrap_or(4096);
            let layers = gguf_block_count(&p).unwrap_or(0);
            root_files.push(ModelInfo {
                path: p.to_string_lossy().into_owned(),
                size_mb,
                context_length,
                layers,
            });
        }
    }
    if !root_files.is_empty() {
        results.insert(0, ModelSubfolder {
            name: "(loose files)".to_string(),
            path: dir.to_string_lossy().into_owned(),
            gguf_files: root_files,
        });
    }
    results
}

// ── Voicebox TTS ──────────────────────────────────────────────────────────

#[tauri::command]
fn cmd_browse_voicebox_exe() -> Result<String, String> {
    let file = rfd::FileDialog::new()
        .set_title("Select Voicebox Executable")
        .add_filter("Executable", &["exe"])
        .pick_file();
    match file {
        Some(path) => Ok(path.to_string_lossy().into_owned()),
        None => Err("cancelled".to_string()),
    }
}

#[tauri::command]
fn cmd_launch_voicebox() -> Result<(), String> {
    let settings = cmd_get_settings();
    let exe_path = settings.voicebox_exe_path;
    if exe_path.is_empty() {
        return Err("Voicebox exe path not set. Use Browse to locate it.".to_string());
    }
    let path = std::path::PathBuf::from(&exe_path);
    if !path.exists() {
        return Err(format!("Voicebox not found at: {}", exe_path));
    }
    let working_dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    std::process::Command::new(&path)
        .current_dir(working_dir)
        .spawn()
        .map_err(|e| format!("Failed to launch Voicebox: {}", e))?;
    Ok(())
}

// ── Whisper STT ───────────────────────────────────────────────────────────

#[tauri::command]
fn cmd_browse_vision_model() -> Result<String, String> {
    let file = rfd::FileDialog::new()
        .set_title("Select Vision Model (.gguf)")
        .add_filter("GGUF Model", &["gguf"])
        .pick_file();
    match file {
        Some(path) => Ok(path.to_string_lossy().into_owned()),
        None => Err("cancelled".to_string()),
    }
}

#[tauri::command]
fn cmd_browse_mmproj() -> Result<String, String> {
    let file = rfd::FileDialog::new()
        .set_title("Select mmproj File (.gguf)")
        .add_filter("GGUF mmproj", &["gguf"])
        .pick_file();
    match file {
        Some(path) => Ok(path.to_string_lossy().into_owned()),
        None => Err("cancelled".to_string()),
    }
}

#[tauri::command]
fn cmd_browse_main_mmproj() -> Result<String, String> {
    let file = rfd::FileDialog::new()
        .set_title("Select mmproj for Main Server (.gguf)")
        .add_filter("GGUF mmproj", &["gguf"])
        .pick_file();
    match file {
        Some(path) => Ok(path.to_string_lossy().into_owned()),
        None => Err("cancelled".to_string()),
    }
}

/// Generic .gguf file picker with a caller-supplied dialog title. Used by the
/// CD-changer magazine loader (per-slot model + mmproj browse).
#[tauri::command]
fn cmd_browse_gguf_file(title: String) -> Result<String, String> {
    let file = rfd::FileDialog::new()
        .set_title(&title)
        .add_filter("GGUF", &["gguf"])
        .pick_file();
    match file {
        Some(path) => Ok(path.to_string_lossy().into_owned()),
        None => Err("cancelled".to_string()),
    }
}

#[tauri::command]
fn cmd_start_vision_server(app: AppHandle) -> Result<(), String> {
    let settings = cmd_get_settings();

    let vision_path = PathBuf::from(&settings.vision_model_path);
    let mmproj_path = PathBuf::from(&settings.mmproj_path);

    if settings.vision_model_path.is_empty() {
        return Err("Vision model path not set. Configure it in Advanced Settings.".to_string());
    }
    if settings.mmproj_path.is_empty() {
        return Err("mmproj path not set. Configure it in Advanced Settings.".to_string());
    }
    if !vision_path.exists() {
        return Err(format!("Vision model not found: {}", vision_path.display()));
    }
    if !mmproj_path.exists() {
        return Err(format!("mmproj file not found: {}", mmproj_path.display()));
    }

    // Stop any existing vision server first.
    let _ = cmd_stop_vision_server();

    let bin = adaptive_llama::resolve_llama_server_path();
    let vram_mb = adaptive_llama::query_vram_mb();
    let ctx: u32 = 4096;
    let (gpu_layers, _) = adaptive_llama::auto_gpu_layers(&vision_path, vram_mb, ctx);

    emit_debug_log(&app, &format!("[vision] Starting vision server on port 8082"), false);
    emit_debug_log(&app, &format!("[vision] Model:  {}", vision_path.display()), false);
    emit_debug_log(&app, &format!("[vision] mmproj: {}", mmproj_path.display()), false);
    emit_debug_log(&app, &format!("[vision] GPU layers: {}", gpu_layers), false);

    let mut cmd = Command::new(&bin);
    cmd.arg("--host").arg("127.0.0.1")
       .arg("--port").arg("8082")
       .arg("-m").arg(&vision_path)
       .arg("--mmproj").arg(&mmproj_path)
       .arg("--ctx-size").arg(ctx.to_string())
       .arg("--n-gpu-layers").arg(gpu_layers.to_string())
       .arg("--threads").arg("4")
       .stdin(Stdio::null())
       .stdout(Stdio::null())
       .stderr(Stdio::piped()); // capture stderr for debug terminal

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd.spawn().map_err(|e| format!("Failed to spawn vision server: {e}"))?;

    // Read stderr in a background thread and forward to the debug terminal.
    if let Some(stderr) = child.stderr.take() {
        let app_clone = app.clone();
        std::thread::spawn(move || {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(l) => {
                        let is_err = l.to_lowercase().contains("error")
                            || l.to_lowercase().contains("fail")
                            || l.to_lowercase().contains("panic");
                        emit_debug_log(&app_clone, &format!("[vision] {}", l), is_err);
                    }
                    Err(_) => break,
                }
            }
            emit_debug_log(&app_clone, "[vision] stderr pipe closed.", false);
        });
    }

    let mut guard = VISION_SERVER.lock().map_err(|_| "vision server mutex poisoned".to_string())?;
    *guard = Some(child);
    Ok(())
}

#[tauri::command]
fn cmd_stop_vision_server() -> Result<(), String> {
    let mut guard = VISION_SERVER.lock().map_err(|_| "vision server mutex poisoned".to_string())?;
    if let Some(ref mut child) = *guard {
        let _ = child.kill();
        let _ = child.wait();
    }
    *guard = None;
    Ok(())
}

#[derive(Serialize)]
struct VisionServerState {
    status: String,
    endpoint: String,
}

#[tauri::command]
fn cmd_vision_server_status() -> VisionServerState {
    // Check whether the process is alive.
    let alive = {
        let mut guard = VISION_SERVER.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(ref mut child) = *guard {
            match child.try_wait() {
                Ok(Some(_)) => { *guard = None; false } // exited
                Ok(None) => true,                       // still running
                Err(_) => false,
            }
        } else {
            false
        }
    };
    if !alive {
        return VisionServerState { status: "stopped".to_string(), endpoint: String::new() };
    }
    // Check health on port 8082.
    use std::io::{Read, Write as IoWrite};
    use std::net::TcpStream;
    let status = match TcpStream::connect_timeout(
        &"127.0.0.1:8082".parse().unwrap(),
        Duration::from_millis(300),
    ) {
        Err(_) => "starting",
        Ok(mut stream) => {
            let req = "GET /health HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n";
            if stream.write_all(req.as_bytes()).is_err() { "down" }
            else {
                let mut buf = [0u8; 256];
                let n = stream.read(&mut buf).unwrap_or(0);
                let body = String::from_utf8_lossy(&buf[..n]);
                if body.contains("\"ok\"") { "running" } else { "starting" }
            }
        }
    };
    VisionServerState {
        status: status.to_string(),
        endpoint: "http://127.0.0.1:8082/v1".to_string(),
    }
}

// ── Listening server helpers ─────────────────────────────────────────────

/// Find a suitable Python interpreter: prefer the gguf-chatbox-local venv,
/// then fall back to a bare `python` on PATH.
fn find_python_for_listening() -> PathBuf {
    let candidates = [
        r"C:\Python314\python.exe",
        r"C:\Python313\python.exe",
        r"C:\Python312\python.exe",
        r"C:\Python311\python.exe",
        r"C:\Python310\python.exe",
    ];
    for c in &candidates {
        let p = PathBuf::from(c);
        if p.exists() { return p; }
    }
    PathBuf::from("python") // hope it's on PATH
}

#[tauri::command]
fn cmd_browse_listening_model() -> Result<String, String> {
    let file = rfd::FileDialog::new()
        .set_title("Select Listening Model Weights (file or TF SavedModel dir)")
        .add_filter("Model Weights", &["h5", "pb", "json", "pt", "pth", "ckpt", "bin"])
        .add_filter("All Files", &["*"])
        .pick_file();
    match file {
        Some(path) => Ok(path.to_string_lossy().into_owned()),
        None => Err("cancelled".to_string()),
    }
}

#[tauri::command]
fn cmd_browse_audio_file() -> Result<String, String> {
    let file = rfd::FileDialog::new()
        .set_title("Select Audio File")
        .add_filter("Audio Files", &["mp3", "wav", "flac", "ogg", "m4a", "aac", "wma", "opus", "aiff", "ape"])
        .add_filter("All Files", &["*"])
        .pick_file();
    match file {
        Some(path) => Ok(path.to_string_lossy().into_owned()),
        None => Err("cancelled".to_string()),
    }
}

#[tauri::command]
fn cmd_start_listening_server(app: AppHandle, model_path: String) -> Result<(), String> {
    let _ = cmd_stop_listening_server();

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    let script = [
        exe_dir.join("backend").join("listening_server.py"),
        exe_dir.parent().map(|p| p.join("backend").join("listening_server.py")).unwrap_or_default(),
        PathBuf::from("backend").join("listening_server.py"),
    ]
    .into_iter()
    .find(|p| p.exists())
    .ok_or_else(|| "Listening server script not found. Expected at backend/listening_server.py next to the app.".to_string())?;

    let python = find_python_for_listening();
    emit_debug_log(&app, &format!("[listening] Python: {}", python.display()), false);
    emit_debug_log(&app, "[listening] Starting listening server on port 8083", false);
    if !model_path.is_empty() {
        emit_debug_log(&app, &format!("[listening] Model: {}", model_path), false);
    }

    let mut cmd = Command::new(&python);
    cmd.arg(&script)
       .arg("--port").arg("8083");
    if !model_path.is_empty() {
        cmd.arg("--model-path").arg(&model_path);
    }
    cmd.stdin(Stdio::null())
       .stdout(Stdio::piped())
       .stderr(Stdio::piped());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd.spawn().map_err(|e| format!("Failed to spawn listening server: {e}"))?;

    if let Some(stderr) = child.stderr.take() {
        let app_clone = app.clone();
        std::thread::spawn(move || {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(l) => {
                        let is_err = l.to_lowercase().contains("error")
                            || l.to_lowercase().contains("fail")
                            || l.to_lowercase().contains("traceback");
                        emit_debug_log(&app_clone, &format!("[listening] {}", l), is_err);
                    }
                    Err(_) => break,
                }
            }
            emit_debug_log(&app_clone, "[listening] server stderr pipe closed.", false);
        });
    }

    let mut guard = LISTENING_SERVER.lock().map_err(|_| "listening server mutex poisoned".to_string())?;
    *guard = Some(child);
    Ok(())
}

#[tauri::command]
fn cmd_stop_listening_server() -> Result<(), String> {
    let mut guard = LISTENING_SERVER.lock().map_err(|_| "listening server mutex poisoned".to_string())?;
    if let Some(ref mut child) = *guard {
        let _ = child.kill();
        let _ = child.wait();
    }
    *guard = None;
    Ok(())
}

#[derive(Serialize)]
struct ListeningServerState {
    status: String,
    endpoint: String,
}

#[tauri::command]
fn cmd_listening_server_status() -> ListeningServerState {
    let alive = {
        let mut guard = LISTENING_SERVER.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(ref mut child) = *guard {
            match child.try_wait() {
                Ok(Some(_)) => { *guard = None; false }
                Ok(None) => true,
                Err(_) => false,
            }
        } else { false }
    };
    if !alive {
        return ListeningServerState { status: "stopped".to_string(), endpoint: String::new() };
    }
    use std::io::{Read, Write as IoWrite};
    use std::net::TcpStream;
    let status = match TcpStream::connect_timeout(
        &"127.0.0.1:8083".parse().unwrap(),
        Duration::from_millis(300),
    ) {
        Err(_) => "starting",
        Ok(mut stream) => {
            let req = "GET /health HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n";
            if stream.write_all(req.as_bytes()).is_err() { "starting" }
            else {
                let mut buf = [0u8; 256];
                let n = stream.read(&mut buf).unwrap_or(0);
                let body = String::from_utf8_lossy(&buf[..n]);
                if body.contains("\"ok\"") || body.contains("200") { "running" } else { "starting" }
            }
        }
    };
    ListeningServerState {
        status: status.to_string(),
        endpoint: "http://127.0.0.1:8083".to_string(),
    }
}

#[tauri::command]
fn cmd_listening_action(app: AppHandle, action: String, file_path: String) -> Result<serde_json::Value, String> {
    // Validate action
    let valid = matches!(action.as_str(), "generate_tags" | "transcribe" | "review");
    if !valid {
        return Err(format!("Unknown listening action: {}", action));
    }

    // Validate file path (must be an absolute path to an existing file)
    let p = PathBuf::from(&file_path);
    if !p.is_absolute() {
        return Err("file_path must be an absolute path".to_string());
    }
    if !p.exists() {
        return Err(format!("Audio file not found: {}", file_path));
    }

    emit_debug_log(&app, &format!("[listening] action={} file={}", action, file_path), false);

    let body = serde_json::json!({ "action": action, "file_path": file_path });

    let resp = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(120))
        .build()
        .post("http://127.0.0.1:8083/action")
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
        .map_err(|e| format!("Listening server not reachable (start it in the SERVER tray): {}", e))?;

    let resp_body = resp.into_string()
        .map_err(|e| format!("Cannot read listening server response: {}", e))?;
    let resp_val: serde_json::Value = serde_json::from_str(&resp_body)
        .map_err(|e| format!("Bad JSON from listening server: {}\nRaw: {}", e, resp_body))?;

    emit_debug_log(&app, &format!("[listening] {} complete", action), false);
    Ok(resp_val)
}

// ── Whisper STT ───────────────────────────────────────────────────────────

#[tauri::command]
fn cmd_browse_whisper_exe() -> Result<String, String> {
    let file = rfd::FileDialog::new()
        .set_title("Select whisper-cli Executable")
        .add_filter("Executable", &["exe"])
        .pick_file();
    match file {
        Some(path) => Ok(path.to_string_lossy().into_owned()),
        None => Err("cancelled".to_string()),
    }
}

#[tauri::command]
fn cmd_browse_whisper_model() -> Result<String, String> {
    let file = rfd::FileDialog::new()
        .set_title("Select Whisper GGML Model")
        .add_filter("GGML Model", &["bin"])
        .pick_file();
    match file {
        Some(path) => Ok(path.to_string_lossy().into_owned()),
        None => Err("cancelled".to_string()),
    }
}

#[tauri::command]
fn cmd_transcribe_audio(audio_data: Vec<u8>) -> Result<String, String> {
    let settings = cmd_get_settings();
    if settings.whisper_exe_path.is_empty() {
        return Err("Whisper exe path not set. Go to Advanced Settings to configure it.".to_string());
    }
    if settings.whisper_model_path.is_empty() {
        return Err("Whisper model not set. Go to Advanced Settings to configure it.".to_string());
    }
    let exe = std::path::PathBuf::from(&settings.whisper_exe_path);
    if !exe.exists() {
        return Err(format!("whisper-cli not found at: {}", settings.whisper_exe_path));
    }
    let model = std::path::PathBuf::from(&settings.whisper_model_path);
    if !model.exists() {
        return Err(format!("Whisper model not found at: {}", settings.whisper_model_path));
    }

    let tmp = std::env::temp_dir().join("gguf_chatbox_mic.wav");
    std::fs::write(&tmp, &audio_data).map_err(|e| e.to_string())?;

    let output = std::process::Command::new(&exe)
        .arg("-m").arg(&model)
        .arg("-f").arg(&tmp)
        .arg("--no-timestamps")
        .arg("-nt")
        .arg("-l").arg("en")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| format!("Failed to run whisper-cli: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("whisper-cli error: {}", stderr.trim()));
    }

    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let _ = std::fs::remove_file(&tmp);
    Ok(text)
}

// ── Multimodal routing ────────────────────────────────────────────────────

/// Returns true if the currently loaded main model appears to support vision
/// (image_url) input — checked via architecture string and settings fallback.
#[tauri::command]
fn cmd_check_main_model_has_vision(state: State<Mutex<AppState>>) -> bool {
    let model_path = {
        let guard = match state.lock() {
            Ok(g) => g,
            Err(_) => return false,
        };
        guard.instance.as_ref().map(|i| i.config.model_path.clone())
    };
    let model_path = match model_path {
        Some(p) => p,
        None => return false,
    };
    let card = load_or_build_card(std::path::Path::new(&model_path));
    let arch = card.architecture.to_lowercase();
    if arch.contains("llava") || arch.contains("moondream") || arch.contains("minicpm")
        || arch.contains("idefics") || arch.contains("internvl") || arch.contains("bakllava")
        || arch.contains("bunny") || arch.contains("vision")
    {
        return true;
    }
    // Fallback: user loaded the dedicated vision model as the main model.
    let settings = cmd_get_settings();
    !settings.vision_model_path.is_empty() && settings.vision_model_path == model_path
}

/// Same as cmd_vision_action but routes to the main text server (port 8080)
/// instead of the dedicated vision server (port 8082).
#[tauri::command]
fn cmd_vision_action_main(app: AppHandle, action: String, image_path: String) -> Result<serde_json::Value, String> {
    let img_bytes = std::fs::read(&image_path)
        .map_err(|e| format!("Cannot read image {}: {}", image_path, e))?;
    let b64 = base64::encode(&img_bytes);

    let ext = std::path::Path::new(&image_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("jpeg")
        .to_lowercase();
    let mime = match ext.as_str() {
        "png"  => "image/png",
        "gif"  => "image/gif",
        "webp" => "image/webp",
        _      => "image/jpeg",
    };
    let data_url = format!("data:{};base64,{}", mime, b64);

    let prompt_text = match action.as_str() {
        "annotate" => "List all objects visible in this image with their approximate positions.",
        "ocr"      => "Extract all text visible in this image verbatim.",
        "custom"   => "Describe this image in as much detail as possible.",
        other      => return Err(format!("Unknown vision action: {}", other)),
    };

    emit_debug_log(&app, &format!("[vision-main] action={} image={}", action, image_path), false);

    let body = serde_json::json!({
        "model": "local",
        "messages": [{
            "role": "user",
            "content": [
                { "type": "image_url", "image_url": { "url": data_url } },
                { "type": "text", "text": prompt_text }
            ]
        }],
        "max_tokens": 1024
    });

    let resp = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(120))
        .build()
        .post("http://127.0.0.1:8080/v1/chat/completions")
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
        .map_err(|e| format!("Main model server not reachable (is it started?): {}", e))?;

    let resp_body = resp.into_string()
        .map_err(|e| format!("Cannot read main model response: {}", e))?;
    let resp_val: serde_json::Value = serde_json::from_str(&resp_body)
        .map_err(|e| format!("Bad JSON from main model: {}", e))?;

    let reply = resp_val["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .to_string();

    emit_debug_log(&app, &format!("[vision-main] reply ({} chars)", reply.len()), false);

    Ok(serde_json::json!({
        "ok": true,
        "action": action,
        "image": image_path,
        "result": reply,
        "raw": resp_val
    }))
}

/// Transcribes an audio file via whisper-cli then sends the transcript to the
/// main model (port 8080) for reasoning.  Used when "Use main multimodal model"
/// is toggled ON in the Listening panel.
#[tauri::command]
fn cmd_audio_transcribe_then_reason(app: AppHandle, file_path: String, action: String) -> Result<serde_json::Value, String> {
    let settings = cmd_get_settings();
    if settings.whisper_exe_path.is_empty() {
        return Err("Whisper exe path not set. Configure it in Advanced Settings.".to_string());
    }
    if settings.whisper_model_path.is_empty() {
        return Err("Whisper model not set. Configure it in Advanced Settings.".to_string());
    }
    let exe = std::path::PathBuf::from(&settings.whisper_exe_path);
    if !exe.exists() {
        return Err(format!("whisper-cli not found at: {}", settings.whisper_exe_path));
    }
    let model = std::path::PathBuf::from(&settings.whisper_model_path);
    if !model.exists() {
        return Err(format!("Whisper model not found at: {}", settings.whisper_model_path));
    }
    let audio = std::path::PathBuf::from(&file_path);
    if !audio.exists() {
        return Err(format!("Audio file not found: {}", file_path));
    }

    emit_debug_log(&app, &format!("[audio-main] transcribing: {}", file_path), false);

    let output = std::process::Command::new(&exe)
        .arg("-m").arg(&model)
        .arg("-f").arg(&audio)
        .arg("--no-timestamps")
        .arg("-nt")
        .arg("-l").arg("en")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| format!("Failed to run whisper-cli: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("whisper-cli error: {}", stderr.trim()));
    }

    let transcript = String::from_utf8_lossy(&output.stdout).trim().to_string();
    emit_debug_log(&app, &format!("[audio-main] transcript {} chars, action={}", transcript.len(), action), false);

    // For plain transcription, return the result directly without an LLM call.
    if action == "transcribe" {
        return Ok(serde_json::json!({
            "ok": true,
            "action": "transcribe",
            "file": file_path,
            "transcription": { "source": "whisper-cli", "language": "en", "text": transcript }
        }));
    }

    let system_prompt = match action.as_str() {
        "generate_tags" => "You are a music analyst. Given an audio transcript, infer and return a JSON object with keys: genre, mood, energy, tempo, and tags (array of strings). Be concise.",
        "review"        => "You are an audio content reviewer. Given a transcript, provide a structured review covering: content summary, tone, notable moments, and quality assessment.",
        other           => return Err(format!("Unknown audio action: {}", other)),
    };

    let user_prompt = format!("Audio file: {}\n\nTranscript:\n{}", file_path, transcript);

    let body = serde_json::json!({
        "model": "local",
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user",   "content": user_prompt }
        ],
        "max_tokens": 1024
    });

    let resp = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(120))
        .build()
        .post("http://127.0.0.1:8080/v1/chat/completions")
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
        .map_err(|e| format!("Main model server not reachable (is it started?): {}", e))?;

    let resp_body = resp.into_string()
        .map_err(|e| format!("Cannot read main model response: {}", e))?;
    let resp_val: serde_json::Value = serde_json::from_str(&resp_body)
        .map_err(|e| format!("Bad JSON from main model: {}", e))?;

    let reply = resp_val["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .to_string();

    emit_debug_log(&app, &format!("[audio-main] reply ({} chars)", reply.len()), false);

    Ok(serde_json::json!({
        "ok": true,
        "action": action,
        "file": file_path,
        "transcript": transcript,
        "result": reply,
        "raw": resp_val
    }))
}

// ── Voicebox TTS ──────────────────────────────────────────────────────────

#[tauri::command]
fn cmd_voicebox_status(url: String) -> bool {
    ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .get(&url)
        .call()
        .is_ok()
}

#[tauri::command]
fn cmd_voicebox_profiles(url: String) -> Result<String, String> {
    let resp = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .get(&format!("{}/profiles", url))
        .call()
        .map_err(|e| format!("Voicebox not reachable: {}", e))?;
    resp.into_string().map_err(|e| e.to_string())
}

#[tauri::command]
fn cmd_voicebox_speak(url: String, text: String, profile_id: String) -> Result<String, String> {
    let body = serde_json::json!({
        "text": text,
        "profile_id": profile_id,
        "language": "en"
    });
    let resp = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .post(&format!("{}/generate", url))
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
        .map_err(|e| format!("Voicebox error: {}", e))?;
    resp.into_string().map_err(|e| e.to_string())
}

// ── HuggingFace model download ────────────────────────────────────────────

#[tauri::command]
fn cmd_download_hf_model(url: String, app: AppHandle) -> Result<(), String> {
    // Accept any https:// URL ending in .gguf — not restricted to /resolve/ links.
    if !url.starts_with("https://") {
        return Err("URL must start with https://".into());
    }
    if url.contains("/tree/") || url.contains("/blob/") {
        return Err(
            "That is a repository browser page, not a direct file link. \
             Navigate to the file, click the download icon, and copy that URL.".into()
        );
    }
    let filename = url.rsplit('/')
        .next()
        .unwrap_or("model.gguf")
        .split('?')
        .next()
        .unwrap_or("model.gguf")
        .to_string();
    if !filename.to_lowercase().ends_with(".gguf") {
        return Err(format!(
            "No .gguf extension found in URL. Got '{}'. Paste a direct download link to a .gguf file.",
            filename
        ));
    }

    let settings = cmd_get_settings();
    let base_dir = if !settings.models_folder.is_empty() {
        PathBuf::from(&settings.models_folder)
    } else {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".gguf-chatbox")
            .join("models")
    };
    let stem = filename.trim_end_matches(".gguf").trim_end_matches(".GGUF").to_string();
    let sub_dir = base_dir.join(&stem);
    std::fs::create_dir_all(&sub_dir).map_err(|e| e.to_string())?;
    let dest = sub_dir.join(&filename);
    let part = sub_dir.join(format!("{}.part", filename));

    let app2 = app.clone();
    let log = move |msg: String, done: bool, error: bool| {
        app2.emit("hf-download-log", HfDownloadLog { line: msg, done, error }).ok();
    };

    std::thread::spawn(move || {
        let existing: u64 = if part.exists() {
            std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0)
        } else { 0 };

        if existing > 0 {
            log(format!("\u{21bb} Resuming from {:.1} MB", existing as f64 / 1_048_576.0), false, false);
        }
        log(format!("\u{2192} GET {}", url), false, false);
        log(format!("  target: {}", dest.display()), false, false);

        let agent = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(30))
            .timeout_read(std::time::Duration::from_secs(300))
            .build();
        let mut req = agent.get(&url);
        if existing > 0 {
            req = req.set("Range", &format!("bytes={}-", existing));
        }
        let resp = match req.call() {
            Ok(r) => r,
            Err(e) => { log(format!("\u{2717} HTTP error: {}", e), true, true); return; }
        };

        let ct = resp.content_type().to_lowercase();
        if ct.contains("text/html") || ct.contains("application/json") {
            log("\u{2717} Server returned a web page, not a model file. Check your URL \u{2014} you need a direct .gguf download link.".into(), true, true);
            return;
        }

        let status = resp.status();
        let resumed = status == 206 && existing > 0;
        let content_len: Option<u64> = resp.header("content-length").and_then(|v| v.parse().ok());
        let total: Option<u64> = if resumed {
            content_len.map(|cl| cl + existing)
        } else {
            content_len
        };
        let start_offset: u64 = if resumed { existing } else { 0 };

        if let Some(t) = total {
            log(format!("  size: {:.1} MB", t as f64 / 1_048_576.0), false, false);
        }
        if resumed {
            log(format!("  server accepted resume at byte {}", existing), false, false);
        } else if existing > 0 {
            log("  server did not accept resume \u{2014} restarting from scratch".into(), false, false);
        }

        let mut reader = resp.into_reader();
        let mut file = match (if resumed {
            std::fs::OpenOptions::new().append(true).open(&part)
        } else {
            std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(&part)
        }) {
            Ok(f) => f,
            Err(e) => { log(format!("\u{2717} Cannot open file: {}", e), true, true); return; }
        };

        let mut downloaded: u64 = start_offset;
        let mut buf = vec![0u8; 1_048_576];
        let mut last_pct: u64 = total
            .map(|t| if t > 0 { (start_offset * 100) / t } else { 0 })
            .unwrap_or(0);

        loop {
            let n = match std::io::Read::read(&mut reader, &mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) => { log(format!("\u{2717} Read error: {}", e), true, true); return; }
            };
            if let Err(e) = std::io::Write::write_all(&mut file, &buf[..n]) {
                log(format!("\u{2717} Write error: {}", e), true, true);
                return;
            }
            downloaded += n as u64;
            if let Some(t) = total {
                let pct = if t > 0 { (downloaded * 100) / t } else { 0 };
                if pct != last_pct {
                    last_pct = pct;
                    log(format!("  {}% \u{2014} {:.1} / {:.1} MB",
                        pct,
                        downloaded as f64 / 1_048_576.0,
                        t as f64 / 1_048_576.0,
                    ), false, false);
                }
            } else if downloaded % (10 * 1_048_576) == 0 {
                log(format!("  {:.1} MB downloaded", downloaded as f64 / 1_048_576.0), false, false);
            }
        }

        drop(file);
        if let Err(e) = std::fs::rename(&part, &dest) {
            log(format!("\u{2717} Rename failed: {} \u{2014} .part file kept for resume", e), true, true);
            return;
        }
        log(format!("\u{2713} Download complete: {}", filename), true, false);

        // Record hf_repo_id in the model card cache.
        // HF direct URLs look like: .../resolve/main/file.gguf or .../blob/main/file.gguf
        let hf_repo_id: String = {
            // Extract owner/repo from URL segments between huggingface.co/ and /resolve or /blob
            let after_host = url.trim_start_matches("https://huggingface.co/");
            let repo = after_host.splitn(3, '/').take(2).collect::<Vec<_>>().join("/");
            if repo.contains('/') { repo } else { String::new() }
        };
        if !hf_repo_id.is_empty() {
            let mut card = load_or_build_card(&dest);
            if card.hf_repo_id.is_empty() {
                card.hf_repo_id = hf_repo_id;
                let _ = save_card(&dest, &card);
            }
        }
    });
    Ok(())
}

// ── Hot Rod Tuner link ────────────────────────────────────────────────────

/// Link to Hot Rod Tuner running on localhost.
/// Sends our exe path + PID so HRT can e-stop this process.
#[tauri::command]
fn cmd_link_hrt(port: u16) -> Result<String, String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("cannot resolve exe path: {}", e))?
        .to_string_lossy()
        .into_owned();
    let pid = std::process::id();

    let url = format!("http://127.0.0.1:{}/link", port);
    let body = serde_json::json!({
        "app_name": "GGUF Chatbox",
        "exe_path": exe,
        "pid": pid
    });

    let resp = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .post(&url)
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
        .map_err(|e| format!("HRT not reachable: {}", e))?;

    if resp.status() == 200 {
        Ok("HRT Link Successful".into())
    } else {
        Err(format!("HRT responded with status {}", resp.status()))
    }
}

// ── Startup splash ────────────────────────────────────────────────────────

#[derive(Clone, Serialize)]
struct StartupProgress {
    message: String,
}

#[tauri::command]
fn cmd_startup(app: AppHandle) -> Result<(), String> {
    app.emit("startup-progress", StartupProgress {
        message: "Checking configuration…".to_string(),
    }).map_err(|e: tauri::Error| e.to_string())?;
    std::thread::sleep(std::time::Duration::from_millis(800));

    // Auto-install llama.cpp if not found
    if adaptive_llama::detect_llama().is_none() {
        app.emit("startup-progress", StartupProgress {
            message: "llama.cpp not found — downloading…".to_string(),
        }).map_err(|e: tauri::Error| e.to_string())?;

        match install_llama_cpp(&app) {
            Ok(_) => {
                app.emit("startup-progress", StartupProgress {
                    message: "llama.cpp installed successfully.".to_string(),
                }).ok();
            }
            Err(e) => {
                eprintln!("[startup] llama.cpp install failed: {}", e);
                app.emit("startup-progress", StartupProgress {
                    message: format!("llama.cpp install failed: {}", e),
                }).ok();
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(600));
    }

    app.emit("startup-progress", StartupProgress {
        message: "Scanning models folder…".to_string(),
    }).map_err(|e: tauri::Error| e.to_string())?;
    std::thread::sleep(std::time::Duration::from_millis(800));

    app.emit("startup-progress", StartupProgress {
        message: "Initialising engine…".to_string(),
    }).map_err(|e: tauri::Error| e.to_string())?;
    std::thread::sleep(std::time::Duration::from_millis(800));

    app.emit("startup-progress", StartupProgress {
        message: "Ready.".to_string(),
    }).map_err(|e: tauri::Error| e.to_string())?;
    std::thread::sleep(std::time::Duration::from_millis(400));

    // Show main window
    if let Some(w) = app.get_webview_window("main") {
        w.show().map_err(|e| e.to_string())?;
    }

    // Close splash
    if let Some(w) = app.get_webview_window("splash") {
        w.close().map_err(|e| e.to_string())?;
    }

    Ok(())
}

// ── Debug log helper ──────────────────────────────────────────────────────

#[derive(Clone, Serialize)]
struct DebugLogEntry {
    line: String,
    error: bool,
}

fn emit_debug_log(app: &AppHandle, line: &str, is_error: bool) {
    let _ = app.emit("debug-log", DebugLogEntry {
        line: line.to_string(),
        error: is_error,
    });
}

// ── Window show/hide commands ─────────────────────────────────────────────

#[tauri::command]
fn cmd_show_advanced_settings(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("advanced-settings") {
        w.show().map_err(|e| e.to_string())?;
        w.set_focus().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn cmd_show_debug_terminal(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("debug-terminal") {
        w.show().map_err(|e| e.to_string())?;
        w.set_focus().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn cmd_log(line: String, is_error: Option<bool>) -> Result<(), String> {
    if is_error.unwrap_or(false) {
        eprintln!("[renderer] {}", line);
    } else {
        println!("[renderer] {}", line);
    }
    Ok(())
}

// ── llama.cpp auto-installer ──────────────────────────────────────────────

fn install_llama_cpp(app: &AppHandle) -> Result<String, String> {
    use std::io::Read;

    let install_dir = adaptive_llama::llama_install_dir();
    std::fs::create_dir_all(&install_dir)
        .map_err(|e| format!("Failed to create install dir: {}", e))?;

    // Detect NVIDIA GPU
    let has_nvidia = std::process::Command::new("nvidia-smi")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    let (asset_keyword, fallback_keyword) = if has_nvidia {
        ("bin-win-cuda-12.4-x64", "bin-win-cpu-x64")
    } else {
        ("bin-win-cpu-x64", "bin-win-cpu-x64")
    };

    app.emit("startup-progress", StartupProgress {
        message: "Querying latest llama.cpp release…".to_string(),
    }).ok();

    let api_agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(30))
        .build();

    let dl_agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(600))
        .build();

    let release_str = api_agent
        .get("https://api.github.com/repos/ggerganov/llama.cpp/releases/latest")
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("GitHub API error: {}", e))?
        .into_string()
        .map_err(|e| format!("read response failed: {}", e))?;

    let release: serde_json::Value = serde_json::from_str(&release_str)
        .map_err(|e| format!("JSON parse error: {}", e))?;

    let assets = release["assets"]
        .as_array()
        .ok_or("no assets in release")?;

    let find_asset = |keyword: &str| -> Option<(String, String)> {
        assets.iter().find_map(|a| {
            let name = a["name"].as_str().unwrap_or("");
            let url = a["browser_download_url"].as_str().unwrap_or("");
            if name.contains(keyword) && name.starts_with("llama-") && name.ends_with(".zip") {
                Some((name.to_string(), url.to_string()))
            } else {
                None
            }
        })
    };

    let (asset_name, download_url) = find_asset(asset_keyword)
        .or_else(|| find_asset(fallback_keyword))
        .ok_or("could not find a suitable llama.cpp release asset")?;

    // Also grab cudart if CUDA
    let cudart_url = if has_nvidia {
        assets.iter().find_map(|a| {
            let name = a["name"].as_str().unwrap_or("");
            let url = a["browser_download_url"].as_str().unwrap_or("");
            if name.starts_with("cudart-") && name.contains("cuda-12.4") && name.ends_with(".zip") {
                Some(url.to_string())
            } else {
                None
            }
        })
    } else {
        None
    };

    app.emit("startup-progress", StartupProgress {
        message: format!("Downloading {}…", asset_name),
    }).ok();

    let download_and_extract = |url: &str, label: &str| -> Result<(), String> {
        let resp = dl_agent.get(url)
            .call()
            .map_err(|e| format!("download {} failed: {}", label, e))?;

        let mut bytes = Vec::new();
        resp.into_reader()
            .read_to_end(&mut bytes)
            .map_err(|e| format!("read {} failed: {}", label, e))?;

        let cursor = std::io::Cursor::new(&bytes);
        let mut archive = zip::ZipArchive::new(cursor)
            .map_err(|e| format!("zip open {} failed: {}", label, e))?;

        for i in 0..archive.len() {
            let mut file = archive.by_index(i)
                .map_err(|e| format!("zip entry error: {}", e))?;
            let name = file.name().to_string();
            if name.ends_with('/') { continue; }
            let file_name = std::path::Path::new(&name)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            if file_name.is_empty() { continue; }

            let out_path = install_dir.join(&file_name);
            let mut out_file = std::fs::File::create(&out_path)
                .map_err(|e| format!("create file {} failed: {}", file_name, e))?;
            std::io::copy(&mut file, &mut out_file)
                .map_err(|e| format!("extract {} failed: {}", file_name, e))?;
        }
        Ok(())
    };

    download_and_extract(&download_url, "llama.cpp")?;

    app.emit("startup-progress", StartupProgress {
        message: "Binaries extracted.".to_string(),
    }).ok();

    if let Some(cudart) = &cudart_url {
        app.emit("startup-progress", StartupProgress {
            message: "Downloading CUDA runtime…".to_string(),
        }).ok();
        download_and_extract(cudart, "cudart")?;
    }

    let llama_path = adaptive_llama::llama_local_path();
    if !llama_path.is_file() {
        return Err(format!("Installation finished but {} not found in {}",
            adaptive_llama::LLAMA_BIN, install_dir.display()));
    }

    app.emit("startup-progress", StartupProgress {
        message: "llama.cpp installed successfully.".to_string(),
    }).ok();

    Ok(llama_path.to_string_lossy().into_owned())
}

// ── Entry point ───────────────────────────────────────────────────────────

/// Binds a dedicated port to act as a single-instance lock.
/// Returns the listener so it stays alive for the process lifetime.
/// Exits immediately if another instance already holds the port.
fn single_instance_guard() -> std::net::TcpListener {
    match std::net::TcpListener::bind("127.0.0.1:19876") {
        Ok(l) => l,
        Err(_) => {
            // Another instance already owns the lock. Nudge it to the
            // foreground (handled by the accept-loop in setup) so the user sees
            // the running window instead of a silent no-op, then exit this copy.
            let _ = std::net::TcpStream::connect("127.0.0.1:19876");
            eprintln!("[startup] GGUF Chatbox is already running — focusing the existing window.");
            std::process::exit(0);
        }
    }
}

fn main() {
    // Prevent duplicate instances — a second launch focuses the running window.
    let instance_lock = single_instance_guard();

    // Guaranteed exit on panic — no headless zombies if the app crashes.
    std::panic::set_hook(Box::new(|info| {
        eprintln!("[panic] {info}");
        std::process::exit(1);
    }));

    tauri::Builder::default()
        .manage(Mutex::new(AppState {
            instance: None,
            chat_history: Vec::new(),
            system_prompt: String::new(),
            echo_config: EchoConfig::default(),
            echo_source: None,
            echo_fired: Vec::new(),
            echo_enabled: true,
            spawned_children: Vec::new(),
            spawned_pids: Vec::new(),
        }))
        .setup(move |app| {
            // Single-instance focus: when a duplicate launch connects to our
            // lock port, bring the existing main window to the foreground
            // instead of silently exiting (which used to leave the user
            // clicking a hidden/orphaned window and thinking buttons were dead).
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                for stream in instance_lock.incoming() {
                    if stream.is_err() {
                        continue;
                    }
                    let h = handle.clone();
                    // Window ops must run on the main thread.
                    let _ = handle.run_on_main_thread(move || {
                        if let Some(w) = h.get_webview_window("main") {
                            let _ = w.unminimize();
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    });
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            cmd_list_models,
            cmd_load_model,
            cmd_send_message,
            cmd_stop_generation,
            cmd_unload_model,
            cmd_set_echo_enabled,
            cmd_set_system_prompt,
            cmd_set_echo_config,
            cmd_get_echo_state,
            cmd_start_server,
            cmd_stop_server,
            cmd_server_status,
            cmd_write_continue_config,
            cmd_get_model_stats,
            cmd_get_settings,
            cmd_update_settings,
            cmd_link_hrt,
            cmd_startup,
            cmd_show_advanced_settings,
            cmd_show_debug_terminal,
            cmd_browse_model_folder,
            cmd_scan_model_folder,
            cmd_browse_voicebox_exe,
            cmd_launch_voicebox,
            cmd_browse_whisper_exe,
            cmd_browse_whisper_model,
            cmd_transcribe_audio,
            cmd_check_main_model_has_vision,
            cmd_vision_action_main,
            cmd_audio_transcribe_then_reason,
            cmd_voicebox_status,
            cmd_voicebox_profiles,
            cmd_voicebox_speak,
            cmd_download_hf_model,
            cmd_reload_gpu_layers,
            cmd_log,
            cmd_open_configurator,
            cmd_process_image,
            cmd_launch_bsr,
            cmd_vision_action,
            cmd_browse_vision_model,
            cmd_browse_gguf_file,
            cmd_browse_mmproj,
            cmd_browse_main_mmproj,
            cmd_start_vision_server,
            cmd_stop_vision_server,
            cmd_vision_server_status,
            cmd_browse_listening_model,
            cmd_browse_audio_file,
            cmd_start_listening_server,
            cmd_stop_listening_server,
            cmd_listening_server_status,
            cmd_listening_action,
            cmd_get_model_card,
            cmd_get_app_profiles,
            cmd_set_active_profile,
            cmd_fetch_hf_card,
        ])
        .on_window_event(|window, event| {
            // If the main window is destroyed by a crash (not just close-requested),
            // force-exit so no headless zombie lingers in Task Manager.
            if let tauri::WindowEvent::Destroyed = event {
                if window.label() == "main" {
                    std::process::exit(0);
                }
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let label = window.label();
                if label == "main" {
                    // Stop both servers so no headless llama.cpp processes are left behind
                    let _ = stop_server();
                    let _ = cmd_stop_vision_server();
                    let _ = cmd_stop_listening_server();
                    // Kill any spawned child processes (configurator, BSR, etc.)
                    if let Some(mutex) = window.app_handle().try_state::<Mutex<AppState>>() {
                        terminate_spawned_children(&*mutex);
                    }
                    // Close all secondary windows before exiting so they don't linger
                    let app = window.app_handle();
                    for lbl in &["advanced-settings", "debug-terminal"] {
                        if let Some(w) = app.get_webview_window(lbl) {
                            let _ = w.close();
                        }
                    }
                    // Hard-exit: kills every thread including the proxy
                    app.exit(0);
                } else if label == "advanced-settings" || label == "debug-terminal" {
                    // Secondary windows hide on close; they don't kill the app
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}



