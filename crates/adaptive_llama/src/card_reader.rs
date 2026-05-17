use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

/// Recommended inference parameters extracted from a model card.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardParams {
    pub temperature: Option<f32>,
    pub top_k: Option<u32>,
    pub top_p: Option<f32>,
    pub repeat_penalty: Option<f32>,
    pub n_predict: Option<i32>,
    pub ctx_size: Option<u32>,
}

impl Default for CardParams {
    fn default() -> Self {
        Self {
            temperature: None,
            top_k: None,
            top_p: None,
            repeat_penalty: None,
            n_predict: None,
            ctx_size: None,
        }
    }
}

/// Metadata about a model, sourced from GGUF header and/or HuggingFace README.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCard {
    /// Human-readable model name (from `general.name` in GGUF).
    pub name: String,
    /// Architecture family (from `general.architecture`, e.g. "llama", "qwen3").
    pub architecture: String,
    /// Model author or organisation, if present in GGUF metadata.
    pub author: String,
    /// Short description, if present.
    pub description: String,
    /// License string, if present.
    pub license: String,
    /// HuggingFace repo ID (e.g. "Qwen/Qwen3-8B") — populated from the
    /// download URL or set manually. Empty if unknown.
    pub hf_repo_id: String,
    /// Recommended inference parameters parsed from the model card or README.
    pub recommended_params: CardParams,
    /// Native context length read directly from GGUF metadata.
    pub native_ctx: u32,
    /// Number of transformer layers from GGUF metadata.
    pub layers: u32,
    /// File size in MB.
    pub size_mb: u64,
    /// Quantisation format (derived from filename, e.g. "Q4_K_M", "BF16").
    pub quant: String,
}

impl ModelCard {
    /// Build a minimal card from GGUF metadata alone (no network fetch required).
    pub fn from_gguf(model_path: &Path) -> Self {
        let name = gguf_read_string_key(model_path, "general.name")
            .or_else(|| model_path.file_stem().and_then(|s| s.to_str()).map(|s| s.to_string()))
            .unwrap_or_default();
        let architecture = gguf_read_string_key(model_path, "general.architecture").unwrap_or_default();
        let author = gguf_read_string_key(model_path, "general.author").unwrap_or_default();
        let description = gguf_read_string_key(model_path, "general.description").unwrap_or_default();
        let license = gguf_read_string_key(model_path, "general.license").unwrap_or_default();
        let native_ctx = crate::llama_detect::gguf_context_length(model_path).unwrap_or(4096);
        let layers = crate::llama_detect::gguf_block_count(model_path).unwrap_or(0);
        let size_mb = std::fs::metadata(model_path)
            .map(|m| m.len() / (1024 * 1024))
            .unwrap_or(0);
        let quant = quant_from_filename(model_path);

        Self {
            name,
            architecture,
            author,
            description,
            license,
            hf_repo_id: String::new(),
            recommended_params: CardParams::default(),
            native_ctx,
            layers,
            size_mb,
            quant,
        }
    }
}

/// Return the path where a model's card JSON is cached.
pub fn card_cache_path(model_path: &Path) -> PathBuf {
    let stem = model_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    let home = {
        #[cfg(target_os = "windows")]
        { std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\Users\\default".to_string()) }
        #[cfg(not(target_os = "windows"))]
        { std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()) }
    };
    PathBuf::from(home)
        .join(".gguf-chatbox")
        .join("cards")
        .join(format!("{}.json", stem))
}

/// Load a card from the JSON cache. Returns `None` if no cache file exists.
pub fn load_cached_card(model_path: &Path) -> Option<ModelCard> {
    let path = card_cache_path(model_path);
    let data = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&data).ok()
}

/// Save a card to the JSON cache, creating parent directories as needed.
pub fn save_card(model_path: &Path, card: &ModelCard) -> Result<(), String> {
    let path = card_cache_path(model_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(card).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())
}

/// Load from cache if present, otherwise build from GGUF metadata and cache it.
pub fn load_or_build_card(model_path: &Path) -> ModelCard {
    if let Some(card) = load_cached_card(model_path) {
        return card;
    }
    let card = ModelCard::from_gguf(model_path);
    let _ = save_card(model_path, &card);
    card
}

/// Merge HuggingFace README markdown into an existing card.
/// Attempts to extract a description and recommended params from common README patterns.
pub fn merge_hf_readme(card: &mut ModelCard, readme: &str, hf_repo_id: &str) {
    card.hf_repo_id = hf_repo_id.to_string();

    // Pull first non-empty paragraph as description if we don't have one.
    if card.description.is_empty() {
        if let Some(desc) = extract_readme_description(readme) {
            card.description = desc;
        }
    }

    // Extract temperature recommendation from README.
    if card.recommended_params.temperature.is_none() {
        card.recommended_params.temperature = extract_readme_temperature(readme);
    }
}

// ── GGUF string key reader ────────────────────────────────────────────────

/// Read a full-key string value from GGUF metadata.
/// `key` is matched exactly (e.g. "general.name", "general.architecture").
pub fn gguf_read_string_key(model_path: &Path, key: &str) -> Option<String> {
    let mut f = std::fs::File::open(model_path).ok()?;

    let mut magic = [0u8; 4];
    f.read_exact(&mut magic).ok()?;
    if &magic != b"GGUF" {
        return None;
    }

    let version = read_u32_le(&mut f)?;
    if version < 2 || version > 3 {
        return None;
    }

    let _tensor_count = read_u64_le(&mut f)?;
    let kv_count = read_u64_le(&mut f)?;

    for _ in 0..kv_count {
        let k = read_gguf_string(&mut f)?;
        let vtype = read_u32_le(&mut f)?;

        if k == key && vtype == 8 {
            return read_gguf_string(&mut f);
        }

        if !skip_gguf_value(&mut f, vtype) {
            return None;
        }
    }
    None
}

