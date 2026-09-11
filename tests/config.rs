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
    // The shipped defaults are the calibrated ones, not placeholders: a vault
    // written today must reproduce the graph the thresholds were measured for.
    assert_eq!(config.graph.dimensions, 8192);
    assert_eq!(config.graph.semantic_min_similarity, 0.08);
    assert_eq!(config.graph.tag_min_jaccard, 0.34);
    assert_eq!(config.graph.corroborate_min_similarity, 0.15);
    assert_eq!(config.graph.max_edges_per_node, 24);
}

#[test]
fn config_rejects_graph_settings_that_would_be_meaningless() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    let config_path = directory.path().join(".relic").join("config.yaml");
    for (bad, expected) in [
        ("graph:\n  dimensions: 0\n", "graph.dimensions"),
        ("graph:\n  semantic_top_k: 0\n", "graph.semantic_top_k"),
        (
            "graph:\n  semantic_min_similarity: 2.0\n",
            "graph.semantic_min_similarity",
        ),
        ("graph:\n  tag_min_jaccard: -1.0\n", "graph.tag_min_jaccard"),
        (
            "graph:\n  corroborate_min_similarity: 9\n",
            "graph.corroborate_min_similarity",
        ),
        (
            "graph:\n  max_edges_per_node: 0\n",
            "graph.max_edges_per_node",
        ),
    ] {
        fs::write(&config_path, format!("version: 1\n{bad}")).unwrap();
        let error = format!("{:#}", vault.config().unwrap_err());
        assert!(error.contains(expected), "{bad} produced '{error}'");
    }
}

#[test]
fn an_older_config_without_graph_settings_still_loads() {
    // The graph section is new, so a vault written by an earlier release must
    // keep working and simply receive the defaults.
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    let config_path = directory.path().join(".relic").join("config.yaml");
    fs::write(
        &config_path,
        "version: 1\nvault:\n  name: Older\n  default_confidence: 0.7\n  default_decay_rate: 0.05\nsync:\n  mode: manual\n  remotes: []\nevolution:\n  fading_threshold: 0.3\n",
    )
    .unwrap();
    let config = vault.config().unwrap();
    assert_eq!(config.vault.name, "Older");
    assert_eq!(
        config.graph.semantic_min_similarity,
        relic2077::config::GraphConfig::default().semantic_min_similarity
    );
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
fn configured_decay_applies_to_new_memories_and_preserves_existing_history() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    let original = vault
        .create(
            "Original",
            "Before the change",
            "knowledge",
            vec![],
            0.8,
            "test",
        )
        .unwrap();
    let original_raw = fs::read_to_string(&original.path).unwrap();
    fs::write(
        directory.path().join(".relic/config.yaml"),
        "version: 1\nvault:\n  default_decay_rate: 0.2\n",
    )
    .unwrap();
    let created = vault
        .create("New", "After the change", "knowledge", vec![], 0.8, "test")
        .unwrap();
    assert_eq!(created.meta.decay_rate, 0.2);
    assert_eq!(vault.get(&created.meta.id).unwrap().meta.decay_rate, 0.2);
    assert_eq!(vault.get(&original.meta.id).unwrap().meta.decay_rate, 0.05);
    assert_eq!(fs::read_to_string(&original.path).unwrap(), original_raw);
}

#[test]
fn invalid_decay_configuration_is_rejected_before_a_memory_is_written() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    for decay in [".nan", ".inf", "-.inf", "-0.1"] {
        fs::write(
            directory.path().join(".relic/config.yaml"),
            format!("version: 1\nvault:\n  default_decay_rate: {decay}\n"),
        )
        .unwrap();
        let error = vault
            .create(
                "Invalid",
                "Must not persist",
                "knowledge",
                vec![],
                0.8,
                "test",
            )
            .unwrap_err();
        assert!(format!("{error:#}").contains("default_decay_rate"));
        assert!(vault.entries().unwrap().is_empty(), "decay={decay}");
    }
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
