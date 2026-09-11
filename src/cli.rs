use crate::vault::{EntryPatch, Vault};
use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "relic",
    version,
    about = "Never fade away — local-first knowledge for every agent"
)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Configure the device-local distillation backend (never Git-synchronized)
    Distiller {
        #[command(subcommand)]
        command: DistillerCommand,
    },
    /// Import memories Codex distilled for itself (Memories must be enabled)
    ImportCodexMemory(CodexMemoryArgs),
    /// Install project-scoped host integration or control the independent worker
    Setup {
        #[arg(long)]
        vault: PathBuf,
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        dsh_home: Option<PathBuf>,
        #[arg(long)]
        host: String,
    },
    /// Run a synchronous host hook; reads one JSON payload from stdin
    Hook {
        #[arg(long)]
        vault: PathBuf,
        #[arg(long, default_value = "codex")]
        host: String,
    },
    /// Merge Codex hooks into an explicit hooks.json path
    InstallHooks {
        #[arg(long)]
        vault: PathBuf,
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        dry_run: bool,
    },
    /// Process captures and sync in an independent foreground worker
    Daemon {
        #[arg(long)]
        vault: PathBuf,
        #[arg(long)]
        once: bool,
        #[arg(long)]
        retry: bool,
    },
    /// Initialize a new knowledge vault
    Init {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Add a knowledge entry
    Add {
        title: String,
        #[arg(short, long, default_value = "")]
        content: String,
        #[arg(long, default_value = "knowledge")]
        kind: String,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        #[arg(long, default_value_t = 0.7)]
        confidence: f64,
        #[arg(long, default_value = "")]
        source_agent: String,
    },
    /// Persist a structured experience from a JSON file
    Capture { file: PathBuf },
    /// Process and review captured experiences
    Queue {
        #[command(subcommand)]
        command: QueueCommand,
    },
    /// Update selected fields on an existing entry
    Update {
        entry_id: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(short, long)]
        content: Option<String>,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        confidence: Option<f64>,
        #[arg(long, value_delimiter = ',')]
        tags: Option<Vec<String>>,
        #[arg(long, value_delimiter = ',')]
        source_agents: Option<Vec<String>>,
        #[arg(long, value_delimiter = ',')]
        links: Option<Vec<String>>,
    },
    /// Mark an older entry as superseded by a newer entry
    Supersede {
        old_entry_id: String,
        new_entry_id: String,
    },
    /// Print an entry by ID
    Get { entry_id: String },
    /// List entries
    List {
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        status: Option<String>,
        /// Require every comma-separated tag
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        /// Filter by the agent that originally supplied the entry
        #[arg(long)]
        source_agent: Option<String>,
        /// Minimum effective confidence after decay
        #[arg(long)]
        min_confidence: Option<f64>,
        /// Maximum effective confidence after decay
        #[arg(long)]
        max_confidence: Option<f64>,
    },
    /// Full-text search the vault
    Search {
        query: String,
        #[arg(short = 'n', long, default_value_t = 10)]
        limit: usize,
        /// Retrieval layer: keyword (literal), semantic (vector), or hybrid
        /// (both, fused by reciprocal rank)
        #[arg(long, default_value = "keyword")]
        mode: String,
    },
    /// Build and query the knowledge graph: typed relations over memories
    Graph {
        #[command(subcommand)]
        command: GraphCommand,
    },
    /// Rebuild the local search index
    Reindex,
    /// Create a reflection draft
    Reflect {
        #[arg(long, default_value = "weekly")]
        period: String,
        /// Only create the reflection when the trigger threshold is met
        #[arg(long)]
        auto: bool,
        /// Minimum active entries required by --auto
        #[arg(long, default_value_t = 5)]
        min_entries: usize,
    },
    /// Detect contradictions and propose reusable patterns
    Analyze {
        /// Minimum entries in a tag group to propose a pattern
        #[arg(long, default_value_t = 2)]
        min_pattern_members: usize,
        /// Materialise each proposed pattern as a new pattern entry
        #[arg(long)]
        write_patterns: bool,
    },
    /// Show vault statistics
    Stats,
    /// Check vault health
    Doctor,
    /// Open a local browser dashboard for vault memories, health, and configuration
    Ui {
        /// Relic vault to inspect; omit to discover the vault from the current directory
        #[arg(long)]
        vault: Option<PathBuf>,
        /// Loopback address to listen on (remote access is not supported)
        #[arg(long, default_value = "127.0.0.1:7338")]
        bind: SocketAddr,
    },
    /// Commit, pull, and push the vault with a git remote
    Sync {
        /// Remote name to push to and pull from; defaults to the first
        /// configured sync remote, then "origin"
        #[arg(long)]
        remote: Option<String>,
        /// Branch to sync; defaults to the currently checked-out branch
        #[arg(long)]
        branch: Option<String>,
        /// Commit message for local changes
        #[arg(long, default_value = "relic: sync knowledge vault")]
        message: String,
    },
    /// Keep the index warm and auto-reflect (maintenance daemon)
    Watch {
        /// Work once and exit instead of looping
        #[arg(long)]
        once: bool,
        /// Seconds between maintenance passes in loop mode
        #[arg(long, default_value_t = 60)]
        interval: u64,
        /// Reflection period for auto-reflection
        #[arg(long, default_value = "weekly")]
        reflect_period: String,
        /// Minimum entries required to auto-reflect
        #[arg(long, default_value_t = 5)]
        min_entries: usize,
    },
    /// Connect Relic to an agent project
    Integrate {
        #[command(subcommand)]
        agent: Integration,
    },
    /// Run the Model Context Protocol server over standard input/output
    Mcp {
        /// Relic vault exposed to connected agents
        #[arg(long)]
        vault: PathBuf,
        /// Agent name recorded when a client does not supply source_agent
        #[arg(long, default_value = "mcp-agent")]
        source_agent: String,
    },
    /// Run the MCP server over Streamable HTTP
    McpHttp {
        /// Relic vault exposed to connected agents
        #[arg(long)]
        vault: PathBuf,
        /// Address to listen on
        #[arg(long, default_value = "127.0.0.1:7337")]
        bind: SocketAddr,
        /// Agent name recorded when a client does not supply source_agent
        #[arg(long, default_value = "http-agent")]
        source_agent: String,
        /// Environment variable containing the required Bearer token
        #[arg(long)]
        bearer_token_env: Option<String>,
        /// Additional accepted browser Origin values
        #[arg(long)]
        allow_origin: Vec<String>,
    },
}

