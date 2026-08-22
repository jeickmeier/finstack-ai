//! breaking-change negative fixture: frozen-family negatives fail closed on incompatible mutations.

use std::fs;
use std::process::Command;

use finstack_ai::AgentSpec;
use finstack_ai_kernel::{RecordBody, RunEvent, RunEventKind};
use finstack_ai_protocol::{ProcessPreAuth, RemotePreAuth, from_diagnostic_json};
use finstack_ai_test::compatibility_fixture;
use serde_json::Value;

fn repo_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn frozen_family_unknown_fields_fail_closed() {
    let spec = fs::read(compatibility_fixture(
        "agent-spec/v1/agent-spec/invalid--unknown-field.json",
    ))
    .expect("agent-spec");
    assert!(
        AgentSpec::from_json(&spec).is_err(),
        "AgentSpec unknown field must fail"
    );

    let journal = fs::read_to_string(compatibility_fixture(
        "journal/breaking/invalid--unknown-state-field.json",
    ))
    .expect("journal");
    assert!(
        from_diagnostic_json::<RecordBody>(&journal).is_err(),
        "journal unknown state-bearing field must fail"
    );

    let event = fs::read_to_string(compatibility_fixture(
        "runtime-events/v1/event/invalid--unknown-field.json",
    ))
    .expect("event");
    assert!(
        serde_json::from_str::<RunEvent>(&event).is_err(),
        "runtime-event unknown field must fail"
    );

    let remote = fs::read_to_string(compatibility_fixture(
        "remote/v1/handshake/invalid--unknown-field.json",
    ))
    .expect("remote");
    assert!(
        serde_json::from_str::<RemotePreAuth>(&remote).is_err(),
        "remote unknown field must fail"
    );

    let process = fs::read_to_string(compatibility_fixture(
        "process/v1/handshake/invalid--unknown-field.json",
    ))
    .expect("process");
    assert!(
        serde_json::from_str::<ProcessPreAuth>(&process).is_err(),
        "process unknown field must fail"
    );

    let rust_unknown = fs::read_to_string(compatibility_fixture(
        "public-rust-api/v1/run-event/invalid--unknown-field.json",
    ))
    .expect("public rust");
    assert!(
        rust_unknown.contains("unexpected"),
        "public-rust-api unknown-field fixture must remain"
    );
}

#[test]
fn durable_event_order_swap_is_not_canonical() {
    let text = fs::read_to_string(compatibility_fixture(
        "runtime-events/v1/order/invalid--durable-order-swap.json",
    ))
    .expect("order fixture");
    let value: Value = serde_json::from_str(&text).expect("json");
    let canonical = value["canonical"]
        .as_array()
        .expect("canonical")
        .iter()
        .map(|item| item.as_str().expect("kind"))
        .collect::<Vec<_>>();
    let swapped = value["swapped"]
        .as_array()
        .expect("swapped")
        .iter()
        .map(|item| item.as_str().expect("kind"))
        .collect::<Vec<_>>();
    assert_eq!(canonical, ["effect_requested", "effect_completed"]);
    assert_eq!(swapped, ["effect_completed", "effect_requested"]);
    assert_ne!(canonical, swapped);
    assert_ne!(RunEventKind::EffectRequested, RunEventKind::EffectCompleted);
}

#[test]
fn wit_v1_world_rename_is_documented_and_blocked() {
    let renamed = fs::read_to_string(compatibility_fixture(
        "wit/v1.0.0/world/invalid--renamed-world.wit",
    ))
    .expect("renamed world");
    assert!(renamed.contains("world agent-plugin"));
    assert!(!renamed.contains("world toolset-plugin"));
    let unknown_field = fs::read_to_string(compatibility_fixture(
        "wit/v1.0.0/manifest/invalid--unknown-field.json",
    ))
    .expect("wit unknown field");
    assert!(unknown_field.contains("ambient_authority"));
}

// The frozen public-item baselines are gated by `mise run check-public-api`,
// which `ci-rust` runs before `test-rust`. It is deliberately not duplicated as
// a test here: the task fails fast, names the regeneration command, and pins the
// rustdoc toolchain, none of which a `uv` shell-out from nextest can do.

#[test]
fn migration_converters_fail_closed_on_unknown_fields() {
    let status = Command::new("uv")
        .args([
            "run",
            "--no-project",
            "python",
            "scripts/migrate/migrate.py",
            "journal",
            "fixtures/compatibility/journal/breaking/invalid--unknown-state-field.json",
            "--dry-run",
        ])
        .current_dir(repo_root())
        .status()
        .expect("migrate journal");
    assert!(!status.success(), "journal converter must fail closed");

    let spec_status = Command::new("uv")
        .args([
            "run",
            "--no-project",
            "python",
            "scripts/migrate/migrate.py",
            "agentspec",
            "fixtures/compatibility/agent-spec/v1/agent-spec/invalid--unknown-field.json",
            "--dry-run",
        ])
        .current_dir(repo_root())
        .status()
        .expect("migrate agentspec");
    assert!(
        !spec_status.success(),
        "AgentSpec converter must fail closed"
    );

    let ok = Command::new("uv")
        .args([
            "run",
            "--no-project",
            "python",
            "scripts/migrate/migrate.py",
            "agentspec",
            "fixtures/compatibility/agent-spec/v1/agent-spec/valid--minimal.json",
            "--dry-run",
        ])
        .current_dir(repo_root())
        .status()
        .expect("migrate valid agentspec");
    assert!(ok.success(), "valid AgentSpec must convert");
}
