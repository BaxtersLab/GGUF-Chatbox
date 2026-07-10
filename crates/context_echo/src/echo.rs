use crate::types::{ChatMessage, EchoConfig, EchoSource, MilestoneHit};

/// Build an EchoSource from the system prompt and the earliest chat messages.
/// The first four exchanges are captured as key facts for later re-injection.
pub fn capture_echo_source(system_prompt: &str, early_messages: &[ChatMessage]) -> EchoSource {
    let key_facts: Vec<String> = early_messages
        .iter()
        .take(4)
        .map(|m| format!("{}: {}", m.role, m.content))
        .collect();
    EchoSource {
        system_prompt: system_prompt.to_string(),
        key_facts,
    }
}

/// Check whether the current token usage has crossed a new milestone.
///
/// `fired` tracks which thresholds have already been triggered — each threshold
/// fires at most once per session regardless of how many times this is called.
pub fn check_milestone(
    current_tokens: usize,
    ctx_size: usize,
    config: &EchoConfig,
    fired: &mut Vec<f32>,
) -> Option<MilestoneHit> {
    if ctx_size == 0 {
        return None;
    }
    let ratio = current_tokens as f32 / ctx_size as f32;
    for &threshold in &config.milestones {
        if ratio >= threshold && !fired.contains(&threshold) {
            fired.push(threshold);
            let echo_text = format!(
                "[Context at {:.0}% of window]",
                threshold * 100.0
            );
            return Some(MilestoneHit { threshold, echo_text });
        }
    }
    None
}

/// Prepend an echo reminder to `prompt` based on the captured source material.
/// The reminder is scaled to fit within the echo budget (auto_scale_echo).
pub fn inject_echo(prompt: &str, echo_source: &EchoSource, milestone: &MilestoneHit) -> String {
    // Build a compact reminder from the echo source.
    let reminder = if echo_source.key_facts.is_empty() {
        format!(
            "[System reminder — {}% context used: {}]",
            (milestone.threshold * 100.0) as u32,
            echo_source.system_prompt
        )
    } else {
        format!(
            "[System reminder — {}% context used: {} | {}]",
            (milestone.threshold * 100.0) as u32,
            echo_source.system_prompt,
            echo_source.key_facts.join(" | ")
        )
    };
    format!("{}\n\n{}", reminder, prompt)
}

/// Produce an echo string scaled to at most `max_echo_pct` of `ctx_size` tokens.
/// Token estimate: chars / 2.
pub fn auto_scale_echo(echo_source: &EchoSource, ctx_size: usize) -> String {
    // Budget: 1% of ctx_size tokens → 1% * 2 chars per token.
    let max_chars = ((ctx_size as f32 * 0.01) as usize * 2).max(20);

    let full = if echo_source.key_facts.is_empty() {
        echo_source.system_prompt.clone()
    } else {
        format!(
            "{} | {}",
            echo_source.system_prompt,
            echo_source.key_facts.join(" | ")
        )
    };

    if full.len() <= max_chars {
        full
    } else {
        // Truncate at a word boundary where possible — but never inside a
        // multi-byte UTF-8 character (slicing at a raw byte index panics).
        let mut cut = max_chars;
        while !full.is_char_boundary(cut) {
            cut -= 1;
        }
        let truncated = &full[..cut];
        match truncated.rfind(' ') {
            Some(pos) if pos > max_chars / 2 => truncated[..pos].to_string(),
            _ => truncated.to_string(),
        }
    }
}
