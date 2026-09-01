use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use toml_edit::{Array, DocumentMut, Item, Table, value};

const GUIDANCE_START: &str = "<!-- relic2077:start -->";
const GUIDANCE_END: &str = "<!-- relic2077:end -->";
const GUIDANCE: &str = r#"<!-- relic2077:start -->
## Relic long-term memory

Relic is the shared long-term memory for this project.

Before substantial work:
- Search Relic for related decisions, conventions, failures, and prior solutions.
- Read relevant entries before designing or modifying the implementation.
- Skip retrieval only for trivial, context-free changes.

After substantial work:
- Store durable decisions, verified fixes, reusable lessons, and hidden constraints.
- Update an existing entry instead of creating a duplicate.
- Supersede obsolete knowledge instead of deleting it.
- Never store secrets, raw logs, temporary status, or facts obvious from the repository.
<!-- relic2077:end -->"#;
const DSH_START: &str = "# relic2077:start";
const DSH_END: &str = "# relic2077:end";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    Codex,
    Claude,
    Cursor,
    Gemini,
    Vscode,
    Dsh,
}

impl AgentKind {
    fn source_agent(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude-code",
            Self::Cursor => "cursor",
            Self::Gemini => "gemini-cli",
            Self::Vscode => "github-copilot",
            Self::Dsh => "dsh",
        }
    }

    fn config_path(self, project: &Path) -> PathBuf {
        match self {
            Self::Codex => project.join(".codex/config.toml"),
            Self::Claude => project.join(".mcp.json"),
            Self::Cursor => project.join(".cursor/mcp.json"),
            Self::Gemini => project.join(".gemini/settings.json"),
            Self::Vscode => project.join(".vscode/mcp.json"),
            Self::Dsh => project.join(".dsh/cordis.patch.yml"),
        }
    }

    fn guidance_path(self, project: &Path) -> PathBuf {
        match self {
            Self::Claude => project.join("CLAUDE.md"),
            Self::Gemini => project.join("GEMINI.md"),
            _ => project.join("AGENTS.md"),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct IntegrationOutcome {
    pub config_path: PathBuf,
    pub agents_path: Option<PathBuf>,
}

struct IntegrationPaths<'a> {
    project: Option<&'a Path>,
    config_path: Option<PathBuf>,
    global_guidance_path: Option<PathBuf>,
    global: bool,
}

pub fn integrate(
    agent: AgentKind,
    project: &Path,
    vault: &Path,
    executable: &Path,
    update_agents: bool,
    dry_run: bool,
) -> Result<IntegrationOutcome> {
    integrate_with_config(
        agent,
        vault,
        executable,
        update_agents,
        dry_run,
        IntegrationPaths {
            project: Some(project),
            config_path: None,
            global_guidance_path: None,
            global: false,
        },
    )
}

pub fn integrate_global(
    agent: AgentKind,
    home: &Path,
    vault: &Path,
    executable: &Path,
    update_agents: bool,
    dry_run: bool,
) -> Result<IntegrationOutcome> {
    let (config_path, guidance_path) = match agent {
        AgentKind::Codex => (
            home.join(".codex/config.toml"),
            Some(home.join(".codex/AGENTS.md")),
        ),
        AgentKind::Claude => (
            home.join(".claude.json"),
            Some(home.join(".claude/CLAUDE.md")),
        ),
        AgentKind::Cursor => (home.join(".cursor/mcp.json"), None),
        AgentKind::Gemini => (
            home.join(".gemini/settings.json"),
            Some(home.join(".gemini/GEMINI.md")),
        ),
        AgentKind::Vscode => (
            home.join(".copilot/mcp-config.json"),
            Some(home.join(".copilot/copilot-instructions.md")),
        ),
        AgentKind::Dsh => bail!("use integrate_dsh for DSH profile configuration"),
    };
    integrate_with_config(
        agent,
        vault,
        executable,
        update_agents,
        dry_run,
        IntegrationPaths {
            project: None,
            config_path: Some(config_path),
            global_guidance_path: guidance_path,
            global: true,
        },
    )
}

pub fn integrate_dsh(
    vault: &Path,
    executable: &Path,
    profile: Option<&str>,
    dsh_home: &Path,
    update_agents: bool,
    dry_run: bool,
) -> Result<IntegrationOutcome> {
    let dsh_home = canonical_directory(dsh_home, "DSH home")?;
    let (config_path, guidance_path) = if let Some(profile) = profile {
        if profile.is_empty()
            || profile == "."
            || profile == ".."
            || profile.contains('/')
            || profile.contains('\\')
        {
            bail!("DSH profile must be a single profile name");
        }
        let profile_directory =
            canonical_directory(&dsh_home.join("profiles").join(profile), "DSH profile")?;
        (
            profile_directory.join("cordis.patch.yml"),
            profile_directory.join("AGENTS.md"),
        )
    } else {
        (
            dsh_home.join("cordis.patch.yml"),
            dsh_home.join("AGENTS.md"),
        )
    };
    integrate_with_config(
        AgentKind::Dsh,
        vault,
        executable,
        update_agents,
        dry_run,
        IntegrationPaths {
            project: None,
            config_path: Some(config_path),
            global_guidance_path: Some(guidance_path),
            global: true,
        },
    )
}

fn integrate_with_config(
    agent: AgentKind,
    vault: &Path,
    executable: &Path,
    update_agents: bool,
    dry_run: bool,
    paths: IntegrationPaths<'_>,
) -> Result<IntegrationOutcome> {
    let project = paths
        .project
        .map(|path| canonical_directory(path, "project"))
        .transpose()?;
    let vault = canonical_directory(vault, "vault")?;
    if !vault.join(".relic/config.yaml").is_file() {
        bail!("{} is not a Relic vault", vault.display());
    }
    let executable = executable
        .canonicalize()
        .with_context(|| format!("could not resolve executable {}", executable.display()))?;
    let config_path = paths.config_path.unwrap_or_else(|| {
        agent.config_path(
            project
                .as_deref()
                .expect("project integration requires a project path"),
        )
    });
    let config_input = read_if_exists(&config_path)?;
    let config_output = match agent {
        AgentKind::Codex => codex_config(&config_input, &executable, &vault)?,
        AgentKind::Claude | AgentKind::Cursor | AgentKind::Gemini => json_config(
            &config_input,
            "mcpServers",
            None,
            &executable,
            &vault,
            agent.source_agent(),
        )?,
        AgentKind::Vscode => json_config(
            &config_input,
            if paths.global {
                "mcpServers"
            } else {
                "servers"
            },
            Some(if paths.global { "local" } else { "stdio" }),
            &executable,
            &vault,
            agent.source_agent(),
        )?,
        AgentKind::Dsh => dsh_config(&config_input, &executable, &vault)?,
    };

    let agents_path = if update_agents {
        project
            .as_deref()
            .map(|project| agent.guidance_path(project))
            .or(paths.global_guidance_path)
    } else {
        None
    };
    let agents_output = agents_path
        .as_ref()
        .map(|path| {
            read_if_exists(path)
                .and_then(|input| upsert_block(&input, GUIDANCE_START, GUIDANCE_END, GUIDANCE))
        })
        .transpose()?;

    if !dry_run {
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&config_path, config_output)?;
        if let (Some(path), Some(content)) = (&agents_path, agents_output) {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(path, content)?;
        }
    }

    Ok(IntegrationOutcome {
        config_path,
        agents_path,
    })
}

