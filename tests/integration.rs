use relic2077::integration::{
    AgentKind, integrate, integrate_codex, integrate_dsh, integrate_global,
};
use relic2077::vault::Vault;
use std::fs;
use tempfile::tempdir;
use toml_edit::DocumentMut;

#[test]
fn configures_codex_and_adds_agents_guidance_idempotently() {
    let directory = tempdir().unwrap();
    let project = directory.path().join("project");
    let vault_path = directory.path().join("vault");
    fs::create_dir(&project).unwrap();
    Vault::init(&vault_path).unwrap();
    fs::write(project.join("AGENTS.md"), "# Existing instructions\n").unwrap();
    fs::create_dir(project.join(".codex")).unwrap();
    fs::write(project.join(".codex/config.toml"), "model = \"gpt-test\"\n").unwrap();
    let executable = std::env::current_exe().unwrap();

    integrate_codex(&project, &vault_path, &executable, true, false).unwrap();
    integrate_codex(&project, &vault_path, &executable, true, false).unwrap();

    let config = fs::read_to_string(project.join(".codex/config.toml")).unwrap();
    assert!(config.contains("model = \"gpt-test\""));
    let config = config.parse::<DocumentMut>().unwrap();
    assert_eq!(
        config["mcp_servers"]["relic"]["required"].as_bool(),
        Some(true)
    );
    assert_eq!(
        config["mcp_servers"]["relic"]["args"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|value| value.as_str())
            .collect::<Vec<_>>(),
        vec![
            "mcp",
            "--vault",
            vault_path.canonicalize().unwrap().to_str().unwrap(),
            "--source-agent",
            "codex"
        ]
    );
    let agents = fs::read_to_string(project.join("AGENTS.md")).unwrap();
    assert!(agents.starts_with("# Existing instructions"));
    assert_eq!(agents.matches("<!-- relic2077:start -->").count(), 1);
    assert_eq!(agents.matches("## Relic long-term memory").count(), 1);
}

#[test]
fn dry_run_does_not_write_files() {
    let directory = tempdir().unwrap();
    let project = directory.path().join("project");
    let vault_path = directory.path().join("vault");
    fs::create_dir(&project).unwrap();
    Vault::init(&vault_path).unwrap();

    let outcome = integrate_codex(
        &project,
        &vault_path,
        &std::env::current_exe().unwrap(),
        true,
        true,
    )
    .unwrap();

    assert!(!outcome.config_path.exists());
    assert!(!outcome.agents_path.unwrap().exists());
}

#[test]
fn configures_every_supported_agent_with_native_project_files() {
    let directory = tempdir().unwrap();
    let vault_path = directory.path().join("vault");
    Vault::init(&vault_path).unwrap();
    let executable = std::env::current_exe().unwrap();
    let cases = [
        (AgentKind::Claude, ".mcp.json", "CLAUDE.md", "claude-code"),
        (AgentKind::Cursor, ".cursor/mcp.json", "AGENTS.md", "cursor"),
        (
            AgentKind::Gemini,
            ".gemini/settings.json",
            "GEMINI.md",
            "gemini-cli",
        ),
        (
            AgentKind::Vscode,
            ".vscode/mcp.json",
            "AGENTS.md",
            "github-copilot",
        ),
    ];

    for (agent, config_path, guidance_path, source_agent) in cases {
        let project = directory.path().join(source_agent);
        fs::create_dir(&project).unwrap();
        integrate(agent, &project, &vault_path, &executable, true, false).unwrap();
        integrate(agent, &project, &vault_path, &executable, true, false).unwrap();

        let config = fs::read_to_string(project.join(config_path)).unwrap();
        assert!(
            config.contains(source_agent),
            "missing source in {config_path}"
        );
        assert!(config.contains("relic"), "missing server in {config_path}");
        serde_json::from_str::<serde_json::Value>(&config).unwrap();
        let guidance = fs::read_to_string(project.join(guidance_path)).unwrap();
        assert_eq!(guidance.matches("<!-- relic2077:start -->").count(), 1);
    }
}

#[test]
fn configures_the_selected_dsh_profile_and_replaces_empty_patch() {
    let directory = tempdir().unwrap();
    let vault_path = directory.path().join("vault");
    let dsh_home = directory.path().join(".dsh");
    let profile = dsh_home.join("profiles/web");
    fs::create_dir_all(&profile).unwrap();
    fs::write(
        profile.join("cordis.patch.yml"),
        "# Existing DSH profile patch\n[]\n",
    )
    .unwrap();
    Vault::init(&vault_path).unwrap();
    let executable = std::env::current_exe().unwrap();

    integrate_dsh(
        &vault_path,
        &executable,
        Some("web"),
        &dsh_home,
        true,
        false,
    )
    .unwrap();
    integrate_dsh(
        &vault_path,
        &executable,
        Some("web"),
        &dsh_home,
        true,
        false,
    )
    .unwrap();

    let patch = fs::read_to_string(profile.join("cordis.patch.yml")).unwrap();
    let parsed = serde_yaml::from_str::<serde_yaml::Value>(&patch).unwrap();
    assert!(parsed.is_sequence());
    assert!(patch.contains("mcp-relic"));
    assert_eq!(patch.matches("# relic2077:start").count(), 1);
    assert!(profile.join("AGENTS.md").exists());
}

