use crate::vault::{EntryPatch, Vault};
use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use std::env;
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
    },
    /// Full-text search the vault
    Search {
        query: String,
        #[arg(short = 'n', long, default_value_t = 10)]
        limit: usize,
    },
    /// Rebuild the local search index
    Reindex,
    /// Create a reflection draft
    Reflect {
        #[arg(long, default_value = "weekly")]
        period: String,
    },
    /// Show vault statistics
    Stats,
    /// Check vault health
    Doctor,
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

impl Cli {
    pub fn run(self) -> Result<()> {
        match self.command {
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
            Command::List { kind, status } => {
                for entry in current_vault()?.entries()?.into_iter().filter(|entry| {
                    kind.as_ref().is_none_or(|v| &entry.meta.kind == v)
                        && status.as_ref().is_none_or(|v| &entry.meta.status == v)
                }) {
                    println!(
                        "{}\t{:.2}\t{}\t{}",
                        entry.meta.id, entry.meta.confidence, entry.meta.status, entry.meta.title
                    );
                }
            }
            Command::Search { query, limit } => {
                for hit in current_vault()?.search(&query, limit)? {
                    println!(
                        "{}  {:.2}  {}\n  {}",
                        hit.id,
                        hit.confidence,
                        hit.title,
                        hit.excerpt.replace('\n', " ")
                    );
                }
            }
            Command::Reindex => println!("Indexed {} entries", current_vault()?.reindex()?),
            Command::Reflect { period } => println!(
                "Created {}",
                current_vault()?.create_reflection(&period)?.display()
            ),
            Command::Stats => {
                let entries = current_vault()?.entries()?;
                let active = entries.iter().filter(|e| e.meta.status == "active").count();
                let avg = if entries.is_empty() {
                    0.0
                } else {
                    entries.iter().map(|e| e.meta.confidence).sum::<f64>() / entries.len() as f64
                };
                println!(
                    "entries: {}\nactive: {}\naverage confidence: {:.2}",
                    entries.len(),
                    active,
                    avg
                );
            }
            Command::Doctor => {
                let vault = current_vault()?;
                let entries = vault.entries()?;
                vault.reindex()?;
                println!(
                    "Vault healthy: {} valid entries; index rebuilt",
                    entries.len()
                );
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
