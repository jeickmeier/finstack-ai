use super::*;

fn entry(label: &str, text: &str) -> PolicyEntry {
    PolicyEntry {
        label: label.to_owned(),
        text: text.to_owned(),
    }
}

#[test]
fn valid_config_passes_validation() {
    let config = PolicyInstructionsConfig {
        entries: vec![
            entry("compliance-footer", "All outputs are for tenant-a internal use only."),
            entry("as-of", "Treat 2026-08-20 as the current date."),
        ],
    };
    config.validate().expect("valid");
}

#[test]
fn empty_entries_are_rejected() {
    let config = PolicyInstructionsConfig { entries: vec![] };
    let error = config.validate().expect_err("empty");
    assert_eq!(
        error.to_string(),
        "instructions_configuration_invalid: entries_empty"
    );
}

#[test]
fn more_than_sixteen_entries_are_rejected() {
    let config = PolicyInstructionsConfig {
        entries: (0..17).map(|i| entry(&format!("rule-{i}"), "text")).collect(),
    };
    let error = config.validate().expect_err("too many");
    assert_eq!(
        error.to_string(),
        "instructions_configuration_invalid: entries_exceed_maximum"
    );
}

#[test]
fn blank_label_or_text_is_rejected() {
    let blank_label = PolicyInstructionsConfig {
        entries: vec![entry("", "text")],
    };
    assert_eq!(
        blank_label.validate().expect_err("label").to_string(),
        "instructions_configuration_invalid: entry_label_empty"
    );
    let blank_text = PolicyInstructionsConfig {
        entries: vec![entry("locale", "   ")],
    };
    assert_eq!(
        blank_text.validate().expect_err("text").to_string(),
        "instructions_configuration_invalid: entry_text_empty"
    );
}