#[test]
fn preserves_existing_json_agent_settings() {
    let directory = tempdir().unwrap();
    let project = directory.path().join("project");
    let vault_path = directory.path().join("vault");
    fs::create_dir_all(project.join(".gemini")).unwrap();
    Vault::init(&vault_path).unwrap();
    fs::write(
        project.join(".gemini/settings.json"),
        r#"{"theme":"dark","mcpServers":{"other":{"command":"other"}}}"#,
    )
    .unwrap();

    integrate(
        AgentKind::Gemini,
        &project,
        &vault_path,
        &std::env::current_exe().unwrap(),
        false,
        false,
    )
    .unwrap();

    let config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(project.join(".gemini/settings.json")).unwrap())
            .unwrap();
    assert_eq!(config["theme"], "dark");
    assert_eq!(config["mcpServers"]["other"]["command"], "other");
    assert_eq!(config["mcpServers"]["relic"]["args"][4], "gemini-cli");
}

#[test]
fn omitting_project_uses_each_agents_global_configuration() {
    let directory = tempdir().unwrap();
    let home = directory.path().join("home");
    let vault_path = directory.path().join("vault");
    fs::create_dir(&home).unwrap();
    Vault::init(&vault_path).unwrap();
    let executable = std::env::current_exe().unwrap();
    let cases = [
        (
            AgentKind::Codex,
            ".codex/config.toml",
            Some(".codex/AGENTS.md"),
        ),
        (AgentKind::Claude, ".claude.json", Some(".claude/CLAUDE.md")),
        (AgentKind::Cursor, ".cursor/mcp.json", None),
        (
            AgentKind::Gemini,
            ".gemini/settings.json",
            Some(".gemini/GEMINI.md"),
        ),
        (
            AgentKind::Vscode,
            ".copilot/mcp-config.json",
            Some(".copilot/copilot-instructions.md"),
        ),
    ];

    for (agent, config_path, guidance_path) in cases {
        let outcome =
            integrate_global(agent, &home, &vault_path, &executable, true, false).unwrap();
        assert_eq!(outcome.config_path, home.join(config_path));
        assert!(outcome.config_path.exists());
        assert_eq!(
            outcome.agents_path,
            guidance_path.map(|path| home.join(path))
        );
        if let Some(path) = guidance_path {
            assert!(home.join(path).exists());
        }
    }

    let copilot: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.join(".copilot/mcp-config.json")).unwrap())
            .unwrap();
    assert_eq!(copilot["mcpServers"]["relic"]["type"], "local");
}

#[test]
fn dsh_without_project_updates_global_agents_file() {
    let directory = tempdir().unwrap();
    let vault_path = directory.path().join("vault");
    let dsh_home = directory.path().join(".dsh");
    fs::create_dir_all(&dsh_home).unwrap();
    fs::write(dsh_home.join("cordis.patch.yml"), "[]\n").unwrap();
    Vault::init(&vault_path).unwrap();
    let dsh_home = dsh_home.canonicalize().unwrap();

    let outcome = integrate_dsh(
        &vault_path,
        &std::env::current_exe().unwrap(),
        None,
        &dsh_home,
        true,
        false,
    )
    .unwrap();

    assert_eq!(outcome.agents_path, Some(dsh_home.join("AGENTS.md")));
    assert_eq!(outcome.config_path, dsh_home.join("cordis.patch.yml"));
    assert!(dsh_home.join("AGENTS.md").exists());
}

#[test]
fn dsh_profile_scope_updates_profile_agents_file() {
    let directory = tempdir().unwrap();
    let vault_path = directory.path().join("vault");
    let dsh_home = directory.path().join(".dsh");
    let profile = dsh_home.join("profiles/tui");
    fs::create_dir_all(&profile).unwrap();
    fs::write(profile.join("cordis.patch.yml"), "[]\n").unwrap();
    Vault::init(&vault_path).unwrap();
    let dsh_home = dsh_home.canonicalize().unwrap();
    let profile = profile.canonicalize().unwrap();

    let outcome = integrate_dsh(
        &vault_path,
        &std::env::current_exe().unwrap(),
        Some("tui"),
        &dsh_home,
        true,
        false,
    )
    .unwrap();

    assert_eq!(outcome.config_path, profile.join("cordis.patch.yml"));
    assert_eq!(outcome.agents_path, Some(profile.join("AGENTS.md")));
    assert!(profile.join("AGENTS.md").exists());
    assert!(!dsh_home.join("AGENTS.md").exists());
}
