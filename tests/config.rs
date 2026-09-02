use relic2077::vault::Vault;
use std::fs;
use tempfile::tempdir;

#[test]
fn default_config_is_written_and_validates() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    let config = vault.config().unwrap();
    assert_eq!(config.version, 1);
    assert_eq!(config.vault.name, "Relic Vault");
    assert!(config.sync.remotes.is_empty());
    assert_eq!(config.evolution.fading_threshold, 0.3);
}

#[test]
fn config_rejects_an_unsupported_schema_version() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    let config_path = directory.path().join(".relic").join("config.yaml");
    fs::write(&config_path, "version: 2\n").unwrap();
    let error = vault.config().unwrap_err();
    assert!(format!("{error:#}").contains("unsupported schema version 2"));
}

#[test]
fn config_rejects_duplicate_sync_remotes() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    let config_path = directory.path().join(".relic").join("config.yaml");
    fs::write(
        &config_path,
        "version: 1\nsync:\n  mode: manual\n  remotes:\n    - name: origin\n      url: git@example.com:r.git\n    - name: origin\n      url: git@example.com:r2.git\n",
    )
    .unwrap();
    let error = vault.config().unwrap_err();
    assert!(format!("{error:#}").contains("duplicate sync remote"));
}

#[test]
fn maintain_reindexes_and_auto_reflects_once() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    for i in 0..5 {
        vault
            .create(
                &format!("Entry {i}"),
                &format!("Body {i}."),
                "knowledge",
                vec![],
                0.8,
                "test",
            )
            .unwrap();
    }
    // First maintenance pass meets the threshold and writes a reflection.
    let first = vault.maintain("daily", 5).unwrap();
    assert!(first.reflected);
    assert!(vault.reflection_path("daily").unwrap().is_some());
    // A second pass in the same period is a no-op for reflection.
    let second = vault.maintain("daily", 5).unwrap();
    assert!(!second.reflected);
}

#[test]
fn maintain_skips_reflection_below_threshold() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    vault
        .create("Only", "One.", "knowledge", vec![], 0.8, "test")
        .unwrap();
    let maintenance = vault.maintain("daily", 5).unwrap();
    assert!(!maintenance.reflected);
    assert!(vault.reflection_path("daily").unwrap().is_none());
}