#[derive(Subcommand)]
enum DistillerCommand {
    Status,
    /// Read a command-backend configuration JSON file and enable it locally
    Configure {
        file: PathBuf,
    },
    /// Return to template drafts
    Disable,
}

#[derive(Subcommand)]
enum QueueCommand {
    /// Requeue an unreviewed draft using the current distiller
    Redistill { capture_id: uuid::Uuid },
    /// List capture states (including candidates, outside knowledge search)
    List,
    /// Read a source event and its candidate
    Get { capture_id: uuid::Uuid },
    /// Prepare a bounded batch of review drafts; also resumes interrupted work
    Work {
        #[arg(long, default_value_t = 100)]
        limit: usize,
        /// Process just this capture; unrelated pending events are untouched
        #[arg(long)]
        capture_id: Option<uuid::Uuid>,
    },
    /// Accept or reject a candidate using a JSON review file
    Review {
        capture_id: uuid::Uuid,
        file: PathBuf,
    },
    /// Requeue a failed capture
    Retry { capture_id: uuid::Uuid },
}

#[derive(Args)]
pub struct CodexMemoryArgs {
    /// Path to Codex's MEMORY.md; defaults to $CODEX_HOME/memories/MEMORY.md
    #[arg(long)]
    pub source: Option<PathBuf>,
    /// Confidence for imported memories
    #[arg(long)]
    pub confidence: Option<f64>,
    /// Report what would be imported without writing anything
    #[arg(long)]
    pub dry_run: bool,
    /// Refresh stored text when a source bullet already has a matching entry
    #[arg(long)]
    pub update: bool,
    /// Skip task groups whose name contains this string (repeatable)
    #[arg(long = "exclude-task-group")]
    pub exclude_task_group: Vec<String>,
}

#[derive(Subcommand)]
enum Integration {
    /// Add Relic MCP configuration for Codex
    Codex(IntegrationArgs),
    /// Add Relic MCP configuration for Claude Code
    Claude(IntegrationArgs),
    /// Add Relic MCP configuration for Cursor
    Cursor(IntegrationArgs),
    /// Add Relic MCP configuration for Gemini CLI
    Gemini(IntegrationArgs),
    /// Add Relic MCP configuration for VS Code and Copilot
    Vscode(IntegrationArgs),
    /// Add Relic MCP configuration for DeepSeek Harness
    Dsh(DshIntegrationArgs),
}

