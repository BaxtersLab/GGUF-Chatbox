use std::process::Command;
use crate::llama_detect::resolve_llama_path;
use crate::types::{InferenceRequest, ModelInstance, SamplingConfig};

/// Builds the `llama` subprocess `Command` for a one-shot (single completion) request.
///
/// The resulting `Command` is ready to be spawned — it is not yet executed.
pub fn build_oneshot_command(instance: &ModelInstance, request: &InferenceRequest, sampling: &SamplingConfig) -> Command {
    let llama_path = resolve_llama_path();
    let mut cmd = Command::new(&llama_path);

    cmd.arg("--model")
        .arg(&instance.model_path)
        .arg("--ctx-size")
        .arg(instance.context_length.to_string())
        .arg("--n-gpu-layers")
        .arg(instance.n_gpu_layers.to_string())
        .arg("--threads")
        .arg(instance.threads.to_string())
        .arg("--temp")
        .arg(sampling.temperature.to_string())
        .arg("--top-k")
        .arg(sampling.top_k.to_string())
        .arg("--top-p")
        .arg(sampling.top_p.to_string())
        .arg("--repeat-penalty")
        .arg(sampling.repeat_penalty.to_string())
        .arg("-f")
        .arg(&request.prompt_path)
        .arg("--no-display-prompt")
        .arg("-n")
        .arg("2048");

    cmd
}

/// Builds the `llama` subprocess `Command` for a persistent conversation session.
///
/// Unlike `build_oneshot_command`, this does not include `-f` (prompt comes via stdin),
/// `--no-display-prompt`, or `-n` (unlimited output). Stdout and stdin must be piped
/// by the caller after spawning.
pub fn build_chat_command(instance: &ModelInstance, sampling: &SamplingConfig) -> Command {
    let llama_path = resolve_llama_path();
    let mut cmd = Command::new(&llama_path);

    cmd.arg("--model")
        .arg(&instance.model_path)
        .arg("--ctx-size")
        .arg(instance.context_length.to_string())
        .arg("--n-gpu-layers")
        .arg(instance.n_gpu_layers.to_string())
        .arg("--threads")
        .arg(instance.threads.to_string())
        .arg("--temp")
        .arg(sampling.temperature.to_string())
        .arg("--top-k")
        .arg(sampling.top_k.to_string())
        .arg("--top-p")
        .arg(sampling.top_p.to_string())
        .arg("--repeat-penalty")
        .arg(sampling.repeat_penalty.to_string())
        .arg("--conversation");

    cmd
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ModelConfig, ModelInstance, SamplingConfig};
    use std::path::PathBuf;

    fn make_instance() -> ModelInstance {
        let config = ModelConfig {
            model_path: PathBuf::from("/models/test.gguf"),
            context_length: 2048,
            gpu_setting: "GPU".to_string(),
        };
        ModelInstance::new(config, 16)
    }

    #[test]
    fn test_command_contains_model_path() {
        let inst = make_instance();
        let req = crate::types::InferenceRequest {
            chunk_id: 0,
            prompt_path: PathBuf::from("/tmp/prompt.txt"),
            output_path: PathBuf::from("/tmp/output.txt"),
            log_path: PathBuf::from("/tmp/run.log"),
            sampling: SamplingConfig::default(),
        };
        let sampling = SamplingConfig::default();
        let cmd = build_oneshot_command(&inst, &req, &sampling);
        let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"/models/test.gguf".to_string()));
        assert!(args.contains(&"--n-gpu-layers".to_string()));
        assert!(args.contains(&"16".to_string()));
        assert!(args.contains(&"--threads".to_string()));
        assert!(args.contains(&"4".to_string())); // default threads from ModelInstance::new
    }
}

