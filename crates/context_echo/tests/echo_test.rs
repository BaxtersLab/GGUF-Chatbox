//! context_echo behaviour: source capture, milestone firing, injection format,
//! and echo scaling (including the UTF-8 truncation regression).

use context_echo::{
    auto_scale_echo, capture_echo_source, check_milestone, inject_echo, ChatMessage, EchoConfig,
    EchoSource, MilestoneHit,
};

fn msg(role: &str, content: &str) -> ChatMessage {
    ChatMessage {
        role: role.to_string(),
        content: content.to_string(),
    }
}

#[test]
fn capture_takes_at_most_four_early_messages() {
    let msgs: Vec<ChatMessage> = (0..6).map(|i| msg("user", &format!("m{i}"))).collect();
    let src = capture_echo_source("sys", &msgs);
    assert_eq!(src.system_prompt, "sys");
    assert_eq!(src.key_facts.len(), 4);
    assert_eq!(src.key_facts[0], "user: m0");
    assert_eq!(src.key_facts[3], "user: m3");
}

#[test]
fn milestones_fire_once_each_in_threshold_order() {
    let cfg = EchoConfig::default(); // 0.25 / 0.50 / 0.75
    let mut fired = Vec::new();

    assert!(check_milestone(100, 1000, &cfg, &mut fired).is_none()); // 10%
    let hit = check_milestone(300, 1000, &cfg, &mut fired).unwrap(); // 30%
    assert_eq!(hit.threshold, 0.25);
    assert!(check_milestone(310, 1000, &cfg, &mut fired).is_none()); // 25% already fired

    // Jumping past two thresholds fires them one call at a time, lowest first.
    let hit2 = check_milestone(760, 1000, &cfg, &mut fired).unwrap();
    assert_eq!(hit2.threshold, 0.50);
    let hit3 = check_milestone(760, 1000, &cfg, &mut fired).unwrap();
    assert_eq!(hit3.threshold, 0.75);
    assert!(check_milestone(760, 1000, &cfg, &mut fired).is_none());
}

#[test]
fn zero_context_size_is_safe() {
    let cfg = EchoConfig::default();
    let mut fired = Vec::new();
    assert!(check_milestone(500, 0, &cfg, &mut fired).is_none());
}

#[test]
fn inject_echo_prepends_reminder_with_facts() {
    let src = EchoSource {
        system_prompt: "You are a helpful assistant.".to_string(),
        key_facts: vec!["user: build the kit".to_string()],
    };
    let hit = MilestoneHit {
        threshold: 0.50,
        echo_text: String::new(),
    };
    let out = inject_echo("continue the task", &src, &hit);
    assert!(out.starts_with("[System reminder — 50% context used:"), "got: {out}");
    assert!(out.contains("You are a helpful assistant."));
    assert!(out.contains("user: build the kit"));
    assert!(out.ends_with("continue the task"));
}

#[test]
fn auto_scale_echo_short_source_is_untouched() {
    let src = EchoSource {
        system_prompt: "short".to_string(),
        key_facts: vec![],
    };
    assert_eq!(auto_scale_echo(&src, 100_000), "short");
}

#[test]
fn auto_scale_echo_truncates_long_ascii_at_word_boundary() {
    let src = EchoSource {
        system_prompt: "word ".repeat(100).trim_end().to_string(),
        key_facts: vec![],
    };
    let out = auto_scale_echo(&src, 2000); // budget: 40 chars
    assert!(out.len() <= 40, "len {} > budget", out.len());
    assert!(
        out.split_whitespace().all(|w| w == "word"),
        "truncation split a word: {out:?}"
    );
}

#[test]
fn auto_scale_echo_never_splits_multibyte_chars() {
    // Regression: `&full[..max_chars]` panicked when the byte budget landed
    // inside a multi-byte UTF-8 character. 3-byte € with a 20-byte budget
    // (ctx 1000) puts the cut mid-character.
    let src = EchoSource {
        system_prompt: "€".repeat(50),
        key_facts: vec![],
    };
    let out = auto_scale_echo(&src, 1000);
    assert!(!out.is_empty());
    assert!(out.chars().all(|c| c == '€'), "got: {out:?}");
}
