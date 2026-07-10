//! ServerConfig parsing (the "slot config" contract with the Tauri frontend)
//! and no-server lifecycle behaviour of the server manager.

use std::path::PathBuf;

use server::{health_check, model_name_from_path, stop_server, ServerConfig, ServerStatus};

#[test]
fn minimal_config_json_parses_with_defaults() {
    // The frontend may send only the three required fields; every override
    // is #[serde(default)] and must come back as None.
    let json = r#"{"model_path":"C:/models/tiny.gguf","context_length":4096,"threads":8}"#;
    let cfg: ServerConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.model_path, PathBuf::from("C:/models/tiny.gguf"));
    assert_eq!(cfg.context_length, 4096);
    assert_eq!(cfg.threads, 8);
    assert!(cfg.mmproj_path.is_none());
    assert!(cfg.temperature_override.is_none());
    assert!(cfg.n_predict_override.is_none());
    assert!(cfg.ctx_cap_override.is_none());
}

#[test]
fn full_config_roundtrips() {
    let json = r#"{
        "model_path": "D:/gguf/vision.gguf",
        "context_length": 8192,
        "threads": 12,
        "mmproj_path": "D:/gguf/mmproj.gguf",
        "temperature_override": 0.4,
        "n_predict_override": -1,
        "ctx_cap_override": 4096
    }"#;
    let cfg: ServerConfig = serde_json::from_str(json).unwrap();
    let back: ServerConfig = serde_json::from_str(&serde_json::to_string(&cfg).unwrap()).unwrap();
    assert_eq!(back.model_path, PathBuf::from("D:/gguf/vision.gguf"));
    assert_eq!(back.mmproj_path, Some(PathBuf::from("D:/gguf/mmproj.gguf")));
    assert_eq!(back.temperature_override, Some(0.4));
    assert_eq!(back.n_predict_override, Some(-1));
    assert_eq!(back.ctx_cap_override, Some(4096));
}

#[test]
fn missing_required_field_is_rejected() {
    let json = r#"{"context_length":4096,"threads":8}"#;
    assert!(serde_json::from_str::<ServerConfig>(json).is_err());
}

#[test]
fn model_name_from_path_strips_dir_and_extension() {
    assert_eq!(
        model_name_from_path(&PathBuf::from("C:/models/phi-4.Q4_K_M.gguf")),
        "phi-4.Q4_K_M"
    );
    assert_eq!(model_name_from_path(&PathBuf::from("bare")), "bare");
    // Unresolvable paths fall back to the "local" slug.
    assert_eq!(model_name_from_path(&PathBuf::from("")), "local");
}

#[test]
fn lifecycle_with_no_server_is_safe() {
    // Stopping when nothing is running must be a no-op Ok, and health must
    // report Stopped without touching the network.
    stop_server().unwrap();
    assert_eq!(health_check(), ServerStatus::Stopped);
    // Idempotent: a second stop is still fine.
    stop_server().unwrap();
}