pub fn integrate_codex(
    project: &Path,
    vault: &Path,
    executable: &Path,
    update_agents: bool,
    dry_run: bool,
) -> Result<IntegrationOutcome> {
    integrate(
        AgentKind::Codex,
        project,
        vault,
        executable,
        update_agents,
        dry_run,
    )
}

fn codex_config(input: &str, executable: &Path, vault: &Path) -> Result<String> {
    let mut config = input
        .parse::<DocumentMut>()
        .context("invalid existing .codex/config.toml")?;
    if config.get("mcp_servers").is_none() {
        config["mcp_servers"] = Item::Table(Table::new());
    }
    let servers = config["mcp_servers"]
        .as_table_like_mut()
        .context("mcp_servers in .codex/config.toml must be a table")?;
    if !servers.contains_key("relic") {
        servers.insert("relic", Item::Table(Table::new()));
    }
    let relic = servers
        .get_mut("relic")
        .and_then(Item::as_table_like_mut)
        .context("mcp_servers.relic in .codex/config.toml must be a table")?;
    relic.insert("command", value(executable.to_string_lossy().as_ref()));
    let mut args = Array::new();
    for argument in mcp_args(vault, "codex") {
        args.push(argument);
    }
    relic.insert("args", value(args));
    relic.insert("required", value(true));
    relic.insert("default_tools_approval_mode", value("writes"));
    Ok(config.to_string())
}