// ── README parsers ────────────────────────────────────────────────────────

fn extract_readme_description(readme: &str) -> Option<String> {
    // Skip YAML front matter (lines between --- delimiters).
    let body = if readme.starts_with("---") {
        readme.splitn(3, "---").nth(2).unwrap_or(readme)
    } else {
        readme
    };

    // Find first non-empty, non-heading paragraph.
    let mut in_para = false;
    let mut para = String::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if in_para && !para.is_empty() {
                break;
            }
            continue;
        }
        if trimmed.starts_with('#') || trimmed.starts_with('|') || trimmed.starts_with("```") {
            if in_para { break; }
            continue;
        }
        in_para = true;
        if !para.is_empty() { para.push(' '); }
        para.push_str(trimmed);
    }
    if para.len() > 10 { Some(para) } else { None }
}

fn extract_readme_temperature(readme: &str) -> Option<f32> {
    // Look for patterns like `temperature: 0.6` or `"temperature": 0.6`.
    for line in readme.lines() {
        let lower = line.to_lowercase();
        if lower.contains("temperature") {
            // Extract first float-looking token after ':' or '='
            if let Some(rest) = lower.split_once(':').map(|(_, r)| r)
                .or_else(|| lower.split_once('=').map(|(_, r)| r))
            {
                let tok = rest.split_whitespace().next().unwrap_or("").trim_matches(|c: char| !c.is_ascii_digit() && c != '.');
                if let Ok(v) = tok.parse::<f32>() {
                    if v > 0.0 && v <= 2.0 {
                        return Some(v);
                    }
                }
            }
        }
    }
    None
}

/// Derive a quantisation tag from the model filename.
fn quant_from_filename(model_path: &Path) -> String {
    let stem = model_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_uppercase();

    for tag in &[
        "Q2_K", "Q3_K_S", "Q3_K_M", "Q3_K_L",
        "Q4_0", "Q4_1", "Q4_K_S", "Q4_K_M", "Q4_K_L",
        "Q5_0", "Q5_1", "Q5_K_S", "Q5_K_M",
        "Q6_K", "Q8_0", "IQ2", "IQ3", "IQ4",
        "BF16", "F16", "F32",
    ] {
        if stem.contains(tag) {
            return tag.to_string();
        }
    }
    String::new()
}

// ── GGUF binary primitives (self-contained, no dep on llama_detect) ──────

fn read_u8(f: &mut std::fs::File) -> Option<u8> {
    let mut buf = [0u8; 1];
    f.read_exact(&mut buf).ok()?;
    Some(buf[0])
}
fn read_u16_le(f: &mut std::fs::File) -> Option<u16> {
    let mut buf = [0u8; 2];
    f.read_exact(&mut buf).ok()?;
    Some(u16::from_le_bytes(buf))
}
fn read_u32_le(f: &mut std::fs::File) -> Option<u32> {
    let mut buf = [0u8; 4];
    f.read_exact(&mut buf).ok()?;
    Some(u32::from_le_bytes(buf))
}
fn read_i32_le(f: &mut std::fs::File) -> Option<i32> {
    let mut buf = [0u8; 4];
    f.read_exact(&mut buf).ok()?;
    Some(i32::from_le_bytes(buf))
}
fn read_u64_le(f: &mut std::fs::File) -> Option<u64> {
    let mut buf = [0u8; 8];
    f.read_exact(&mut buf).ok()?;
    Some(u64::from_le_bytes(buf))
}
fn read_f32_le(f: &mut std::fs::File) -> Option<f32> {
    let mut buf = [0u8; 4];
    f.read_exact(&mut buf).ok()?;
    Some(f32::from_le_bytes(buf))
}
fn read_f64_le(f: &mut std::fs::File) -> Option<f64> {
    let mut buf = [0u8; 8];
    f.read_exact(&mut buf).ok()?;
    Some(f64::from_le_bytes(buf))
}
fn read_gguf_string(f: &mut std::fs::File) -> Option<String> {
    let len = read_u64_le(f)? as usize;
    if len > 1_000_000 { return None; }
    let mut buf = vec![0u8; len];
    f.read_exact(&mut buf).ok()?;
    String::from_utf8(buf).ok()
}
fn skip_gguf_value(f: &mut std::fs::File, vtype: u32) -> bool {
    match vtype {
        0 => read_u8(f).is_some(),
        1 => { f.seek(SeekFrom::Current(1)).ok(); true }
        2 => read_u16_le(f).map(|_| ()).is_some(),
        3 => { f.seek(SeekFrom::Current(2)).ok(); true }
        4 => read_u32_le(f).map(|_| ()).is_some(),
        5 => read_i32_le(f).map(|_| ()).is_some(),
        6 => read_f32_le(f).map(|_| ()).is_some(),
        7 => { read_u32_le(f).map(|_| ()).is_some() }
        8 => read_gguf_string(f).map(|_| ()).is_some(),
        9 => {
            let elem_type = match read_u32_le(f) { Some(t) => t, None => return false };
            let count = match read_u64_le(f) { Some(c) => c, None => return false };
            for _ in 0..count {
                if !skip_gguf_value(f, elem_type) { return false; }
            }
            true
        }
        10 => read_u64_le(f).map(|_| ()).is_some(),
        11 => { f.seek(SeekFrom::Current(8)).ok(); true }
        12 => read_f64_le(f).map(|_| ()).is_some(),
        _ => false,
    }
}
