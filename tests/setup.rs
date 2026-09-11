use relic2077::{automation, setup, vault::Vault};
use serde_json::json;
use std::{fs, path::Path, time::Duration};
use tempfile::tempdir;
#[test]
fn setup_preserves_other_plugins_and_distinguishes_installation_from_events() {
    let temp = tempdir().unwrap();
    let v = Vault::init(&temp.path().join("vault")).unwrap();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let home = temp.path().join("dsh");
    fs::create_dir(&home).unwrap();
    fs::write(
        home.join("cordis.patch.yml"),
        "- name: other-plugin\n  config:\n    keep: true\n",
    )
    .unwrap();
    let exe = Path::new(env!("CARGO_BIN_EXE_relic"));
    setup::install(&v, &project, &home, exe, "codex").unwrap();
    setup::install(&v, &project, &home, exe, "dsh").unwrap();
    setup::install(&v, &project, &home, exe, "dsh").unwrap();
    let items: Vec<serde_json::Value> =
        serde_yaml::from_str(&fs::read_to_string(home.join("cordis.patch.yml")).unwrap()).unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["config"]["keep"], true);
    let state = setup::status(&v, &[]).unwrap();
    assert_eq!(state["codex"]["state"], "awaiting_event");
    assert_eq!(state["dsh"]["state"], "awaiting_event");
    let event = json!({"host":"dsh","project":fs::canonicalize(&project).unwrap(),"status":"ok"});
    assert_eq!(
        setup::status(&v, &[event]).unwrap()["dsh"]["state"],
        "triggered"
    );
    fs::write(project.join(".codex/hooks.json"), "{}").unwrap();
    assert_eq!(
        setup::status(&v, &[]).unwrap()["codex"]["state"],
        "not_installed"
    );
    assert!(!automation::running(&v).unwrap());
    let _lease = automation::lease(&v).unwrap();
    assert!(automation::running(&v).unwrap());
}
#[test]
fn worker_start_is_idempotent_and_stops_without_fake_hook_events() {
    let temp = tempdir().unwrap();
    let v = Vault::init(temp.path()).unwrap();
    automation::start(&v, Path::new(env!("CARGO_BIN_EXE_relic"))).unwrap();
    automation::start(&v, Path::new(env!("CARGO_BIN_EXE_relic"))).unwrap();
    assert!(automation::running(&v).unwrap());
    automation::stop(&v).unwrap();
    for _ in 0..80 {
        if !automation::running(&v).unwrap() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(!automation::running(&v).unwrap());
    assert!(!v.root.join(".relic/automation/events.json").exists());
}