fn json_config(
    input: &str,
    servers_key: &str,
    server_type: Option<&str>,
    executable: &Path,
    vault: &Path,
    source_agent: &str,
) -> Result<String> {
    let mut document: Value = if input.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(input).context("invalid existing JSON MCP configuration")?
    };
    let root = document
        .as_object_mut()
        .context("MCP configuration root must be a JSON object")?;
    let servers = root
        .entry(servers_key)
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .with_context(|| format!("{servers_key} must be a JSON object"))?;
    let mut relic = Map::new();
    if let Some(server_type) = server_type {
        relic.insert("type".into(), json!(server_type));
    }
    relic.insert(
        "command".into(),
        json!(executable.to_string_lossy().as_ref()),
    );
    relic.insert("args".into(), json!(mcp_args(vault, source_agent)));
    servers.insert("relic".into(), Value::Object(relic));
    Ok(format!("{}\n", serde_json::to_string_pretty(&document)?))
}

fn dsh_config(input: &str, executable: &Path, vault: &Path) -> Result<String> {
    let block = format!(
        "{DSH_START}\n- insert:\n    - id: mcp-relic\n      name: '@deepseek-ai/dsh-mcp-client'\n      config:\n        serverName: relic\n        transport: stdio\n        command: {}\n        args:\n          - mcp\n          - --vault\n          - {}\n          - --source-agent\n          - dsh\n        failOnStartupError: true\n{DSH_END}",
        yaml_scalar(executable.to_string_lossy().as_ref()),
        yaml_scalar(vault.to_string_lossy().as_ref())
    );
    let significant_lines = input
        .lines()
        .filter(|line| {
            let line = line.trim();
            !line.is_empty() && !line.starts_with('#')
        })
        .collect::<Vec<_>>();
    if !input.contains(DSH_START) && significant_lines == ["[]"] {
        let mut output = input
            .lines()
            .filter(|line| line.trim() != "[]")
            .collect::<Vec<_>>()
            .join("\n");
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(&block);
        output.push('\n');
        Ok(output)
    } else {
        upsert_block(input, DSH_START, DSH_END, &block)
    }
}

fn yaml_scalar(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn mcp_args(vault: &Path, source_agent: &str) -> Vec<String> {
    vec![
        "mcp".into(),
        "--vault".into(),
        vault.to_string_lossy().into_owned(),
        "--source-agent".into(),
        source_agent.into(),
    ]
}

fn canonical_directory(path: &Path, label: &str) -> Result<PathBuf> {
    let path = path
        .canonicalize()
        .with_context(|| format!("could not resolve {label} directory {}", path.display()))?;
    if !path.is_dir() {
        bail!("{label} path is not a directory: {}", path.display());
    }
    Ok(path)
}

fn read_if_exists(path: &Path) -> Result<String> {
    if path.exists() {
        fs::read_to_string(path).with_context(|| format!("could not read {}", path.display()))
    } else {
        Ok(String::new())
    }
}

fn upsert_block(input: &str, start_marker: &str, end_marker: &str, block: &str) -> Result<String> {
    match (input.find(start_marker), input.find(end_marker)) {
        (Some(start), Some(end)) if start < end => {
            let suffix = end + end_marker.len();
            Ok(format!("{}{}{}", &input[..start], block, &input[suffix..]))
        }
        (None, None) => {
            let separator = if input.is_empty() || input.ends_with("\n\n") {
                ""
            } else if input.ends_with('\n') {
                "\n"
            } else {
                "\n\n"
            };
            Ok(format!("{input}{separator}{block}\n"))
        }
        _ => bail!("configuration contains an incomplete Relic managed block"),
    }
}