#[derive(Args)]
struct IntegrationArgs {
    /// Relic vault exposed to the agent
    #[arg(long)]
    vault: PathBuf,
    /// Agent project to configure; omit for user-global configuration
    #[arg(long)]
    project: Option<PathBuf>,
    /// Add active Relic memory guidance to the agent's instruction file
    #[arg(long)]
    update_agents: bool,
    /// Print the files that would change without writing them
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args)]
struct DshIntegrationArgs {
    /// Relic vault exposed to DSH
    #[arg(long)]
    vault: PathBuf,
    /// DSH profile to configure; omit for global DSH configuration
    #[arg(long)]
    profile: Option<String>,
    /// DSH data directory (defaults to DSH_HOME or ~/.dsh)
    #[arg(long)]
    dsh_home: Option<PathBuf>,
    /// Add active Relic guidance to profile or global AGENTS.md
    #[arg(long)]
    update_agents: bool,
    /// Print the files that would change without writing them
    #[arg(long)]
    dry_run: bool,
}

#[derive(Subcommand)]
enum GraphCommand {
    /// Rebuild the vector layer and the relation network from Markdown
    Build {
        /// Print the resulting graph as JSON
        #[arg(long)]
        json: bool,
        /// Drop every derived artifact first, so the rebuild starts from nothing
        #[arg(long)]
        force: bool,
    },
    /// Summarise the graph: relation counts, components, hubs, and orphans
    Stats {
        /// Print the summary as JSON
        #[arg(long)]
        json: bool,
        /// Ignore relations weaker than this weight
        #[arg(long, default_value_t = 0.0, value_name = "WEIGHT")]
        min_weight: f64,
    },
    /// List everything reachable from one memory
    Neighbors {
        entry_id: String,
        /// Maximum hops
        #[arg(long, default_value_t = 1)]
        depth: usize,
        /// Ignore hops weaker than this weight
        #[arg(long, default_value_t = 0.0, value_name = "WEIGHT")]
        min_weight: f64,
        /// Restrict to relation kinds (comma separated, repeatable)
        #[arg(long = "kind", value_delimiter = ',')]
        kinds: Vec<String>,
        /// Print the result as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show the strongest chain of relations between two memories
    Path {
        from: String,
        to: String,
        /// Refuse to traverse hops weaker than this weight
        #[arg(long, default_value_t = 0.0, value_name = "WEIGHT")]
        min_weight: f64,
        /// Print the result as JSON
        #[arg(long)]
        json: bool,
    },
    /// Explain every relation between two memories, with its evidence
    Explain { from: String, to: String },
    /// List the memories closest to one memory in the vector space
    Similar {
        entry_id: String,
        #[arg(short = 'n', long, default_value_t = 10)]
        limit: usize,
        /// Minimum cosine similarity
        #[arg(long, value_name = "SIMILARITY")]
        min_similarity: Option<f64>,
        /// Print the result as JSON
        #[arg(long)]
        json: bool,
    },
    /// Export the graph for Graphviz or Mermaid
    Export {
        /// dot or mermaid
        #[arg(long, default_value = "mermaid")]
        format: String,
        /// Omit relations weaker than this weight
        #[arg(long, default_value_t = 0.0, value_name = "WEIGHT")]
        min_weight: f64,
    },
}

impl Cli {
    pub fn run(self) -> Result<()> {
        match self.command {
            Command::Distiller { command } => {
                let vault = current_vault()?;
                match command {
                    DistillerCommand::Configure { file } => {
                        let config = serde_json::from_slice(&std::fs::read(file)?)?;
                        crate::distillation::configure(&vault, &config)?;
                    }
                    DistillerCommand::Disable => crate::distillation::configure(
                        &vault,
                        &crate::distillation::Config::Template,
                    )?,
                    DistillerCommand::Status => {}
                }
                println!(
                    "{}",
                    serde_json::to_string_pretty(&crate::distillation::config(&vault)?)?
                );
            }
            Command::ImportCodexMemory(args) => {
                let vault = current_vault()?;
                let report = crate::codex_memory::import(
                    &vault,
                    crate::codex_memory::ImportRequest {
                        source: args.source,
                        update: args.update,
                        dry_run: args.dry_run,
                        exclude_task_groups: args.exclude_task_group,
                        confidence: args.confidence,
                    },
                )?;
                println!("{}", serde_json::to_string_pretty(&report.value())?);
            }
            Command::Setup {
                vault,
                project,
                dsh_home,
                host,
            } => {
                let vault = Vault {
                    root: std::fs::canonicalize(vault)?,
                };
                match host.as_str() {
                    "worker" => crate::automation::start(&vault, &env::current_exe()?)?,
                    "stop" => crate::automation::stop(&vault)?,
                    _ => crate::setup::install(
                        &vault,
                        &project,
                        &dsh_home.unwrap_or_else(crate::setup::default_dsh_home),
                        &env::current_exe()?,
                        &host,
                    )?,
                }
                println!("{}", crate::automation::status(&vault)?);
            }
            Command::Hook { vault, host } => {
                use std::io::Read;
                let mut bytes = Vec::new();
                std::io::stdin()
                    .take(1024 * 1024 + 1)
                    .read_to_end(&mut bytes)?;
                anyhow::ensure!(bytes.len() <= 1024 * 1024, "Hook input exceeds 1 MiB");
                let output = crate::hooks::handle(
                    &Vault { root: vault },
                    &host,
                    serde_json::from_slice(&bytes)?,
                )?;
                println!("{}", serde_json::to_string(&output)?);
            }
            Command::InstallHooks {
                vault,
                config,
                dry_run,
            } => {
                let output = crate::hooks::install(&config, &vault, &env::current_exe()?, dry_run)?;
                println!("{}", serde_json::to_string_pretty(&output)?);
                if !dry_run {
                    eprintln!(
                        "Hooks installed. Review and trust the changed hooks in Codex, then start a new task. Start relic daemon separately."
                    );
                }
            }
            Command::Daemon { vault, once, retry } => {
                let vault = Vault { root: vault };
                vault.config()?;
                let stop_generation = crate::automation::stop_generation(&vault);
                let _lease = if once {
                    None
                } else {
                    let lease = crate::automation::lease(&vault)?;
                    Some(lease)
                };
                let mut retry = retry;
                loop {
                    if !once && crate::automation::stop_requested(&vault, &stop_generation) {
                        break;
                    }
                    match crate::automation::tick(&vault, retry) {
                        Ok(state) => {
                            if once {
                                println!("{}", serde_json::to_string_pretty(&state)?);
                            }
                        }
                        Err(e) if once => return Err(e),
                        Err(e) => eprintln!("Worker: {e:#}"),
                    }
                    retry = false;
                    if once {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_secs(5));
                }
            }
            Command::Init { path } => {
                let vault = Vault::init(&path.canonicalize().unwrap_or(path))?;
                println!("Initialized Relic vault at {}", vault.root.display());
            }
            Command::Add {
                title,
                content,
                kind,
                tags,
                confidence,
                source_agent,
            } => {
                let vault = current_vault()?;
                let entry =
                    vault.create(&title, &content, &kind, tags, confidence, &source_agent)?;
                println!(
                    "{}\t{}",
                    entry.meta.id,
                    entry.path.strip_prefix(&vault.root)?.display()
                );
            }
            Command::Capture { file } => {
                let mut input: crate::capture::CaptureInput =
                    serde_json::from_slice(&std::fs::read(file)?)?;
                if input.source_agent.is_empty() {
                    input.source_agent = "cli".into();
                }
                println!(
                    "{}",
                    serde_json::to_string_pretty(&crate::capture::capture(
                        &current_vault()?,
                        input
                    )?)?
                );
            }
            Command::Queue { command } => {
                let vault = current_vault()?;
                let result = match command {
                    QueueCommand::Redistill { capture_id } => {
                        serde_json::to_value(crate::capture::redistill(&vault, capture_id)?)?
                    }
                    QueueCommand::List => serde_json::to_value(crate::capture::list(&vault)?)?,
                    QueueCommand::Get { capture_id } => {
                        let (source, item) = crate::capture::get(&vault, capture_id)?;
                        serde_json::json!({"source": source, "item": item})
                    }
                    QueueCommand::Work { limit, capture_id } => {
                        let report = match capture_id {
                            Some(id) => crate::capture::work_one(&vault, id)?,
                            None => crate::capture::work(&vault, limit)?,
                        };
                        serde_json::to_value(report)?
                    }
                    QueueCommand::Review { capture_id, file } => {
                        let review = serde_json::from_slice(&std::fs::read(file)?)?;
                        serde_json::to_value(crate::capture::review(&vault, capture_id, review)?)?
                    }
                    QueueCommand::Retry { capture_id } => {
                        serde_json::to_value(crate::capture::retry(&vault, capture_id)?)?
                    }
                };
                println!("{}", serde_json::to_string_pretty(&result)?);
            }
            Command::Update {
                entry_id,
                title,
                content,
                kind,
                status,
                confidence,
                tags,
                source_agents,
                links,
            } => {
                let entry = current_vault()?.update(
                    &entry_id,
                    EntryPatch {
                        title,
                        content,
                        kind,
                        status,
                        confidence,
                        tags,
                        source_agents,
                        links,
                    },
                )?;
                println!(
                    "{}\t{}\t{}",
                    entry.meta.id, entry.meta.status, entry.meta.title
                );
            }
            Command::Supersede {
                old_entry_id,
                new_entry_id,
            } => {
                let (old, new) = current_vault()?.supersede(&old_entry_id, &new_entry_id)?;
                println!("{}\tsuperseded by\t{}", old.meta.id, new.meta.id);
            }
            Command::Get { entry_id } => println!("{}", current_vault()?.get(&entry_id)?.render()?),
            Command::List {
                kind,
                status,
                tags,
                source_agent,
                min_confidence,
                max_confidence,
            } => {
                for entry in current_vault()?.entries()?.into_iter().filter(|entry| {
                    let confidence = entry.meta.effective_confidence();
                    kind.as_ref().is_none_or(|v| &entry.meta.kind == v)
                        && status.as_ref().is_none_or(|v| &entry.meta.status == v)
                        && tags.iter().all(|tag| entry.meta.tags.contains(tag))
                        && source_agent
                            .as_ref()
                            .is_none_or(|agent| entry.meta.source_agents.contains(agent))
                        && min_confidence.is_none_or(|minimum| confidence >= minimum)
                        && max_confidence.is_none_or(|maximum| confidence <= maximum)
                }) {
                    println!(
                        "{}\t{:.2}\t{}\t{}",
                        entry.meta.id,
                        entry.meta.effective_confidence(),
                        entry.meta.status,
                        entry.meta.title
                    );
                }
            }
            Command::Search { query, limit, mode } => {
                let vault = current_vault()?;
                let mode = crate::search::Mode::parse(&mode)?;
                for hit in vault.hybrid_search(&query, limit, mode)? {
                    println!(
                        "{}  {:.2}  {}\n  {}",
                        hit.id,
                        hit.confidence,
                        hit.title,
                        hit.excerpt.replace('\n', " ")
                    );
                    let mut because = Vec::new();
                    if let Some(rank) = hit.keyword_rank {
                        because.push(format!("keyword #{rank}"));
                    }
                    if let Some(rank) = hit.semantic_rank {
                        because.push(format!(
                            "semantic #{rank} (cosine {:.3})",
                            hit.similarity.unwrap_or_default()
                        ));
                    }
                    if !because.is_empty() {
                        println!("  matched by: {}", because.join(", "));
                    }
                    if !hit.shared_terms.is_empty() {
                        println!("  related vocabulary: {}", hit.shared_terms.join(", "));
                    }
                }
            }
            Command::Graph { command } => {
                let vault = current_vault()?;
                run_graph_command(&vault, command)?;
            }
            Command::Reindex => println!("Indexed {} entries", current_vault()?.reindex()?),
            Command::Reflect {
                period,
                auto,
                min_entries,
            } => {
                let vault = current_vault()?;
                if auto && !vault.should_reflect(&period, min_entries)? {
                    if vault.reflection_path(&period)?.is_some() {
                        println!("Reflection already exists; skipping");
                    } else {
                        let count = vault.entries()?.len();
                        println!(
                            "Skipping reflection: only {count} entries (need >= {min_entries})"
                        );
                    }
                    return Ok(());
                }
                println!("Created {}", vault.create_reflection(&period)?.display());
            }
            Command::Analyze {
                min_pattern_members,
                write_patterns,
            } => {
                let vault = current_vault()?;
                let entries = vault.entries()?;
                let contradictions = crate::analysis::detect_contradictions(&entries);
                let patterns = crate::analysis::extract_patterns(&entries, min_pattern_members);
                if contradictions.is_empty() {
                    println!("No contradictions detected.");
                } else {
                    println!("Contradictions ({}):", contradictions.len());
                    for contradiction in &contradictions {
                        println!(
                            "  {} vs {} ({})",
                            contradiction.a, contradiction.b, contradiction.reason
                        );
                    }
                }
                if patterns.is_empty() {
                    println!(
                        "No candidate patterns (need >= {min_pattern_members} entries per tag)."
                    );
                } else {
                    println!("Candidate patterns ({}):", patterns.len());
                    for pattern in &patterns {
                        println!("  {} ({} members)", pattern.tag, pattern.members.len());
                    }
                }
                if write_patterns {
                    let mut wrote = 0;
                    for pattern in &patterns {
                        let entry = vault.write_pattern(&pattern.tag)?;
                        wrote += 1;
                        println!("  wrote {}", entry.meta.id);
                    }
                    println!("Wrote {wrote} pattern(s).");
                }
            }
            Command::Stats => {
                let entries = current_vault()?.entries()?;
                let active = entries.iter().filter(|e| e.meta.status == "active").count();
                let avg = if entries.is_empty() {
                    0.0
                } else {
                    entries
                        .iter()
                        .map(|e| e.meta.effective_confidence())
                        .sum::<f64>()
                        / entries.len() as f64
                };
                println!(
                    "entries: {}\nactive: {}\naverage effective confidence: {:.2}",
                    entries.len(),
                    active,
                    avg
                );
            }
            Command::Doctor => {
                let vault = current_vault()?;
                let entries = vault.entries()?;
                vault.reindex()?;
                let config = match vault.config() {
                    Ok(config) => {
                        println!(
                            "config ok (schema {}; {} sync remote(s))",
                            config.version,
                            config.sync.remotes.len()
                        );
                        Some(config)
                    }
                    Err(error) => {
                        println!("config problem: {error:#}");
                        None
                    }
                };
                println!(
                    "Vault healthy: {} valid entries; index rebuilt",
                    entries.len()
                );
                let _ = config;
            }
            Command::Ui { vault, bind } => {
                let vault = match vault {
                    Some(path) => Vault::discover(&path.canonicalize().unwrap_or(path))?,
                    None => current_vault()?,
                };
                crate::ui::serve(vault, bind)?;
            }
            Command::Sync {
                remote,
                branch,
                message,
            } => {
                let vault = current_vault()?;
                let branch = match branch {
                    Some(branch) => branch,
                    None => crate::git::current_branch(&vault.root)?,
                };
                let remote = match remote {
                    Some(remote) => remote,
                    None => {
                        let config = vault.config()?;
                        if let Some(first) = config.sync.remotes.first() {
                            crate::git::ensure_remote(&vault.root, &first.name, &first.url)?;
                            first.name.clone()
                        } else {
                            "origin".to_string()
                        }
                    }
                };
                let outcome = crate::git::sync(&vault.root, &remote, &branch, &message)?;
                println!(
                    "Synced {}/{} at {}",
                    outcome.remote, outcome.branch, outcome.head
                );
                println!("committed local changes: {}", outcome.committed);
                println!("pulled remote changes: {}", outcome.pulled);
                if !outcome.resolved.is_empty() {
                    println!(
                        "resolved conflicts (remote kept, local preserved under .relic/conflicts):"
                    );
                    for path in &outcome.resolved {
                        println!("  {path}");
                    }
                }
                println!("pushed to {}/{}", outcome.remote, outcome.branch);
            }
            Command::Watch {
                once,
                interval,
                reflect_period,
                min_entries,
            } => {
                let vault = current_vault()?;
                loop {
                    let maintenance = vault.maintain(&reflect_period, min_entries)?;
                    println!(
                        "maintained {} entries (reflected: {}; prepared: {}; recovered: {}; failed: {})",
                        maintenance.entries,
                        maintenance.reflected,
                        maintenance.captures.prepared,
                        maintenance.captures.recovered,
                        maintenance.captures.failed
                    );
                    for error in &maintenance.captures.errors {
                        eprintln!("capture: {error}");
                    }
                    if once {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_secs(interval));
                }
            }
            Command::Integrate { agent } => {
                if let Integration::Dsh(args) = &agent {
                    let dsh_home = args.dsh_home.clone().unwrap_or_else(default_dsh_home);
                    let outcome = crate::integration::integrate_dsh(
                        &args.vault,
                        &env::current_exe()?,
                        args.profile.as_deref(),
                        &dsh_home,
                        args.update_agents,
                        args.dry_run,
                    )?;
                    print_integration_outcome(outcome, args.dry_run);
                    if args.profile.is_some() && args.update_agents {
                        eprintln!(
                            "Note: DSH does not automatically load profile AGENTS.md files; omit \
                             --profile to update the globally loaded $DSH_HOME/AGENTS.md"
                        );
                    }
                    return Ok(());
                }
                let (kind, args) = match agent {
                    Integration::Codex(args) => (crate::integration::AgentKind::Codex, args),
                    Integration::Claude(args) => (crate::integration::AgentKind::Claude, args),
                    Integration::Cursor(args) => (crate::integration::AgentKind::Cursor, args),
                    Integration::Gemini(args) => (crate::integration::AgentKind::Gemini, args),
                    Integration::Vscode(args) => (crate::integration::AgentKind::Vscode, args),
                    Integration::Dsh(_) => unreachable!(),
                };
                let executable = env::current_exe()?;
                let outcome = if let Some(project) = args.project.as_deref() {
                    crate::integration::integrate(
                        kind,
                        project,
                        &args.vault,
                        &executable,
                        args.update_agents,
                        args.dry_run,
                    )?
                } else {
                    crate::integration::integrate_global(
                        kind,
                        &default_home(),
                        &args.vault,
                        &executable,
                        args.update_agents,
                        args.dry_run,
                    )?
                };
                print_integration_outcome(outcome, args.dry_run);
            }
            Command::Mcp {
                vault,
                source_agent,
            } => {
                let vault = Vault::discover(&vault.canonicalize().unwrap_or(vault))?;
                crate::mcp::serve_with_source_agent(vault, &source_agent)?;
            }
            Command::McpHttp {
                vault,
                bind,
                source_agent,
                bearer_token_env,
                allow_origin,
            } => {
                let vault = Vault::discover(&vault.canonicalize().unwrap_or(vault))?;
                let bearer_token = bearer_token_env
                    .as_deref()
                    .map(|name| {
                        env::var(name)
                            .map_err(anyhow::Error::from)
                            .and_then(|value| {
                                if value.trim().is_empty() {
                                    anyhow::bail!(
                                        "Bearer token environment variable '{name}' is empty"
                                    )
                                }
                                Ok(value)
                            })
                    })
                    .transpose()?;
                crate::mcp::serve_http(vault, bind, &source_agent, bearer_token, allow_origin)?;
            }
        }
        Ok(())
    }
}

fn current_vault() -> Result<Vault> {
    Vault::discover(&env::current_dir()?)
}

fn default_dsh_home() -> PathBuf {
    env::var_os("DSH_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".dsh")))
        .unwrap_or_else(|| PathBuf::from(".dsh"))
}

fn default_home() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn print_integration_outcome(outcome: crate::integration::IntegrationOutcome, dry_run: bool) {
    let action = if dry_run { "Would update" } else { "Updated" };
    println!("{action} {}", outcome.config_path.display());
    if let Some(path) = outcome.agents_path {
        println!("{action} {}", path.display());
    }
}

fn run_graph_command(vault: &Vault, command: GraphCommand) -> Result<()> {
    match command {
        GraphCommand::Build { json, force } => {
            if force {
                // Derived artifacts are disposable by contract; removing them
                // first proves the rebuild reads only Markdown.
                let _ = std::fs::remove_file(vault.graph_path());
                let _ = std::fs::remove_file(vault.embeddings_path());
            }
            let graph = vault.rebuild_graph()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&graph)?);
            } else {
                let stats = graph.stats(0.0);
                println!(
                    "Built a relation network over {} memories: {} relations, {} component(s), {} isolated.",
                    stats.nodes, stats.edges, stats.components, stats.isolated
                );
                println!(
                    "  {} dimensions, fingerprints {}",
                    graph.dimensions,
                    &graph.fingerprint[..12.min(graph.fingerprint.len())]
                );
                for (kind, count) in &stats.edges_by_kind {
                    println!("  {kind:<13} {count}");
                }
                for note in &stats.notes {
                    println!("  note: {note}");
                }
            }
        }
        GraphCommand::Stats { json, min_weight } => {
            let stats = vault.graph()?.stats(min_weight);
            if json {
                println!("{}", serde_json::to_string_pretty(&stats)?);
                return Ok(());
            }
            println!(
                "{} memories, {} relations, density {:.4}, mean degree {:.2}",
                stats.nodes, stats.edges, stats.density, stats.mean_degree
            );
            println!(
                "{} component(s), largest {}, {} isolated",
                stats.components, stats.largest_component, stats.isolated
            );
            for (kind, count) in &stats.edges_by_kind {
                println!("  {kind:<13} {count}");
            }
            for (derivation, count) in &stats.edges_by_derivation {
                println!("  by derivation {derivation:<12} {count}");
            }
            if !stats.hubs.is_empty() {
                println!("Hubs:");
                for hub in &stats.hubs {
                    println!(
                        "  {}  degree {}  weighted {:.2}  {}",
                        hub.id, hub.degree, hub.weighted_degree, hub.title
                    );
                }
            }
            if !stats.orphans.is_empty() {
                println!("Isolated ({}):", stats.orphans.len());
                for id in stats.orphans.iter().take(10) {
                    println!("  {id}");
                }
            }
            for note in &stats.notes {
                println!("note: {note}");
            }
        }
        GraphCommand::Neighbors {
            entry_id,
            depth,
            min_weight,
            kinds,
            json,
        } => {
            let kinds = crate::graph::parse_kinds(&kinds)?;
            let hits =
                vault
                    .graph()?
                    .neighborhood(&entry_id, depth, min_weight, kinds.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&hits)?);
                return Ok(());
            }
            if hits.is_empty() {
                println!("Nothing within {depth} hop(s) of {entry_id}.");
                return Ok(());
            }
            for hit in hits {
                println!(
                    "d{}  {:.3}  {:<13} {}  {}",
                    hit.depth,
                    hit.score,
                    hit.kind.as_str(),
                    hit.id,
                    hit.title
                );
                println!("     via {} — {}", hit.via, hit.evidence);
            }
        }
        GraphCommand::Path {
            from,
            to,
            min_weight,
            json,
        } => {
            let path = vault.graph()?.path(&from, &to, min_weight)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&path)?);
                return Ok(());
            }
            let Some(path) = path else {
                println!("No chain of relations links {from} to {to}.");
                return Ok(());
            };
            println!(
                "{} hop(s), confidence {:.3} (the weakest link decides):",
                path.edges.len(),
                path.bottleneck
            );
            for (index, id) in path.nodes.iter().enumerate() {
                let title = vault
                    .graph()?
                    .node(id)
                    .map(|node| node.title.clone())
                    .unwrap_or_default();
                println!("  {id}  {title}");
                if let Some(edge) = path.edges.get(index) {
                    println!(
                        "    --{} {:.3}--> {}",
                        edge.kind.as_str(),
                        edge.weight,
                        edge.evidence
                    );
                }
            }
        }
        GraphCommand::Explain { from, to } => {
            let edges = vault.graph()?.explain(&from, &to);
            if edges.is_empty() {
                println!("No relation recorded between {from} and {to}.");
                return Ok(());
            }
            for edge in edges {
                println!(
                    "{:<13} {:.3}  {}\n  {}",
                    edge.kind.as_str(),
                    edge.weight,
                    edge.derivation.as_str(),
                    edge.evidence
                );
            }
        }
        GraphCommand::Similar {
            entry_id,
            limit,
            min_similarity,
            json,
        } => {
            let minimum = match min_similarity {
                Some(value) => value,
                None => vault.config()?.graph.semantic_min_similarity,
            };
            let related = vault.similar(&entry_id, limit, minimum)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&related)?);
                return Ok(());
            }
            if related.is_empty() {
                println!("No memory is within {minimum:.3} of {entry_id}.");
                return Ok(());
            }
            for hit in related {
                println!("{:.3}  {}  {}", hit.similarity, hit.id, hit.title);
                if !hit.shared_terms.is_empty() {
                    println!("     related vocabulary: {}", hit.shared_terms.join(", "));
                }
            }
        }
        GraphCommand::Export { format, min_weight } => {
            let graph = vault.graph()?;
            match format.as_str() {
                "dot" | "graphviz" => print!("{}", graph.to_dot(min_weight)),
                "mermaid" => print!("{}", graph.to_mermaid(min_weight)),
                "json" => println!("{}", serde_json::to_string_pretty(&graph)?),
                other => {
                    anyhow::bail!(
                        "unknown export format '{other}' (expected dot, mermaid, or json)"
                    )
                }
            }
        }
    }
    Ok(())
}
